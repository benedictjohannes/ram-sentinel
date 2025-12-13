use std::fmt;
use std::io;
use std::path::PathBuf;

#[derive(Debug)]
pub enum ConfigError {
    FileRead(PathBuf, io::Error),
    FileParse(PathBuf, String),
    ConfigFileNotFound(PathBuf),
    EffectiveEmpty,
    IntervalTooHigh(u64),
    IntervalTooLow(u64),
    PsiConfig(String),
    PsiUnavailable(String),
    RegexError(String, usize, String, String), // field_name, index, pattern, error
    InvalidSize(String, String), // field_name, value
    InvalidPercent(String, f32), // field_name, value
}

impl ConfigError {
    pub fn exit_code(&self) -> i32 {
        match self {
            ConfigError::FileRead(..) => 2,
            ConfigError::ConfigFileNotFound(_) => 2,
            ConfigError::FileParse(..) => 3,
            ConfigError::EffectiveEmpty => 4,
            ConfigError::IntervalTooHigh(_) => 5,
            ConfigError::IntervalTooLow(_) => 6,
            ConfigError::PsiConfig(_) => 7,
            ConfigError::PsiUnavailable(_) => 8,
            ConfigError::RegexError(..) => 9,
            ConfigError::InvalidSize(..) => 10,
            ConfigError::InvalidPercent(..) => 11,
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::FileRead(path, e) => write!(f, "Error reading config file {:?}: {}", path, e),
            ConfigError::FileParse(path, e) => write!(f, "Error parsing config file {:?}: {}", path, e),
            ConfigError::ConfigFileNotFound(path) => write!(f, "Error: Config file specified but not found: {:?}", path),
            ConfigError::EffectiveEmpty => write!(f, "Configuration is effectively empty (no metrics enabled)."),
            ConfigError::IntervalTooHigh(val) => write!(f, "check_interval_ms > 300000. Got: {}", val),
            ConfigError::IntervalTooLow(val) => write!(f, "check_interval_ms < 100. Got: {}", val),
            ConfigError::PsiConfig(e) => write!(f, "PSI Configuration Error: {}", e),
            ConfigError::PsiUnavailable(e) => write!(f, "PSI enabled but /proc/pressure/memory is not valid: {}", e),
            ConfigError::RegexError(field, idx, pat, err) => write!(f, "Invalid regex in {}: entry {} ('{}'): {}", field, idx, pat, err),
            ConfigError::InvalidSize(field, val) => write!(f, "Invalid size string in {}: '{}'", field, val),
            ConfigError::InvalidPercent(field, val) => write!(f, "{} must be between 0-100, got {}", field, val),
        }
    }
}

