use chrono::Utc;
use notify_rust::Notification;
use serde_json::json;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::events::{LogLevel, LogMode, SentinelEvent};

// Re-export for convenience/backward compatibility of imports
// pub use crate::events::{LogLevel as Level, LogMode as Mode};

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

/// Primary entry point for logging/notifying.
pub fn emit(event: &SentinelEvent) {
    // 1. Check Global Log Level (Filtering)
    // If event severity (e.g., Info=3) is greater than Configured Level (e.g., Warn=2), skip.
    if event.severity() > get_log_level() {
        return;
    }

    // 2. Desktop Notification (if applicable)
    emit_notification(event);

    // 3. Output to Stdout
    match get_log_mode() {
        LogMode::Json => log_json(event),
        LogMode::Compact => log_compact(event),
    }
}

fn log_compact(event: &SentinelEvent) {
    // Format: YYYY-MM-DDTHH:MM:SSZ [LEVEL] Message...
    println!(
        "{} [{}] {}",
        Utc::now().to_rfc3339(),
        event.severity().as_str(),
        event
    );
}

fn log_json(event: &SentinelEvent) {
    // We use serde_json::to_value to get the fields of the event
    let mut log_entry = serde_json::to_value(event).unwrap_or(json!({
        "event": "SerializationError"
    }));

    // Flatten metadata into the root object
    if let Some(map) = log_entry.as_object_mut() {
        map.insert("timestamp".into(), Utc::now().to_rfc3339().into());
        map.insert("level".into(), event.severity().as_str().into());
        match event {
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

fn emit_notification(event: &SentinelEvent) {
    // Only notify on actual issues or actions, not just Info logs
    match event {
        SentinelEvent::LowMemoryWarn { .. }
        | SentinelEvent::LowSwapWarn { .. }
        | SentinelEvent::PsiPressureWarn { .. } => {
            send_notification("Low Memory Warning", &event.to_string(), "dialog-warning");
        }
        SentinelEvent::KillExecuted { .. } => {
            send_notification("System Load Shedding", &event.to_string(), "process-stop");
        }
        SentinelEvent::KillTriggered { .. } => {
            send_notification(
                "Kill Sequence Initiated",
                &event.to_string(),
                "process-stop",
            );
        }
        SentinelEvent::Message { level, text, .. } => match level {
            LogLevel::Warn => {
                send_notification("Ram Sentinel Warning", text, "dialog-warning");
            }
            LogLevel::Error => {
                send_notification("Ram Sentinel Error", text, "dialog-error");
            }
            _ => {}
        },
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