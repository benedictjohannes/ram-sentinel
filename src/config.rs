use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::exit;
use directories::ProjectDirs;
use regex::Regex;
use log::info;

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    // Metric Triggers
    pub psi: Option<PsiConfig>,
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
pub struct PsiConfig {
    pub warn_max_percent: Option<f32>,
    pub kill_max_percent: Option<f32>,
    pub amount_to_free: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MemoryConfig {
    pub warn_min_free_bytes: Option<String>,
    pub warn_min_free_percent: Option<f32>,
    pub kill_min_free_bytes: Option<String>,
    pub kill_min_free_percent: Option<f32>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum KillStrategy {
    LargestRss,
    HighestOomScore,
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

pub struct RuntimeContext {
    pub config: Config,
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
        let (config, path_loaded) = match cli_config_path {
            Some(path) => {
                if !path.exists() {
                    eprintln!("Error: Config file specified but not found: {:?}", path);
                    exit(2);
                }
                (Self::parse_file(&path), Some(path))
            }
            None => Self::find_and_load_config(),
        };

        config.validate(path_loaded.as_deref());

        // Optimization: Compile Regex patterns
        let ignore_names_regex = compile_patterns(&config.ignore_names);
        let kill_targets_regex = compile_patterns(&config.kill_targets);

        RuntimeContext {
            config,
            ignore_names_regex,
            kill_targets_regex,
        }
    }

    fn find_and_load_config() -> (Config, Option<PathBuf>) {
        if let Some(proj_dirs) = ProjectDirs::from("", "", "ram-sentinel") {
            let config_dir = proj_dirs.config_dir();
            let extensions = ["yaml", "yml", "json", "toml"];
            
            for ext in &extensions {
                let path = config_dir.join(format!("config.{}", ext));
                if path.exists() {
                    return (Self::parse_file(&path), Some(path));
                }
            }
        }
        
        if let Some(config_home) = directories::BaseDirs::new().map(|b| b.config_dir().to_path_buf()) {
             let extensions = ["yaml", "yml", "json", "toml"];
             for ext in &extensions {
                let path = config_home.join(format!("ram-sentinel.{}", ext));
                if path.exists() {
                     return (Self::parse_file(&path), Some(path));
                }
             }
        }

        info!("No configuration file found. Loading sane defaults.");
        (Self::sane_defaults(), None)
    }

    fn parse_file(path: &Path) -> Config {
        let content = fs::read_to_string(path).unwrap_or_else(|e| {
            eprintln!("Error reading config file {:?}: {}", path, e);
            exit(2);
        });

        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("yaml");
        let config: Config = match ext {
            "yaml" | "yml" => serde_yaml::from_str(&content).expect("Failed to parse YAML config"),
            "json" => serde_json::from_str(&content).expect("Failed to parse JSON config"),
            "toml" => toml::from_str(&content).expect("Failed to parse TOML config"),
            _ => serde_yaml::from_str(&content).expect("Failed to parse YAML config"), // Default to yaml
        };

        config
    }

    pub fn sane_defaults() -> Config {
        Config {
            psi: Some(PsiConfig {
                warn_max_percent: None,
                kill_max_percent: None,
                amount_to_free: None,
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

    fn validate(&self, _path: Option<&Path>) {
        // Exit Code 4: Effectively empty
        if self.psi.is_none() && self.ram.is_none() && self.swap.is_none() {
            eprintln!("Error: Configuration is effectively empty (no metrics enabled).");
            exit(4);
        }

        // Exit Code 5: PSI logical error
        if let Some(psi) = &self.psi {
            if psi.kill_max_percent.is_some() && psi.amount_to_free.is_none() {
                eprintln!("Error: PSI kill_max_percent set but amount_to_free is missing.");
                exit(5);
            }
        }

        // Exit Code 6: Interval too high
        if self.check_interval_ms > 300000 {
            eprintln!("Error: check_interval_ms > 300000.");
            exit(6);
        }

        // Exit Code 7: PSI availability
        if self.psi.is_some() {
             let psi_path = Path::new("/proc/pressure/memory");
             if !psi_path.exists() {
                 eprintln!("Error: PSI enabled but /proc/pressure/memory not available.");
                 exit(7);
             }
             if fs::read_to_string(psi_path).is_err() {
                 eprintln!("Error: PSI enabled but cannot read /proc/pressure/memory.");
                 exit(7);
             }
        }
    }
}

fn compile_patterns(raw: &[String]) -> Vec<Pattern> {
    raw.iter().map(|s| {
        if s.starts_with('/') && s.ends_with('/') && s.len() > 2 {
            let regex_str = &s[1..s.len()-1];
            match Regex::new(regex_str) {
                Ok(re) => Pattern::Regex(re),
                Err(e) => {
                    eprintln!("Warning: Invalid regex '{}': {}", s, e);
                    Pattern::Literal(s.clone())
                }
            }
        } else {
            Pattern::Literal(s.clone())
        }
    }).collect()
}
