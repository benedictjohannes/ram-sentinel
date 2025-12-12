use crate::config::{RuntimeContext, KillStrategy};
use sysinfo::{System, RefreshKind, ProcessRefreshKind, ProcessesToUpdate};
use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid as NixPid;
use std::thread;
use std::time::Duration;
use std::fs;
use log::{info, warn, error};
use notify_rust::Notification;

pub struct Killer {
    system: System,
}

impl Killer {
    pub fn new() -> Self {
        Self {
            system: System::new_with_specifics(
                RefreshKind::nothing().with_processes(ProcessRefreshKind::everything())
            ),
        }
    }

    pub fn kill_sequence(&mut self, ctx: &RuntimeContext, reason_desc: &str, mut amount_needed: Option<u64>) {
        info!("Initiating Kill Sequence. Reason: {}. Needed: {:?}", reason_desc, amount_needed);
        
        // We might need to kill multiple times
        loop {
            self.system.refresh_processes(ProcessesToUpdate::All, true);
            let mut candidates = self.find_candidates(ctx);
            
            if candidates.is_empty() {
                warn!("No eligible kill candidates found!");
                break;
            }

            self.sort_candidates(&mut candidates, ctx.config.kill_strategy);

            if let Some(victim_pid) = candidates.first() {
                // Get process info before killing to know how much we might free
                if let Some(victim) = self.system.process(*victim_pid) {
                    let victim_mem = victim.memory(); // In bytes
                    let victim_name = victim.name().to_string_lossy().to_string();
                    let victim_start_time = victim.start_time();

                    info!("Selected victim: {} (PID: {}). Memory: {} bytes.", victim_name, victim_pid, victim_mem);

                    if self.kill_process(ctx, *victim_pid, &victim_name, victim_start_time) {
                         // Success
                         if let Some(needed) = amount_needed {
                             if victim_mem >= needed {
                                 info!("Freed approximately {} bytes (needed {}). Stopping kill sequence.", victim_mem, needed);
                                 break;
                             } else {
                                 amount_needed = Some(needed - victim_mem);
                                 info!("Freed {} bytes. Still need {}. Continuing...", victim_mem, amount_needed.unwrap());
                             }
                         } else {
                             break;
                         }
                    } else {
                        error!("Failed to kill victim. Aborting sequence.");
                        break;
                    }
                } else {
                    warn!("Selected victim {} vanished before kill.", victim_pid);
                    continue;
                }
            }
        }
    }

    fn find_candidates(&self, ctx: &RuntimeContext) -> Vec<sysinfo::Pid> {
        let my_pid = std::process::id();
        
        self.system.processes().iter()
            .filter(|(pid, process)| {
                let pid = **pid; // pid is &&Pid, so deref twice to get Pid (which is Copy)
                if pid.as_u32() == my_pid { return false; } // Don't kill self

                let name = process.name().to_string_lossy();
                // Check ignore list
                for pat in &ctx.ignore_names_regex {
                    if pat.matches(&name) { return false; }
                }

                // Check kill targets
                let mut matched = false;
                for pat in &ctx.kill_targets_regex {
                    if pat.matches(&name) { matched = true; break; }
                }
                
                if !matched {
                    let cmd_line = process.cmd().iter()
                        .map(|s| s.to_string_lossy())
                        .collect::<Vec<_>>()
                        .join(" ");
                        
                    for pat in &ctx.kill_targets_regex {
                        if pat.matches(&cmd_line) { matched = true; break; }
                    }
                }
                
                matched
            })
            .map(|(&pid, _)| pid)
            .collect()
    }

    fn sort_candidates(&self, candidates: &mut Vec<sysinfo::Pid>, strategy: KillStrategy) {
        candidates.sort_by(|a, b| {
            let proc_a = self.system.process(*a).unwrap();
            let proc_b = self.system.process(*b).unwrap();

            match strategy {
                KillStrategy::LargestRss => proc_b.memory().cmp(&proc_a.memory()), // Descending
                KillStrategy::HighestOomScore => {
                    let score_a = get_oom_score(*a);
                    let score_b = get_oom_score(*b);
                    score_b.cmp(&score_a) // Descending
                }
            }
        });
    }

    fn kill_process(&mut self, ctx: &RuntimeContext, pid: sysinfo::Pid, name: &str, create_time: u64) -> bool {
        let nix_pid = NixPid::from_raw(pid.as_u32() as i32);
        
        info!("Sending SIGTERM to process '{}' (PID: {})", name, pid);
        if let Err(e) = kill(nix_pid, Signal::SIGTERM) {
            error!("Failed to send SIGTERM to {}: {}", pid, e);
            return false;
        }

        thread::sleep(Duration::from_millis(ctx.config.sigterm_wait_ms));

        // Verify if still running and same process
        // We use refresh_processes with a specific PID filter if possible, or just refresh all?
        // sysinfo 0.37 doesn't seem to have `refresh_process(pid)`.
        // It has `refresh_processes` taking `ProcessesToUpdate`.
        // We can pass `ProcessesToUpdate::Some(&[pid])`.
        
        self.system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
        
        if self.system.process(pid).is_none() {
             // Process is gone
             info!("Process {} terminated gracefully after SIGTERM.", pid);
             send_notification("System Load Shedding", &format!("Terminated process '{}' (PID {}) to prevent system freeze.", name, pid), "process-stop");
             return true;
        }
        
        // Process exists, check start time
        if let Some(process) = self.system.process(pid) {
            if process.start_time() != create_time {
                 info!("Process {} terminated (PID reused).", pid);
                 send_notification("System Load Shedding", &format!("Terminated process '{}' (PID {}) to prevent system freeze.", name, pid), "process-stop");
                 return true;
            }
        }

        info!("Process {} still running. Sending SIGKILL.", pid);
        if let Err(e) = kill(nix_pid, Signal::SIGKILL) {
             error!("Failed to send SIGKILL to {}: {}", pid, e);
             return false;
        }
        
        send_notification("System Load Shedding", &format!("Force killed process '{}' (PID {}) to prevent system freeze.", name, pid), "process-stop");
        true
    }
}

fn get_oom_score(pid: sysinfo::Pid) -> i32 {
    let path = format!("/proc/{}/oom_score", pid);
    if let Ok(s) = fs::read_to_string(path) {
        if let Ok(val) = s.trim().parse::<i32>() {
            return val;
        }
    }
    0
}

fn send_notification(title: &str, body: &str, icon: &str) {
    let _ = Notification::new()
        .summary(title)
        .body(body)
        .icon(icon)
        .show();
}
