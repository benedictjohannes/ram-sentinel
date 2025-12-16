use byte_unit::Byte;
use chrono::Utc;
use clap::ValueEnum;
use notify_rust::Notification;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fmt;
use std::sync::atomic::{AtomicU8, Ordering};

// --- Module Level State ---

// Atomic storage for thread-safe, lock-free access to configuration
static CURRENT_LOG_LEVEL: AtomicU8 = AtomicU8::new(3); // Default: INFO (3)
static CURRENT_LOG_MODE: AtomicU8 = AtomicU8::new(0); // Default: Compact (0)

pub fn set_logging_level(l: LogLevel) {
    CURRENT_LOG_LEVEL.store(l as u8, Ordering::Relaxed);
}

pub fn set_logging_mode(m: LogMode) {
    CURRENT_LOG_MODE.store(m as u8, Ordering::Relaxed);
}

pub fn get_log_level() -> LogLevel {
    LogLevel::from_u8(CURRENT_LOG_LEVEL.load(Ordering::Relaxed))
}

fn get_log_mode() -> LogMode {
    LogMode::from_u8(CURRENT_LOG_MODE.load(Ordering::Relaxed))
}

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
    fn from_u8(v: u8) -> Self {
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
    fn from_u8(v: u8) -> Self {
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
        psi_pressure_curr: Option<f64>,
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
                psi_pressure_curr,
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

                let psi_str = match psi_pressure_curr {
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

// --- Logic Implementation ---

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

    /// Primary entry point for logging/notifying.
    pub fn emit(&self) {
        // 1. Check Global Log Level (Filtering)
        // If event severity (e.g., Info=3) is greater than Configured Level (e.g., Warn=2), skip.
        if self.severity() > get_log_level() {
            return;
        }

        // 2. Desktop Notification (if applicable)
        self.emit_notification();

        // 3. Output to Stdout
        match get_log_mode() {
            LogMode::Json => self.log_json(),
            LogMode::Compact => self.log_compact(),
        }
    }

    fn log_compact(&self) {
        // Format: YYYY-MM-DDTHH:MM:SSZ [LEVEL] Message...
        println!(
            "{} [{}] {}",
            Utc::now().to_rfc3339(),
            self.severity().as_str(),
            self
        );
    }

    fn log_json(&self) {
        // We use serde_json::to_value to get the fields of the event
        let mut log_entry = serde_json::to_value(self).unwrap_or(json!({
            "event": "SerializationError"
        }));

        // Flatten metadata into the root object
        if let Some(map) = log_entry.as_object_mut() {
            map.insert("timestamp".into(), Utc::now().to_rfc3339().into());
            map.insert("level".into(), self.severity().as_str().into());
            match self {
                SentinelEvent::Message { level: _, text } => {
                    map.insert("message".into(), text.as_str().into());
                    map.remove("text");
                }
                _ => {}
            }
        }

        // Write to stdout
        println!("{}", serde_json::to_string(&log_entry).unwrap());
    }

    fn emit_notification(&self) {
        // Only notify on actual issues or actions, not just Info logs
        match self {
            SentinelEvent::LowMemoryWarn { .. }
            | SentinelEvent::LowSwapWarn { .. }
            | SentinelEvent::PsiPressureWarn { .. } => {
                Self::send_notification("Low Memory Warning", &self.to_string(), "dialog-warning");
            }
            SentinelEvent::KillExecuted { .. } => {
                Self::send_notification("System Load Shedding", &self.to_string(), "process-stop");
            }
            SentinelEvent::KillTriggered { .. } => {
                Self::send_notification(
                    "Kill Sequence Initiated",
                    &self.to_string(),
                    "process-stop",
                );
            }
            SentinelEvent::Message {
                level: LogLevel::Error,
                text,
            } => {
                Self::send_notification("Ram Sentinel Error", text, "dialog-error");
            }
            _ => {}
        }
    }

    fn send_notification(summary: &str, body: &str, icon: &str) {
        // This fails silently if no notification daemon is running, which is preferred for a background service
        let _ = Notification::new()
            .summary(summary)
            .body(body)
            .icon(icon)
            .show();
    }
}
