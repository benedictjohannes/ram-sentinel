use std::borrow::Cow;
use crate::config::{RuntimeContext, KillStrategy};
use sysinfo::{System, RefreshKind, ProcessRefreshKind, ProcessesToUpdate};
use nix::sys::signal::{kill, Signal};
use nix::unistd::{Pid as NixPid, Uid};
use std::thread;
use std::time::Duration;
use std::fs;
use std::cmp::Ordering;
use log::{info, warn, error};
use notify_rust::Notification;

pub struct Killer {
    system: System,
}

// Internal struct to handle the ranking logic (Process + KillScore + MatchPriority)
struct KillCandidate {
    pid: sysinfo::Pid,
    score: u64,           // RSS (bytes) or OOM Score (normalized to u64)
    match_index: usize,   // 0..N for explicit targets, usize::MAX for non-matches
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
        
        loop {
            // 1. Refresh World State
            self.system.refresh_processes(ProcessesToUpdate::All, true);
            
            // 2. Get Candidates (Sorted by Priority -> Score)
            let candidates = self.get_ranked_candidates(ctx);
            
            if candidates.is_empty() {
                warn!("No eligible kill candidates found!");
                break;
            }

            // 3. Pick the Top Candidate
            if let Some(candidate) = candidates.first() {
                if let Some(victim) = self.system.process(candidate.pid) {
                    let victim_mem = victim.memory();
                    let victim_name = victim.name().to_string_lossy().to_string();
                    let victim_start_time = victim.start_time();

                    info!("Selected victim: {} (PID: {}). Score: {}. MatchPriority: {}", 
                        victim_name, candidate.pid, candidate.score, 
                        if candidate.match_index == usize::MAX { "None".to_string() } else { candidate.match_index.to_string() }
                    );

                    if self.kill_process(ctx, candidate.pid, &victim_name, victim_start_time) {
                         if let Some(needed) = amount_needed {
                             if victim_mem >= needed {
                                 info!("Freed approx {} bytes (needed {}). Stopping sequence.", victim_mem, needed);
                                 break;
                             } else {
                                 amount_needed = Some(needed - victim_mem);
                                 info!("Freed {} bytes. Still need {}. Continuing...", victim_mem, amount_needed.unwrap());
                             }
                         } else {
                             break;
                         }
                    } else {
                        error!("Failed to kill victim PID {} {}. Aborting sequence.", candidate.pid, victim_name);
                        send_notification("Kill Failure", 
                        &format!("Failed to terminate process '{}' (PID {})", victim_name, candidate.pid), 
                        "dialog-error");
                        break;
                    }
                } else {
                    // Process vanished between list generation and access
                    continue;
                }
            }
        }
    }

    // Replaces find_candidates + sort_candidates
    fn get_ranked_candidates(&self, ctx: &RuntimeContext) -> Vec<KillCandidate> {
        let my_pid = std::process::id();
        let current_uid = Uid::effective();
        let is_root = current_uid.is_root();
        let current_uid_str = current_uid.to_string();
        
        let mut candidates: Vec<KillCandidate> = self.system.processes().iter()
            .filter(|(pid, process)| {
                // Filter 1: Never kill self
                if pid.as_u32() == my_pid { return false; } 

                // Filter 2: Ownership Check (if not root)
                if !is_root {
                    if let Some(proc_uid) = process.user_id() {
                        if proc_uid.to_string() != current_uid_str {
                            return false;
                        }
                    } else {
                        return false;
                    }
                }

                // Filter 3: Never kill Ignored Names
                let name = process.name().to_string_lossy();
                for pat in &ctx.ignore_names_regex {
                    if pat.matches(&name) { return false; }
                }
                true
            })
            .map(|(pid, process)| {
                let name = process.name().to_string_lossy();
                let mut match_index = usize::MAX;

                for (idx, pat) in ctx.kill_targets_regex.iter().enumerate() {
                    if pat.matches(&name) {
                        match_index = idx;
                        break;
                    }
                }

                // Only construct expensive cmdline if name didn't match
                if match_index == usize::MAX && !ctx.kill_targets_regex.is_empty() {
                    let cmd_line = process.cmd().iter()
                        .map(|s| s.to_string_lossy())
                        .collect::<Vec<Cow<str>>>()
                        .join(" ");
                    
                    for (idx, pat) in ctx.kill_targets_regex.iter().enumerate() {
                        if pat.matches(&cmd_line) {
                            match_index = idx;
                            break;
                        }
                    }
                }

                // 2. Calculate Kill Score
                let score = match ctx.kill_strategy {
                    KillStrategy::LargestRss => process.memory(),
                    KillStrategy::HighestOomScore => get_oom_score(*pid) as u64,
                };

                KillCandidate {
                    pid: *pid,
                    score,
                    match_index,
                }
            })
            .collect();

        // 3. Sort: First by Match Priority (Ascending), Then by Score (Descending)
        candidates.sort_by(|a, b| {
            // Compare Match Index (Lower index = Higher priority)
            match a.match_index.cmp(&b.match_index) {
                Ordering::Equal => {
                    // If priority is same, largest score dies first
                    b.score.cmp(&a.score)
                },
                other => other,
            }
        });

        candidates
    }

    fn kill_process(&mut self, ctx: &RuntimeContext, pid: sysinfo::Pid, name: &str, create_time: u64) -> bool {
        let nix_pid = NixPid::from_raw(pid.as_u32() as i32);
        
        info!("Sending SIGTERM to process '{}' (PID: {})", name, pid);
        if let Err(e) = kill(nix_pid, Signal::SIGTERM) {
            if e == nix::errno::Errno::ESRCH {
                info!("Process {} already gone (ESRCH) during SIGTERM.", pid);
                return true;
            }
            error!("Failed to send SIGTERM to {}: {}", pid, e);
            return false;
        }

        thread::sleep(Duration::from_millis(ctx.sigterm_wait_ms));

        // Refill process list specifically to check this PID
        self.system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
        
        // Check if gone
        if self.system.process(pid).is_none() {
             info!("Process {} terminated gracefully.", pid);
             send_notification("System Load Shedding", &format!("Terminated process '{}' (PID {}) to prevent system freeze.", name, pid), "process-stop");
             return true;
        }
        
        // Check if PID reused (safety)
        if let Some(process) = self.system.process(pid) {
            if process.start_time() != create_time {
                 send_notification("System Load Shedding", &format!("Terminated process '{}' (PID {}) to prevent system freeze.", name, pid), "process-stop");
                 info!("Process {} terminated (PID reused).", pid);
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
