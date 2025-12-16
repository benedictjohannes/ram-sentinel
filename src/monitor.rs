use crate::{
    config::{MemoryConfigParsed, RuntimeContext},
    logging::{LogLevel, SentinelEvent, get_log_level},
    psi::read_psi_total,
};
use std::time::Instant;
use sysinfo::{MemoryRefreshKind, RefreshKind, System};

pub struct Monitor {
    system: System,
    last_psi_total: Option<u64>,
    last_psi_time: Instant,
    last_warn_time: Option<Instant>,
    pub ram_bytes: Option<u64>,
    pub ram_percent: Option<f64>,
    pub swap_bytes: Option<u64>,
    pub swap_percent: Option<f64>,
    pub psi_pressure: Option<f64>,
}

pub enum MonitorStatus {
    Normal,
    Warn,                // Event emitted internally
    Kill(SentinelEvent), // Main needs the event to decide/log (contains amount_needed)
}

impl Monitor {
    pub fn new() -> Self {
        let mut system = System::new_with_specifics(
            RefreshKind::nothing().with_memory(MemoryRefreshKind::everything()),
        );
        system.refresh_memory();

        let total = Self::read_psi();

        // logging test start
        SentinelEvent::Startup { interval_ms: 0 }.emit();
        SentinelEvent::LowMemoryWarn {
            available_bytes: 1024 * 1024 * 50,
            available_percent: 5.0,
            threshold_type: "percent".to_string(),
            threshold_value: 10.0,
        }
        .emit();
        SentinelEvent::LowSwapWarn {
            free_bytes: 1024 * 1024 * 10,
            free_percent: 1.0,
            threshold_type: "percent".to_string(),
            threshold_value: 10.0,
        }
        .emit();
        SentinelEvent::PsiPressureWarn {
            pressure_curr: 45.5,
            threshold: 20.0,
        }
        .emit();
        SentinelEvent::KillTriggered {
            trigger: "LowMemory".to_string(),
            observed_value: 5.0,
            threshold_value: 10.0,
            threshold_type: "percent".to_string(),
            amount_needed: Some(1024 * 1024 * 500),
        }
        .emit();
        SentinelEvent::KillCandidateSelected {
            pid: 12345,
            process_name: "chrome-dummy".to_string(),
            score: 5000,
            rss: 1024 * 1024 * 200,
            match_index: 0,
        }
        .emit();
        SentinelEvent::KillExecuted {
            pid: 12345,
            process_name: "chrome-dummy".to_string(),
            strategy: "SIGTERM".to_string(),
            rss_freed: 1024 * 1024 * 200,
        }
        .emit();
        SentinelEvent::KillSequenceAborted {
            reason: "Init Test Complete".to_string(),
        }
        .emit();
        SentinelEvent::KillCandidateIgnored {
            pid: 6789,
            reason: "Dummy Verify".to_string(),
        }
        .emit();
        // logging test end

        Self {
            system,
            last_psi_total: total,
            last_psi_time: Instant::now(),
            last_warn_time: None,
            ram_bytes: None,
            ram_percent: None,
            swap_bytes: None,
            swap_percent: None,
            psi_pressure: None,
        }
    }

    pub fn check(&mut self, ctx: &RuntimeContext) -> MonitorStatus {
        self.system.refresh_memory();
        let now = Instant::now();

        // We use this to store a warning from a higher priority check.
        // It can be overridden ONLY by a Kill, never by another Warning.
        let mut pending_warn: Option<SentinelEvent> = None;

        // Priority 1: RAM (Hard Limit)
        if let Some(ram_config) = &ctx.ram {
            let available = self.system.available_memory();
            let total = self.system.total_memory();

            if total > 0 {
                let percent_free = (available as f64 / total as f64) * 100.0;
                self.ram_bytes = Some(available);
                self.ram_percent = Some(percent_free);

                if let Some((threshold, type_str)) =
                    check_kill(ram_config, available, percent_free as f32)
                {
                    let amount_needed = calc_needed(ram_config, available, total);
                    return MonitorStatus::Kill(SentinelEvent::KillTriggered {
                        trigger: "LowMemory".to_string(),
                        observed_value: if type_str == "bytes" {
                            available as f64
                        } else {
                            percent_free
                        },
                        threshold_value: threshold,
                        threshold_type: type_str,
                        amount_needed,
                    });
                }

                if let Some((threshold, type_str)) =
                    check_warn(ram_config, available, percent_free as f32)
                {
                    if pending_warn.is_none() {
                        pending_warn = Some(SentinelEvent::LowMemoryWarn {
                            available_bytes: available,
                            available_percent: percent_free,
                            threshold_type: type_str,
                            threshold_value: threshold,
                        });
                    }
                }
            }
        }

        // Priority 2: Swap (Hard Limit)
        if let Some(swap_config) = &ctx.swap {
            let free = self.system.free_swap();
            let total = self.system.total_swap();

            if total > 0 {
                let percent_free = (free as f64 / total as f64) * 100.0;
                self.swap_bytes = Some(free);
                self.swap_percent = Some(percent_free);

                if let Some((threshold, type_str)) =
                    check_kill(swap_config, free, percent_free as f32)
                {
                    let amount_needed = calc_needed(swap_config, free, total);
                    return MonitorStatus::Kill(SentinelEvent::KillTriggered {
                        trigger: "LowSwap".to_string(),
                        observed_value: if type_str == "bytes" {
                            free as f64
                        } else {
                            percent_free
                        },
                        threshold_value: threshold,
                        threshold_type: type_str,
                        amount_needed,
                    });
                }

                if let Some((threshold, type_str)) =
                    check_warn(swap_config, free, percent_free as f32)
                {
                    if pending_warn.is_none() {
                        pending_warn = Some(SentinelEvent::LowSwapWarn {
                            free_bytes: free,
                            free_percent: percent_free,
                            threshold_type: type_str,
                            threshold_value: threshold,
                        });
                    }
                }
            }
        }

        // Priority 3: PSI
        if let Some(psi_config) = &ctx.psi {
            if now.duration_since(self.last_psi_time).as_millis() as u64
                >= psi_config.check_interval_ms
            {
                if let Some(current_total) = Self::read_psi() {
                    // We need previous data to calculate pressure
                    if let Some(last_total) = self.last_psi_total {
                        let time_delta_us =
                            now.duration_since(self.last_psi_time).as_micros() as f64;
                        let total_delta = (current_total.saturating_sub(last_total)) as f64;

                        let pressure = if time_delta_us > 0.0 {
                            (total_delta / time_delta_us) * 100.0
                        } else {
                            0.0
                        };

                        // Update State
                        self.last_psi_total = Some(current_total);
                        self.last_psi_time = now;
                        self.psi_pressure = Some(pressure);

                        // Check Kill
                        if let Some(kill_max) = psi_config.kill_max_percent {
                            if pressure as f32 > kill_max {
                                let amount = psi_config.amount_to_free.expect("validated");
                                return MonitorStatus::Kill(SentinelEvent::KillTriggered {
                                    trigger: "PsiPressure".to_string(),
                                    observed_value: pressure,
                                    threshold_value: kill_max as f64,
                                    threshold_type: "percent".to_string(),
                                    amount_needed: Some(amount),
                                });
                            }
                        }

                        // Check Warn
                        if pending_warn.is_none() {
                            if let Some(warn_max) = psi_config.warn_max_percent {
                                if pressure as f32 > warn_max {
                                    pending_warn = Some(SentinelEvent::PsiPressureWarn {
                                        pressure_curr: pressure,
                                        threshold: warn_max as f64,
                                    });
                                }
                            }
                        }
                    } else {
                        self.last_psi_total = Some(current_total);
                        self.last_psi_time = now;
                    }
                }
            }
        }

        // emit heartbeat
        if get_log_level() >= LogLevel::Debug {
            SentinelEvent::Monitor {
                memory_available_bytes: self.ram_bytes,
                memory_available_percent: self.ram_percent,
                swap_free_bytes: self.swap_bytes,
                swap_free_percent: self.swap_percent,
                psi_pressure_curr: self.psi_pressure,
            }.emit();
        }

        // Final Decision (Warnings)
        if let Some(event) = pending_warn {
            if self.can_warn(ctx) {
                event.emit();
                self.last_warn_time = Some(now);
                return MonitorStatus::Warn;
            }
        }

        MonitorStatus::Normal
    }

    fn can_warn(&self, ctx: &RuntimeContext) -> bool {
        match self.last_warn_time {
            Some(last) => {
                (Instant::now().duration_since(last).as_millis() as u64) >= ctx.warn_reset_ms
            }
            None => true,
        }
    }

    fn read_psi() -> Option<u64> {
        read_psi_total().ok()
    }

}

// Helpers returning (threshold_val, type_string)
fn check_kill(
    config: &MemoryConfigParsed,
    free_bytes: u64,
    free_percent: f32,
) -> Option<(f64, String)> {
    if let Some(limit) = config.kill_min_free_bytes {
        if free_bytes < limit {
            return Some((limit as f64, "bytes".to_string()));
        }
        // Strict Priority: If bytes limit exists, ignore percent?
        // Task said: "If a byte limit is set, the percentage limit is ignored".
        return None;
    }
    if let Some(limit_percent) = config.kill_min_free_percent {
        if free_percent < limit_percent {
            return Some((limit_percent as f64, "percent".to_string()));
        }
    }
    None
}

fn check_warn(
    config: &MemoryConfigParsed,
    free_bytes: u64,
    free_percent: f32,
) -> Option<(f64, String)> {
    if let Some(limit) = config.warn_min_free_bytes {
        if free_bytes < limit {
            return Some((limit as f64, "bytes".to_string()));
        }
        return None;
    }
    if let Some(limit_percent) = config.warn_min_free_percent {
        if free_percent < limit_percent {
            return Some((limit_percent as f64, "percent".to_string()));
        }
    }
    None
}

fn calc_needed(config: &MemoryConfigParsed, current_free: u64, total: u64) -> Option<u64> {
    let target = if let Some(bytes) = config.kill_min_free_bytes {
        bytes
    } else if let Some(percent) = config.kill_min_free_percent {
        (total as f64 * (percent as f64 / 100.0)) as u64
    } else {
        0
    };

    if target > current_free {
        Some(target - current_free)
    } else {
        None
    }
}
