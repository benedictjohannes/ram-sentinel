use crate::config_error::ConfigError;
use crate::events::{LogLevel, SentinelEvent};
use crate::logging;
use crate::psi;
use crate::utils::parse_size;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Config {
    // Metric Triggers
    pub psi: Option<psi::PsiConfig>,
    pub ram: Option<MemoryConfig>,
    pub swap: Option<MemoryConfig>,
    pub zram: Option<MemoryConfig>,

    // Operational Settings
    #[serde(default = "default_interval")]
    pub check_interval_ms: u64,
    #[serde(default = "warn_interval")]
    pub warn_reset_ms: u64,
    #[serde(default = "sigterm_wait_ms")]
    pub sigterm_wait_ms: u64,

    // Targeting Logic
    #[serde(default = "default_ignore_targets")]
    pub ignore_targets: Vec<String>,

    #[serde(default = "default_kill_targets")]
    pub kill_targets: Vec<String>,

    #[serde(default = "default_strategy")]
    pub kill_strategy: KillStrategy,

    #[serde(default = "default_allow_kill_outside_targets")]
    pub allow_kill_outside_targets: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MemoryConfig {
    pub warn_min_free_bytes: Option<String>,
    pub warn_min_free_percent: Option<f32>,
    pub kill_min_free_bytes: Option<String>,
    pub kill_min_free_percent: Option<f32>,
}
#[derive(Debug, Clone)]
pub struct MemoryConfigParsed {
    pub warn_min_free_bytes: Option<u64>,
    pub warn_min_free_percent: Option<f32>,
    pub kill_min_free_bytes: Option<u64>,
    pub kill_min_free_percent: Option<f32>,
}

impl MemoryConfigParsed {
    pub fn try_from_config(config: MemoryConfig) -> Result<Self, ConfigError> {
        let warn_min_free_bytes = if let Some(s) = config.warn_min_free_bytes.as_ref() {
            Some(parse_size(s).ok_or_else(|| {
                ConfigError::InvalidSize("warn_min_free_bytes".to_string(), s.clone())
            })?)
        } else {
            None
        };

        let kill_min_free_bytes = if let Some(s) = config.kill_min_free_bytes.as_ref() {
            Some(parse_size(s).ok_or_else(|| {
                ConfigError::InvalidSize("kill_min_free_bytes".to_string(), s.clone())
            })?)
        } else {
            None
        };

        if let Some(p) = config.warn_min_free_percent {
            if !(0.0..=100.0).contains(&p) {
                return Err(ConfigError::InvalidPercent(
                    "warn_min_free_percent".to_string(),
                    p,
                ));
            }
        }

        if let Some(p) = config.kill_min_free_percent {
            if !(0.0..=100.0).contains(&p) {
                return Err(ConfigError::InvalidPercent(
                    "kill_min_free_percent".to_string(),
                    p,
                ));
            }
        }

        Ok(Self {
            warn_min_free_bytes,
            warn_min_free_percent: config.warn_min_free_percent,
            kill_min_free_bytes,
            kill_min_free_percent: config.kill_min_free_percent,
        })
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum KillStrategy {
    LargestRss,
    HighestOomScore,
}

impl MemoryConfig {
    fn is_effectively_empty(&self) -> bool {
        self.warn_min_free_bytes.is_none()
            && self.warn_min_free_percent.is_none()
            && self.kill_min_free_bytes.is_none()
            && self.kill_min_free_percent.is_none()
    }
}

// Default Generators
fn default_interval() -> u64 {
    1000
}
fn warn_interval() -> u64 {
    30000
}
fn sigterm_wait_ms() -> u64 {
    5000
}
fn default_strategy() -> KillStrategy {
    KillStrategy::HighestOomScore
}
fn default_kill_targets() -> Vec<String> {
    vec!["type=renderer".to_string(), "-contentproc".to_string()]
}
fn default_allow_kill_outside_targets() -> bool {
    false
}
fn default_ignore_targets() -> Vec<String> {
    vec!["ram-sentinel".to_string()]
}

#[derive(Debug)]
pub struct RuntimeContext {
    pub psi: Option<psi::PsiConfigParsed>,
    pub ram: Option<MemoryConfigParsed>,
    pub swap: Option<MemoryConfigParsed>,
    pub zram: Option<MemoryConfigParsed>,

    pub check_interval_ms: u64,
    pub warn_reset_ms: u64,
    pub sigterm_wait_ms: u64,

    pub kill_strategy: KillStrategy,

    pub ignore_targets_regex: Vec<TargetPattern>,
    pub kill_targets_regex: Vec<TargetPattern>,
    pub allow_kill_outside_targets: bool,
}

#[derive(Debug)]
pub struct TargetPattern {
    pub cgroup: Option<Pattern>,
    pub cmdline: Pattern,
}

#[derive(Debug)]
pub enum Pattern {
    Literal(String),
    Regex(Regex),
    StartsWith(String),
}

impl Pattern {
    pub fn matches(&self, s: &str) -> bool {
        match self {
            Pattern::Literal(lit) => s.contains(lit),
            Pattern::Regex(re) => re.is_match(s),
            Pattern::StartsWith(prefix) => s.starts_with(prefix),
        }
    }
}

impl Config {
    pub fn load(cli_config_path: Option<PathBuf>) -> Result<RuntimeContext, ConfigError> {
        let config = Self::load_raw_validated(cli_config_path)?;

        // Optimization: Compile Regex patterns
        let ignore_targets_regex = compile_patterns(&config.ignore_targets, "ignore_targets")?;
        let kill_targets_regex = compile_patterns(&config.kill_targets, "kill_targets")?;

        let psi_parsed = if let Some(p) = config.psi {
            let parsed = psi::PsiConfigParsed::try_from_config(p, config.check_interval_ms)
                .map_err(|e| ConfigError::PsiConfig(e.to_string()))?;

            if let Err(e) = psi::validate_psi_availability() {
                return Err(ConfigError::PsiUnavailable(e.to_string()));
            }
            Some(parsed)
        } else {
            None
        };

        let ram_parsed = if let Some(r) = config.ram {
            Some(MemoryConfigParsed::try_from_config(r)?)
        } else {
            None
        };

        let swap_parsed = if let Some(s) = config.swap {
            Some(MemoryConfigParsed::try_from_config(s)?)
        } else {
            None
        };

        let zram_parsed = if let Some(z) = config.zram {
            Some(MemoryConfigParsed::try_from_config(z)?)
        } else {
            None
        };

        Ok(RuntimeContext {
            psi: psi_parsed,
            ram: ram_parsed,
            swap: swap_parsed,
            zram: zram_parsed,
            check_interval_ms: config.check_interval_ms,
            warn_reset_ms: config.warn_reset_ms,
            sigterm_wait_ms: config.sigterm_wait_ms,
            kill_strategy: config.kill_strategy,
            ignore_targets_regex,
            kill_targets_regex,
            allow_kill_outside_targets: config.allow_kill_outside_targets,
        })
    }

    pub fn load_raw_validated(cli_config_path: Option<PathBuf>) -> Result<Config, ConfigError> {
        let mut config = match cli_config_path {
            Some(path) => {
                if !path.exists() {
                    return Err(ConfigError::ConfigFileNotFound(path));
                }
                Self::parse_file(&path)?
            }
            None => Self::find_and_load_config()?,
        };

        config.validate()?;
        Ok(config)
    }

    fn find_and_load_config() -> Result<Config, ConfigError> {
        if let Some(config_home) =
            directories::BaseDirs::new().map(|b| b.config_dir().to_path_buf())
        {
            let path = config_home.join("ram-sentinel.toml");
            if path.exists() {
                return Self::parse_file(&path);
            }
        }

        logging::emit(&SentinelEvent::Message {
            level: LogLevel::Info,
            text: "No configuration file found. Loading sane defaults.",
        });
        Ok(Self::sane_defaults())
    }

    fn parse_file(path: &Path) -> Result<Config, ConfigError> {
        let content =
            fs::read_to_string(path).map_err(|e| ConfigError::FileRead(path.to_path_buf(), e))?;

        toml::from_str(&content)
            .map_err(|e| ConfigError::FileParse(path.to_path_buf(), e.to_string()))
    }

    pub fn sane_defaults() -> Config {
        Config {
            psi: Some(psi::PsiConfig {
                warn_max_percent: None,
                kill_max_percent: None,
                amount_to_free: None,
                check_interval_ms: None,
            }),
            ram: Some(MemoryConfig {
                warn_min_free_bytes: None,
                warn_min_free_percent: Some(10.0),
                kill_min_free_bytes: None,
                kill_min_free_percent: Some(5.0),
            }),
            swap: Some(MemoryConfig {
                warn_min_free_bytes: None,
                warn_min_free_percent: None,
                kill_min_free_bytes: None,
                kill_min_free_percent: None,
            }),
            zram: Some(MemoryConfig {
                warn_min_free_bytes: None,
                warn_min_free_percent: None,
                kill_min_free_bytes: None,
                kill_min_free_percent: None,
            }),
            check_interval_ms: default_interval(),
            warn_reset_ms: warn_interval(),
            sigterm_wait_ms: sigterm_wait_ms(),
            ignore_targets: default_ignore_targets(),
            kill_targets: default_kill_targets(),
            kill_strategy: default_strategy(),
            allow_kill_outside_targets: default_allow_kill_outside_targets(),
        }
    }

    fn validate(&mut self) -> Result<(), ConfigError> {
        let psi_empty = self.psi.as_ref().map_or(true, |p| p.is_effectively_empty());
        let ram_empty = self.ram.as_ref().map_or(true, |r| r.is_effectively_empty());
        let swap_empty = self
            .swap
            .as_ref()
            .map_or(true, |s| s.is_effectively_empty());
        let zram_empty = self
            .zram
            .as_ref()
            .map_or(true, |z| z.is_effectively_empty());

        if psi_empty && ram_empty && swap_empty && zram_empty {
            return Err(ConfigError::EffectiveEmpty);
        }

        // Detailed Validation
        if let Some(r) = self.ram.as_ref() {
            MemoryConfigParsed::try_from_config(r.clone())?;
        }

        if let Some(s) = self.swap.as_ref() {
            MemoryConfigParsed::try_from_config(s.clone())?;
        }

        if let Some(z) = self.zram.as_ref() {
            MemoryConfigParsed::try_from_config(z.clone())?;
        }

        if let Some(p) = self.psi.as_ref() {
            psi::PsiConfigParsed::try_from_config(p.clone(), self.check_interval_ms)
                .map_err(|e| ConfigError::PsiConfig(e.to_string()))?;
        }

        // Validate Patterns
        compile_patterns(&self.ignore_targets, "ignore_targets")?;
        compile_patterns(&self.kill_targets, "kill_targets")?;

        if psi_empty {
            self.psi = None;
        }
        if ram_empty {
            self.ram = None;
        }
        if swap_empty {
            self.swap = None;
        }
        if zram_empty {
            self.zram = None;
        }

        if self.check_interval_ms > 300000 {
            return Err(ConfigError::IntervalTooHigh(self.check_interval_ms));
        }

        if self.check_interval_ms < 100 {
            return Err(ConfigError::IntervalTooLow(self.check_interval_ms));
        }

        Ok(())
    }
}

fn compile_patterns(raw: &[String], field_name: &str) -> Result<Vec<TargetPattern>, ConfigError> {
    let mut patterns = Vec::new();
    for (i, raw_str) in raw.iter().enumerate() {
        if raw_str.starts_with("@[") {
            if let Some(end_bracket) = raw_str.find("]@ ") {
                let cgroup_part = &raw_str[2..end_bracket];
                let cmdline_part = &raw_str[end_bracket + 3..];

                let cgroup_pat = parse_single_pattern(cgroup_part, field_name, i, raw_str)?;
                let cmdline_pat = parse_single_pattern(cmdline_part, field_name, i, raw_str)?;

                patterns.push(TargetPattern {
                    cgroup: Some(cgroup_pat),
                    cmdline: cmdline_pat,
                });
                continue;
            } else {
                return Err(ConfigError::FileParse(
                    PathBuf::from("config"),
                    format!(
                        "Invalid pattern in {}: missing mandatory closing ']@ ' in '{}'",
                        field_name, raw_str
                    ),
                ));
            }
        }

        // Fallback or explicit cmdline-only
        let cmdline_pat = parse_single_pattern(raw_str, field_name, i, raw_str)?;
        patterns.push(TargetPattern {
            cgroup: None,
            cmdline: cmdline_pat,
        });
    }
    Ok(patterns)
}

fn parse_single_pattern(
    s: &str,
    field_name: &str,
    index: usize,
    raw_full: &str,
) -> Result<Pattern, ConfigError> {
    if s.starts_with('/') && s.ends_with('/') && s.len() > 2 {
        let regex_str = &s[1..s.len() - 1];
        match Regex::new(regex_str) {
            Ok(re) => Ok(Pattern::Regex(re)),
            Err(e) => Err(ConfigError::RegexError(
                field_name.to_string(),
                index,
                raw_full.to_string(),
                e.to_string(),
            )),
        }
    } else if s.starts_with('^') && s.len() > 1 {
        Ok(Pattern::StartsWith(s[1..].to_string()))
    } else {
        Ok(Pattern::Literal(s.to_string()))
    }
}
