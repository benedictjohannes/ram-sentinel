use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::num::ParseIntError;
use crate::utils::parse_size;

#[derive(Debug)]
pub enum PsiError {
    Io(io::Error),
    FieldNotFound,
    Parse(ParseIntError),
    ValidationError(String), // New variant for validation errors
}

impl fmt::Display for PsiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PsiError::Io(e) => write!(f, "Filesystem access error: {}", e),
            PsiError::FieldNotFound => write!(f, "PSI field 'some total=' was not found."),
            PsiError::Parse(e) => write!(f, "Value parsing error: {}", e),
            PsiError::ValidationError(msg) => write!(f, "Configuration validation error: {}", msg),
        }
    }
}

impl Error for PsiError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            PsiError::Io(e) => Some(e),
            PsiError::Parse(e) => Some(e),
            PsiError::FieldNotFound => None,
            PsiError::ValidationError(_) => None,
        }
    }
}

impl From<io::Error> for PsiError {
    fn from(err: io::Error) -> PsiError {
        PsiError::Io(err)
    }
}

impl From<ParseIntError> for PsiError {
    fn from(err: ParseIntError) -> PsiError {
        PsiError::Parse(err)
    }
}

pub fn read_psi_total() -> Result<u64, PsiError> {
    let content = fs::read_to_string("/proc/pressure/memory")?;

    for line in content.lines() {
        if line.starts_with("some") {
            for part in line.split_whitespace() {
                if let Some(val_str) = part.strip_prefix("total=") {
                    return Ok(val_str.parse::<u64>()?);
                }
            }
        }
    }
    Err(PsiError::FieldNotFound)
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PsiConfig {
    pub warn_max_percent: Option<f32>,
    pub kill_max_percent: Option<f32>,
    pub amount_to_free: Option<String>,
}
impl PsiConfig {
    pub fn is_effectively_empty(&self) -> bool {
        self.warn_max_percent.is_none() && self.kill_max_percent.is_none()
    }
}
#[derive(Debug, Clone)]
pub struct PsiConfigParsed {
    pub warn_max_percent: Option<f32>,
    pub kill_max_percent: Option<f32>,
    pub amount_to_free: Option<u64>,
}

impl PsiConfigParsed {
    pub fn try_from_config(config: PsiConfig) -> Result<Self, PsiError> {
        if config.kill_max_percent.is_some() && config.amount_to_free.is_none() {
            return Err(PsiError::ValidationError("PSI kill_max_percent set but amount_to_free is missing.".to_string()));
        }
        
        let amount_to_free = if let Some(amt_str) = config.amount_to_free {
            let parsed_amt = parse_size(&amt_str).ok_or_else(|| {
                PsiError::ValidationError(format!("PSI amount_to_free invalid format: {}", amt_str))
            })?;

            if parsed_amt == 0 {
                return Err(PsiError::ValidationError("PSI amount_to_free is illegal (parses to 0).".to_string()));
            }
            Some(parsed_amt)
        } else {
            None
        };

        Ok(Self {
            warn_max_percent: config.warn_max_percent,
            kill_max_percent: config.kill_max_percent,
            amount_to_free: amount_to_free,
        })
    }
}

pub fn validate_psi_availability() -> Result<(), PsiError> {
    read_psi_total().map(|_| ()) // Just check if it can be read, discard value
}
