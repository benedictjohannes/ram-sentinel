use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use regex::Regex;
use log::info;
use crate::psi;
use crate::utils::parse_size;
use crate::config_error::ConfigError;

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    // Metric Triggers
    pub psi: Option<psi::PsiConfig>,
    pub ram: Option<MemoryConfig>,
    pub swap: Option<MemoryConfig>,

    // Operational Settings
    #[serde(default = "default_interval")]
    pub check_interval_ms: u64,
    #[serde(default = "warn_interval")]
    pub warn_reset_ms: u64,
    #[serde(default = "sigterm_wait_ms")]
    pub sigterm_wait_ms: u64,

    // Targeting Logic
    #[serde(default)]
    pub ignore_names: Vec<String>,
    
    #[serde(default = "default_kill_targets")] 
    pub kill_targets: Vec<String>,
    
    #[serde(default = "default_strategy")]
    pub kill_strategy: KillStrategy,
}


#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
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
                ConfigError::InvalidSize("warnMinFreeBytes".to_string(), s.clone())
            })?)
        } else {
            None
        };

        let kill_min_free_bytes = if let Some(s) = config.kill_min_free_bytes.as_ref() {
             Some(parse_size(s).ok_or_else(|| {
                ConfigError::InvalidSize("killMinFreeBytes".to_string(), s.clone())
            })?)
        } else {
            None
        };

        if let Some(p) = config.warn_min_free_percent {
            if !(0.0..=100.0).contains(&p) {
                return Err(ConfigError::InvalidPercent("warnMinFreePercent".to_string(), p));
            }
        }

        if let Some(p) = config.kill_min_free_percent {
            if !(0.0..=100.0).contains(&p) {
                return Err(ConfigError::InvalidPercent("killMinFreePercent".to_string(), p));
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
#[serde(rename_all = "camelCase")]
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
fn default_interval() -> u64 { 1000 }
fn warn_interval() -> u64 { 30000 }
fn sigterm_wait_ms() -> u64 { 5000 }
fn default_strategy() -> KillStrategy { KillStrategy::HighestOomScore }
fn default_kill_targets() -> Vec<String> {
    vec![
        "type=renderer".to_string(),
        "-contentproc".to_string()
    ]
}

#[derive(Debug)]
pub struct RuntimeContext {
    pub psi: Option<psi::PsiConfigParsed>,
    pub ram: Option<MemoryConfigParsed>,
    pub swap: Option<MemoryConfigParsed>,

    pub check_interval_ms: u64,
    pub warn_reset_ms: u64,
    pub sigterm_wait_ms: u64,

    pub kill_strategy: KillStrategy,

    pub ignore_names_regex: Vec<Pattern>,
    pub kill_targets_regex: Vec<Pattern>,
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
        let config = match cli_config_path {
            Some(path) => {
                if !path.exists() {
                     // Was Exit code 2
                    return Err(ConfigError::ConfigFileNotFound(path));
                }
                Self::parse_file(&path)?
            }
            None => Self::find_and_load_config()?,
        };

        config.validate()?;

        // Optimization: Compile Regex patterns
        let ignore_names_regex = compile_patterns(&config.ignore_names, "ignore_names")?;
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

        Ok(RuntimeContext {
            psi: psi_parsed,
            ram: ram_parsed,
            swap: swap_parsed,
            check_interval_ms: config.check_interval_ms,
            warn_reset_ms: config.warn_reset_ms,
            sigterm_wait_ms: config.sigterm_wait_ms,
            kill_strategy: config.kill_strategy,
            ignore_names_regex,
            kill_targets_regex,
        })
    }

    fn find_and_load_config() -> Result<Config, ConfigError> {
        if let Some(config_home) = directories::BaseDirs::new().map(|b| b.config_dir().to_path_buf()) {
             let extensions = ["yaml", "yml", "json", "toml"];
             for ext in &extensions {
                let path = config_home.join(format!("ram-sentinel.{}", ext));
                if path.exists() {
                     return Self::parse_file(&path);
                }
             }
        }

        info!("No configuration file found. Loading sane defaults.");
        Ok(Self::sane_defaults())
    }

    fn parse_file(path: &Path) -> Result<Config, ConfigError> {
        let content = fs::read_to_string(path)
            .map_err(|e| ConfigError::FileRead(path.to_path_buf(), e))?;

        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("yaml");

        macro_rules! parse {
            ($func:expr) => {
                $func(&content).map_err(|e| ConfigError::FileParse(path.to_path_buf(), e.to_string()))
            };
        }

        match ext {
            "yaml" | "yml" => parse!(serde_yaml::from_str),
            "json" => parse!(serde_json::from_str),
            "toml" => parse!(toml::from_str),
            _ => parse!(serde_yaml::from_str),
        }
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
            check_interval_ms: default_interval(),
            warn_reset_ms: warn_interval(),
            sigterm_wait_ms: sigterm_wait_ms(),
            ignore_names: vec![],
            kill_targets: default_kill_targets(),
            kill_strategy: default_strategy(),
        }
    }

    fn validate(&self) -> Result<(), ConfigError> {
        let psi_empty = self.psi.as_ref().map_or(true, |p| p.is_effectively_empty());
        let ram_empty = self.ram.as_ref().map_or(true, |r| r.is_effectively_empty());
        let swap_empty = self.swap.as_ref().map_or(true, |s| s.is_effectively_empty());
        
        if psi_empty && ram_empty && swap_empty {
            return Err(ConfigError::EffectiveEmpty);
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

fn compile_patterns(raw: &[String], field_name: &str) -> Result<Vec<Pattern>, ConfigError> {
    let mut patterns = Vec::new();
    for (i, s) in raw.iter().enumerate() {
        if s.starts_with('/') && s.ends_with('/') && s.len() > 2 {
            // Case 1: Regex
            let regex_str = &s[1..s.len()-1];
            match Regex::new(regex_str) {
                Ok(re) => patterns.push(Pattern::Regex(re)),
                Err(e) => {
                    return Err(ConfigError::RegexError(
                        field_name.to_string(), 
                        i, 
                        s.clone(), 
                        e.to_string()
                    ));
                }
            }
        } else if s.starts_with('^') && s.len() > 1 {
            // Case 2: StartsWith
            patterns.push(Pattern::StartsWith(s[1..].to_string()));
        } else {
            // Case 3: Literal
            patterns.push(Pattern::Literal(s.clone()));
        }
    }
    Ok(patterns)
}
