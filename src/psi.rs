use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::num::ParseIntError;

#[derive(Debug)]
pub enum PsiError {
    Io(io::Error),
    FieldNotFound,
    Parse(ParseIntError),
}

impl fmt::Display for PsiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PsiError::Io(e) => write!(f, "Filesystem access error: {}", e),
            PsiError::FieldNotFound => write!(f, "PSI field 'some total=' was not found."),
            PsiError::Parse(e) => write!(f, "Value parsing error: {}", e),
        }
    }
}

impl Error for PsiError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            PsiError::Io(e) => Some(e),
            PsiError::Parse(e) => Some(e),
            PsiError::FieldNotFound => None,
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
            if let Some(pos) = line.find("total=") {
                let val_str = &line[pos + 6..];
                return Ok(val_str.trim().parse::<u64>()?);
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
pub struct PsiConfigParsed {
    pub warn_max_percent: Option<f32>,
    pub kill_max_percent: Option<f32>,
    pub amount_to_free: Option<u64>,
}
