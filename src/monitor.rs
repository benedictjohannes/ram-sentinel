use crate::{config::{MemoryConfig, RuntimeContext}, psi::read_psi_total};
use sysinfo::{System, RefreshKind, MemoryRefreshKind};
use std::time::Instant;
use byte_unit::Byte;

pub struct Monitor {
    system: System,
    last_psi_total: u64,
    last_psi_time: Instant,
    last_warn_time: Option<Instant>,
}

pub enum MonitorStatus {
    Normal,
    Warn(String), // Message
    Kill(KillReason),
}

#[derive(Debug)]
pub enum KillReason {
    PsiPressure(f32, u64), // Pressure, Bytes to free
    LowMemory(u64), // Bytes available
    LowSwap(u64), // Bytes free
}

impl Monitor {
    pub fn new() -> Self {
        let mut system = System::new_with_specifics(
            RefreshKind::nothing().with_memory(MemoryRefreshKind::everything())
        );
        system.refresh_memory();
        
        let (total, _) = Self::read_psi();

        Self {
            system,
            last_psi_total: total,
            last_psi_time: Instant::now(),
            last_warn_time: None,
        }
    }

    pub fn check(&mut self, ctx: &RuntimeContext) -> MonitorStatus {
        self.system.refresh_memory();
        
        // Check if we are in "Warn Reset" cooldown
        let now = Instant::now();
        if let Some(last) = self.last_warn_time {
            if (now.duration_since(last).as_millis() as u64) < ctx.config.warn_reset_ms {
                // Cooldown active
            }
        }

        // 1. PSI Check
        if let Some(psi_config) = &ctx.config.psi {
            let (current_total, current_time) = Self::read_psi();
            // Calculate pressure
            let time_delta_us = current_time.duration_since(self.last_psi_time).as_micros() as f64;
            let total_delta = (current_total.saturating_sub(self.last_psi_total)) as f64;
            
            let pressure = if time_delta_us > 0.0 {
                (total_delta / time_delta_us) * 100.0
            } else {
                0.0
            };

            // Update state
            self.last_psi_total = current_total;
            self.last_psi_time = current_time;

            if let Some(kill_max) = psi_config.kill_max_percent {
                if pressure as f32 > kill_max {
                    let amount = parse_size(psi_config.amount_to_free.as_ref().unwrap());
                    return MonitorStatus::Kill(KillReason::PsiPressure(pressure as f32, amount));
                }
            }

            if let Some(warn_max) = psi_config.warn_max_percent {
                if pressure as f32 > warn_max && self.can_warn(ctx) {
                    self.last_warn_time = Some(now);
                    return MonitorStatus::Warn(format!("Memory pressure reached {:.1}%", pressure));
                }
            }
        }

        // 2. RAM Check
        if let Some(ram_config) = &ctx.config.ram {
            let available = self.system.available_memory();
            let total = self.system.total_memory();
            let percent_free = (available as f64 / total as f64) * 100.0;

            if should_kill(ram_config, available, percent_free as f32) {
                return MonitorStatus::Kill(KillReason::LowMemory(available));
            }

            if should_warn(ram_config, available, percent_free as f32) && self.can_warn(ctx) {
                self.last_warn_time = Some(now);
                return MonitorStatus::Warn(format!("Low RAM: {:.2} ({:.2}%) free", Byte::from_u64(available).get_appropriate_unit(byte_unit::UnitType::Decimal), percent_free));
            }
        }

        // 3. Swap Check
        if let Some(swap_config) = &ctx.config.swap {
            let free = self.system.free_swap();
            let total = self.system.total_swap();
            // Avoid division by zero if no swap
            if total > 0 {
                let percent_free = (free as f64 / total as f64) * 100.0;

                if should_kill(swap_config, free, percent_free as f32) {
                    return MonitorStatus::Kill(KillReason::LowSwap(free));
                }

                if should_warn(swap_config, free, percent_free as f32) && self.can_warn(ctx) {
                    self.last_warn_time = Some(now);
                    return MonitorStatus::Warn(format!("Low Swap: {:.2} ({:.2}%) free", Byte::from_u64(free).get_appropriate_unit(byte_unit::UnitType::Decimal), percent_free));
                }
            }
        }

        MonitorStatus::Normal
    }

    fn can_warn(&self, ctx: &RuntimeContext) -> bool {
        match self.last_warn_time {
            Some(last) => (Instant::now().duration_since(last).as_millis() as u64) >= ctx.config.warn_reset_ms,
            None => true,
        }
    }

    fn read_psi() -> (u64, Instant) {
        if let Ok(val) = read_psi_total() {
            return (val, Instant::now());
        }
        (0, Instant::now())
    }
    
    pub fn get_system(&self) -> &System {
        &self.system
    }
}

fn should_kill(config: &MemoryConfig, free_bytes: u64, free_percent: f32) -> bool {
    if let Some(limit_bytes) = &config.kill_min_free_bytes {
        let limit = parse_size(limit_bytes);
        if free_bytes < limit { return true; }
    }
    if let Some(limit_percent) = config.kill_min_free_percent {
        if free_percent < limit_percent { return true; }
    }
    false
}

fn should_warn(config: &MemoryConfig, free_bytes: u64, free_percent: f32) -> bool {
    if let Some(limit_bytes) = &config.warn_min_free_bytes {
        let limit = parse_size(limit_bytes);
        if free_bytes < limit { return true; }
        return false; 
    }
    if let Some(limit_percent) = config.warn_min_free_percent {
        if free_percent < limit_percent { return true; }
    }
    false
}

fn parse_size(s: &str) -> u64 {
    Byte::parse_str(s, true).map(|b| b.as_u64()).unwrap_or(0)
}