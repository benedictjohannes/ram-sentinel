use std::fs;
use std::sync::OnceLock;

use crate::events::{LogLevel, SentinelEvent};
use crate::logging;

static HEADERS_VALID: OnceLock<bool> = OnceLock::new();

#[derive(Debug, Default, Clone)]
pub struct AggregatedSwapStats {
    pub disk_total: u64,
    pub disk_free: u64,
    pub zram_total: u64,
    pub zram_free: u64,
}

pub fn get_aggregated_swap_stats(has_swap: bool, has_zram: bool) -> AggregatedSwapStats {
    // If validation failed previously, return default/empty stats
    if let Some(false) = HEADERS_VALID.get() {
        return AggregatedSwapStats::default();
    }

    // 1. Parse /proc/swaps
    let swaps_content = fs::read_to_string("/proc/swaps").unwrap_or_default();

    // Validate headers on first run
    let is_valid = *HEADERS_VALID.get_or_init(|| {
        let first_line = swaps_content.lines().next().unwrap_or("");
        let parts: Vec<&str> = first_line.split_whitespace().collect();

        // Expect: Filename (0), Type (1), Size (2), Used (3), Priority (4)
        let ok = parts.get(0) == Some(&"Filename")
            && parts.get(2) == Some(&"Size")
            && parts.get(3) == Some(&"Used");

        if !ok {
            logging::emit(&SentinelEvent::Message {
                level: LogLevel::Error,
                text: "Invalid /proc/swaps header format. Column mismatch. Expected Filename(0), Size(2), Used(3). Disabling swap monitoring.",
            });
        }
        ok
    });

    if !is_valid {
        return AggregatedSwapStats::default();
    }

    let mut stats = AggregatedSwapStats::default();

    // Skip header
    for line in swaps_content.lines().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 4 {
            continue;
        }

        let filename = parts[0];
        let total_kb: u64 = parts[2].parse().unwrap_or(0);
        let used_kb: u64 = parts[3].parse().unwrap_or(0);

        // Kernel reports in KB, we want bytes
        let virtual_total = total_kb * 1024;
        let virtual_used = used_kb * 1024;
        let virtual_free = virtual_total.saturating_sub(virtual_used);

        if is_zram(filename) {
            if has_zram {
                stats.zram_total += virtual_total;
                stats.zram_free += virtual_free;
            }
        } else if has_swap {
            stats.disk_total += virtual_total;
            stats.disk_free += virtual_free;
        }
    }

    stats
}

fn is_zram(filename: &str) -> bool {
    filename.starts_with("/dev/zram")
}
