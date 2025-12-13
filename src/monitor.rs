use crate::{
    config::{MemoryConfigParsed, RuntimeContext},
    psi::read_psi_total,
};
use byte_unit::Byte;
use std::time::Instant;
use sysinfo::{MemoryRefreshKind, RefreshKind, System};

pub struct Monitor {
    system: System,
    last_psi_total: Option<u64>,
    last_psi_time: Instant,
    last_warn_time: Option<Instant>,
}

pub enum MonitorStatus {
    Normal,
    Warn(String), // Message
    Kill(KillReason),
}

#[derive(Debug)]
#[allow(dead_code)] // this gets printed in logs
// Kill reasons, ordered by urgency. 
// Actual priority determined by check order in `Monitor::check()`.
pub enum KillReason {
    LowMemory(u64),        // Priority 1
    LowSwap(u64),          // Priority 2
    PsiPressure(f32, u64), // Priority 3
}

impl Monitor {
    pub fn new() -> Self {
        let mut system = System::new_with_specifics(
            RefreshKind::nothing().with_memory(MemoryRefreshKind::everything()),
        );
        system.refresh_memory();

        let total = Self::read_psi();

        Self {
            system,
            last_psi_total: total,
            last_psi_time: Instant::now(),
            last_warn_time: None,
        }
    }

    pub fn check(&mut self, ctx: &RuntimeContext) -> MonitorStatus {
        self.system.refresh_memory();
        let now = Instant::now();

        // We use this to store a warning from a higher priority check.
        // It can be overridden ONLY by a Kill, never by another Warning.
        let mut pending_warn: Option<String> = None;

        // Priority 1: RAM (Hard Limit)
        if let Some(ram_config) = &ctx.ram {
            let available = self.system.available_memory();
            let total = self.system.total_memory();

            if total > 0 {
                let percent_free = (available as f64 / total as f64) * 100.0;

                if should_kill(ram_config, available, percent_free as f32) {
                    return MonitorStatus::Kill(KillReason::LowMemory(available));
                }

                if should_warn(ram_config, available, percent_free as f32) {
                    pending_warn = Some(format!(
                        "Low RAM: {} ({:.2}%) available",
                        Byte::from_u64(available)
                            .get_appropriate_unit(byte_unit::UnitType::Decimal),
                        percent_free
                    ));
                }
            }
        }

        // Priority 2: Swap (Hard Limit)
        if let Some(swap_config) = &ctx.swap {
            let free = self.system.free_swap();
            let total = self.system.total_swap();

            if total > 0 {
                let percent_free = (free as f64 / total as f64) * 100.0;

                if should_kill(swap_config, free, percent_free as f32) {
                    return MonitorStatus::Kill(KillReason::LowSwap(free));
                }

                if pending_warn.is_none() && should_warn(swap_config, free, percent_free as f32) {
                    pending_warn = Some(format!(
                        "Low Swap: {} ({:.2}%) available",
                        Byte::from_u64(free).get_appropriate_unit(byte_unit::UnitType::Decimal),
                        percent_free
                    ));
                }
            }
        }

        // Priority 3: PSI (Soft Pressure)
        if let Some(psi_config) = &ctx.psi {
            // We only run PSI logic if the interval has passed
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

                        // 1. Check Kill (Escalate Warn -> Kill)
                        if let Some(kill_max) = psi_config.kill_max_percent {
                            if pressure as f32 > kill_max {
                                let amount =
                                    psi_config.amount_to_free.expect("validated in config");
                                return MonitorStatus::Kill(KillReason::PsiPressure(
                                    pressure as f32,
                                    amount,
                                ));
                            }
                        }

                        // 2. Check Warn (Only if RAM or Swap didn't warn)
                        if pending_warn.is_none() {
                            if let Some(warn_max) = psi_config.warn_max_percent {
                                if pressure as f32 > warn_max {
                                    pending_warn =
                                        Some(format!("Memory pressure reached {:.2}%", pressure));
                                }
                            }
                        }
                    } else {
                        // Initialize Baseline (First Run)
                        self.last_psi_total = Some(current_total);
                        self.last_psi_time = now;
                    }
                }
            }
        }

        // Final Decision (Warnings)
        if let Some(msg) = pending_warn {
            if self.can_warn(ctx) {
                self.last_warn_time = Some(now);
                return MonitorStatus::Warn(msg);
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

    pub fn get_system(&self) -> &System {
        &self.system
    }

    pub fn refresh_memory(&mut self) {
        self.system.refresh_memory();
    }
}

fn should_kill(config: &MemoryConfigParsed, free_bytes: u64, free_percent: f32) -> bool {
    if let Some(limit) = config.kill_min_free_bytes {
        if free_bytes < limit { return true; }
        return false;
    }
    if let Some(limit_percent) = config.kill_min_free_percent {
        if free_percent < limit_percent { return true; }
    }
    false
}

fn should_warn(config: &MemoryConfigParsed, free_bytes: u64, free_percent: f32) -> bool {
    if let Some(limit) = config.warn_min_free_bytes {
        if free_bytes < limit { return true; }
        return false;
    }
    if let Some(limit_percent) = config.warn_min_free_percent {
        if free_percent < limit_percent { return true; }
    }
    false
}
