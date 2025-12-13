use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::exit;
use regex::Regex;
use log::info;
use crate::psi;
use crate::utils::parse_size;

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
    pub fn from_config(config: MemoryConfig) -> Self {
        let warn_min_free_bytes = config.warn_min_free_bytes.as_ref().map(|s| {
            parse_size(s).unwrap_or_else(|| {
                eprintln!("Error: Invalid size string in warnMinFreeBytes: '{}'", s);
                exit(10);
            })
        });

        let kill_min_free_bytes = config.kill_min_free_bytes.as_ref().map(|s| {
            parse_size(s).unwrap_or_else(|| {
                eprintln!("Error: Invalid size string in killMinFreeBytes: '{}'", s);
                exit(10);
            })
        });

        Self {
            warn_min_free_bytes,
            warn_min_free_percent: config.warn_min_free_percent,
            kill_min_free_bytes,
            kill_min_free_percent: config.kill_min_free_percent,
        }
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
}

impl Pattern {
    pub fn matches(&self, s: &str) -> bool {
        match self {
            Pattern::Literal(lit) => s.contains(lit),
            Pattern::Regex(re) => re.is_match(s),
        }
    }
}

impl Config {
    pub fn load(cli_config_path: Option<PathBuf>) -> RuntimeContext {
        let config = match cli_config_path {
            Some(path) => {
                if !path.exists() {
                    // Exit code 2: Error reading config file
                    eprintln!("Error: Config file specified but not found: {:?}", path);
                    exit(2);
                }
                Self::parse_file(&path)
            }
            None => Self::find_and_load_config(),
        };

        config.validate();

        // Optimization: Compile Regex patterns
        let ignore_names_regex = compile_patterns(&config.ignore_names, "ignore_names");
        let kill_targets_regex = compile_patterns(&config.kill_targets, "kill_targets");

        let psi_parsed = if let Some(p) = config.psi {
            let parsed = psi::PsiConfigParsed::try_from_config(p, config.check_interval_ms).unwrap_or_else(|e| {
                eprintln!("Error: {}", e);
                exit(7);
            });

            // Exit Code 8: PSI availability
            if let Err(e) = psi::validate_psi_availability() {
                eprintln!("Error: PSI enabled but /proc/pressure/memory is not valid: {}", e);
                exit(8);
            }
            Some(parsed)
        } else {
            None
        };

        RuntimeContext {
            psi: psi_parsed,
            ram: config.ram.map(MemoryConfigParsed::from_config),
            swap: config.swap.map(MemoryConfigParsed::from_config),
            check_interval_ms: config.check_interval_ms,
            warn_reset_ms: config.warn_reset_ms,
            sigterm_wait_ms: config.sigterm_wait_ms,
            kill_strategy: config.kill_strategy,
            ignore_names_regex,
            kill_targets_regex,
        }
    }

    fn find_and_load_config() -> Config {
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
        Self::sane_defaults()
    }

    fn parse_file(path: &Path) -> Config {
        let content = fs::read_to_string(path).unwrap_or_else(|e| {
            // Exit code 2: Error reading config file
            eprintln!("Error reading config file {:?}: {}", path, e);
            exit(2);
        });

        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("yaml");

        macro_rules! parse_err {
            ($r:expr) => {
                $r.map_err(|e| {
                    // Exit code 3: Error parsing config file
                    eprintln!("Error parsing config file {:?}: {}", path, e);
                    exit(3);
                })
            };
        }

        match ext {
            "yaml" | "yml" => parse_err!(serde_yaml::from_str(&content)).unwrap(),
            "json" => parse_err!(serde_json::from_str(&content)).unwrap(),
            "toml" => parse_err!(toml::from_str(&content)).unwrap(),
            _ => parse_err!(serde_yaml::from_str(&content)).unwrap(),
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

    fn validate(&self) {
        let psi_empty = self.psi.as_ref().map_or(true, |p| p.is_effectively_empty());
        let ram_empty = self.ram.as_ref().map_or(true, |r| r.is_effectively_empty());
        let swap_empty = self.swap.as_ref().map_or(true, |s| s.is_effectively_empty());
        
        // Exit Code 4: Effectively empty
        if psi_empty && ram_empty && swap_empty {
            eprintln!("Error: Configuration is effectively empty (no metrics enabled).");
            exit(4);
        }

        // Exit Code 5: Interval too high
        if self.check_interval_ms > 300000 {
            eprintln!("Error: check_interval_ms > 300000.");
            exit(5);
        }
        // Exit Code 6: Interval too low
        if self.check_interval_ms < 100 {
            eprintln!("Error: check_interval_ms < 100.");
            exit(6);
        }
    }}

fn compile_patterns(raw: &[String], field_name: &str) -> Vec<Pattern> {
    raw.iter().enumerate().map(|(i, s)| {
        if s.starts_with('/') && s.ends_with('/') && s.len() > 2 {
            let regex_str = &s[1..s.len()-1];
            match Regex::new(regex_str) {
                Ok(re) => Pattern::Regex(re),
                Err(e) => {
                    // Exit Code 9: Invalid regex pattern
                    eprintln!(
                        "Error: Invalid regex in {}: entry {} ('{}'): {}",
                        field_name, i, s, e
                    );
                    exit(9);
                }
            }
        } else {
            Pattern::Literal(s.clone())
        }
    }).collect()
}

