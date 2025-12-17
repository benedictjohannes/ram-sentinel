use byte_unit::Byte;
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::fmt;

// --- Enums ---

#[derive(Debug, Copy, Clone, PartialEq, PartialOrd, Serialize, Deserialize, ValueEnum)]
#[repr(u8)]
pub enum LogLevel {
    Error = 1,
    Warn = 2,
    Info = 3,
    Debug = 4,
}

impl LogLevel {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => LogLevel::Error,
            2 => LogLevel::Warn,
            3 => LogLevel::Info,
            4 => LogLevel::Debug,
            _ => LogLevel::Info, // Safe fallback
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            LogLevel::Error => "ERROR",
            LogLevel::Warn => "WARN",
            LogLevel::Info => "INFO",
            LogLevel::Debug => "DEBUG",
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, ValueEnum)]
#[repr(u8)]
pub enum LogMode {
    Compact = 0,
    Json = 1,
}

impl LogMode {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => LogMode::Json,
            _ => LogMode::Compact,
        }
    }
}

// --- Event Definition ---

#[derive(Serialize, Clone)]
#[serde(tag = "message", rename_all = "snake_case")]
pub enum SentinelEvent {
    // Generic Message Wrapper
    Message {
        #[serde(skip)] // We don't need "level" twice in JSON (it's in the root)
        level: LogLevel,
        text: String,
    },

    Startup {
        interval_ms: u64,
    },
    Monitor {
        memory_available_bytes: Option<u64>,
        memory_available_percent: Option<f64>,
        swap_free_bytes: Option<u64>,
        swap_free_percent: Option<f64>,
        psi_pressure: Option<f64>,
    },
    LowMemoryWarn {
        available_bytes: u64,
        available_percent: f64,
        threshold_type: String,
        threshold_value: f64,
    },
    LowSwapWarn {
        free_bytes: u64,
        free_percent: f64,
        threshold_type: String,
        threshold_value: f64,
    },
    PsiPressureWarn {
        pressure_curr: f64,
        threshold: f64,
    },
    KillTriggered {
        trigger: String,
        observed_value: f64,
        threshold_value: f64,
        threshold_type: String,
        amount_needed: Option<u64>,
    },
    KillCandidateSelected {
        pid: u32,
        process_name: String,
        score: u64,
        rss: u64,
        match_index: usize,
    },
    KillExecuted {
        pid: u32,
        process_name: String,
        strategy: String,
        rss_freed: u64,
    },
    KillSequenceAborted {
        reason: String,
    },
    KillCandidateIgnored {
        pid: u32,
        reason: String,
    },
}

// --- Display Implementation (for Compact Mode) ---
impl fmt::Display for SentinelEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SentinelEvent::Message { text, .. } => write!(f, "{}", text),

            SentinelEvent::Startup { interval_ms } => {
                write!(f, "ram-sentinel started. Interval: {}ms", interval_ms)
            }
            SentinelEvent::Monitor {
                memory_available_bytes,
                memory_available_percent: _,
                swap_free_bytes,
                swap_free_percent: _,
                psi_pressure,
            } => {
                let avail_str = match memory_available_bytes {
                    Some(b) => Byte::from_u64(*b)
                        .get_appropriate_unit(byte_unit::UnitType::Decimal)
                        .to_string(),
                    None => "N/A".to_string(),
                };

                let swap_str = match swap_free_bytes {
                    Some(b) => Byte::from_u64(*b)
                        .get_appropriate_unit(byte_unit::UnitType::Decimal)
                        .to_string(),
                    None => "N/A".to_string(),
                };

                let psi_str = match psi_pressure {
                    Some(p) => format!("{:.2}", p),
                    None => "N/A".to_string(),
                };

                write!(
                    f,
                    "Memory: {} available, Swap: {} available, PSI: {}",
                    avail_str, swap_str, psi_str
                )
            }
            SentinelEvent::LowMemoryWarn {
                available_bytes,
                available_percent,
                threshold_type,
                threshold_value,
            } => {
                let avail_str = Byte::from_u64(*available_bytes)
                    .get_appropriate_unit(byte_unit::UnitType::Decimal)
                    .to_string();
                if threshold_type == "bytes" {
                    let thresh_str = Byte::from_u64(*threshold_value as u64)
                        .get_appropriate_unit(byte_unit::UnitType::Decimal)
                        .to_string();
                    write!(
                        f,
                        "Low RAM: {} available (Limit: {})",
                        avail_str, thresh_str
                    )
                } else {
                    write!(
                        f,
                        "Low RAM: {} ({:.2}%) available (Limit: {:.2}%)",
                        avail_str, available_percent, threshold_value
                    )
                }
            }
            SentinelEvent::LowSwapWarn {
                free_bytes,
                free_percent,
                threshold_type,
                threshold_value,
            } => {
                let free_str = Byte::from_u64(*free_bytes)
                    .get_appropriate_unit(byte_unit::UnitType::Decimal)
                    .to_string();
                if threshold_type == "bytes" {
                    let thresh_str = Byte::from_u64(*threshold_value as u64)
                        .get_appropriate_unit(byte_unit::UnitType::Decimal)
                        .to_string();
                    write!(
                        f,
                        "Low Swap: {} available (Limit: {})",
                        free_str, thresh_str
                    )
                } else {
                    write!(
                        f,
                        "Low Swap: {} ({:.2}%) available (Limit: {:.2}%)",
                        free_str, free_percent, threshold_value
                    )
                }
            }
            SentinelEvent::PsiPressureWarn {
                pressure_curr,
                threshold,
            } => {
                write!(
                    f,
                    "Memory Pressure: {:.2}% (Limit: {:.2}%)",
                    pressure_curr, threshold
                )
            }
            SentinelEvent::KillTriggered {
                trigger,
                observed_value,
                threshold_value,
                threshold_type,
                ..
            } => {
                let observed_str = if threshold_type == "bytes" {
                    Byte::from_u64(*observed_value as u64)
                        .get_appropriate_unit(byte_unit::UnitType::Decimal)
                        .to_string()
                } else {
                    format!("{:.2}%", observed_value)
                };
                let limit_str = if threshold_type == "bytes" {
                    Byte::from_u64(*threshold_value as u64)
                        .get_appropriate_unit(byte_unit::UnitType::Decimal)
                        .to_string()
                } else {
                    format!("{:.2}%", threshold_value)
                };
                write!(
                    f,
                    "Kill Triggered: {} - Observed {} < Limit {}",
                    trigger, observed_str, limit_str
                )
            }
            SentinelEvent::KillCandidateSelected {
                process_name,
                pid,
                score,
                rss,
                ..
            } => {
                let rss_str = Byte::from_u64(*rss)
                    .get_appropriate_unit(byte_unit::UnitType::Decimal)
                    .to_string();
                write!(
                    f,
                    "Selected Target: {} (PID {}). Score: {}, RSS: {}",
                    process_name, pid, score, rss_str
                )
            }
            SentinelEvent::KillExecuted {
                process_name,
                pid,
                strategy,
                rss_freed,
            } => {
                let rss_str = Byte::from_u64(*rss_freed)
                    .get_appropriate_unit(byte_unit::UnitType::Decimal)
                    .to_string();
                write!(
                    f,
                    "{} {} (PID {}). Freed: {}",
                    strategy, process_name, pid, rss_str
                )
            }
            SentinelEvent::KillSequenceAborted { reason } => {
                write!(f, "Kill Sequence Aborted: {}", reason)
            }
            SentinelEvent::KillCandidateIgnored { pid, reason } => {
                write!(f, "Ignored Candidate PID {}: {}", pid, reason)
            }
        }
    }
}

impl SentinelEvent {
    /// Determines the log severity of the current event.
    pub fn severity(&self) -> LogLevel {
        match self {
            // For the generic message, we simply return the contained level
            SentinelEvent::Message { level, .. } => *level,
            SentinelEvent::Monitor { .. } => LogLevel::Debug,

            SentinelEvent::Startup { .. }
            | SentinelEvent::KillCandidateSelected { .. }
            | SentinelEvent::KillExecuted { .. }
            | SentinelEvent::KillSequenceAborted { .. }
            | SentinelEvent::KillCandidateIgnored { .. } => LogLevel::Info,

            SentinelEvent::LowMemoryWarn { .. }
            | SentinelEvent::LowSwapWarn { .. }
            | SentinelEvent::PsiPressureWarn { .. } => LogLevel::Warn,

            SentinelEvent::KillTriggered { .. } => LogLevel::Error,
        }
    }
}
