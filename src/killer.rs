use crate::config::{KillStrategy, RuntimeContext};
use crate::events::SentinelEvent;
use crate::logging;
use nix::sys::signal::{Signal, kill};
use nix::unistd::{Pid as NixPid, SysconfVar, Uid, sysconf};
use std::fmt::Write; // For writing to path_buffer
use std::fs::{self, File};
use std::io::Read;
use std::thread;
use std::time::Duration;

pub struct Killer {
    // Buffers for zero-allocation logic
    read_buffer: Vec<u8>,
    path_buffer: String,
    page_size: u64,
}

#[derive(Debug, Clone)]
struct Champion {
    pid: u32,
    score: u64,         // Sorting metric (RSS or OOM Score)
    rss: u64,           // Actual memory usage in bytes
    match_index: usize, // 0..N for explicit targets, usize::MAX for non-matches
    start_time: u64,    // From /proc/[pid]/stat (for safety check)
}

impl Killer {
    pub fn new() -> Self {
        // Query system page size (default to 4096 if fails)
        let page_size = match sysconf(SysconfVar::PAGE_SIZE) {
            Ok(Some(val)) => val as u64,
            _ => 4096,
        };

        Self {
            // Pre-allocate AND initialize to ensure pages are physically backed (prevent page faults during OOM)
            read_buffer: vec![0u8; 256 * 1024],
            path_buffer: String::with_capacity(256),
            page_size,
        }
    }

    pub fn kill_sequence(&mut self, ctx: &RuntimeContext, mut amount_needed: Option<u64>) {
        loop {
            // 1. Scan /proc and find the best candidate ("The Champion")
            let champion_opt = self.find_champion(ctx);

            if let Some(champion) = champion_opt {
                // Fetch name for logging (on-demand, after scan loop)
                let name = self
                    .get_process_name(champion.pid)
                    .unwrap_or_else(|| "unknown".to_string());

                logging::emit(&SentinelEvent::KillCandidateSelected {
                    pid: champion.pid,
                    process_name: name.clone(),
                    score: champion.score,
                    rss: champion.rss,
                    match_index: champion.match_index,
                });

                // 2. Kill Logic
                match self.kill_process(ctx, &champion, &name) {
                    Some(freed_bytes) => {
                        if let Some(needed) = amount_needed {
                            if freed_bytes >= needed {
                                logging::emit(&SentinelEvent::KillSequenceAborted {
                                    reason: format!("Freed {} bytes. Target reached.", freed_bytes),
                                });
                                break;
                            } else {
                                amount_needed = Some(needed - freed_bytes);
                            }
                        } else {
                            // If no specific amount was requested, stop after one kill
                            break;
                        }
                    }
                    None => {
                        logging::emit(&SentinelEvent::KillSequenceAborted {
                            reason: format!(
                                "Failed to kill victim PID {} {}. Aborting.",
                                champion.pid, name
                            ),
                        });
                        break;
                    }
                }
            } else {
                logging::emit(&SentinelEvent::KillSequenceAborted {
                    reason: "No eligible kill candidates found!".to_string(),
                });
                break;
            }
        }
    }

    fn get_process_name(&mut self, pid: u32) -> Option<String> {
        if self.read_file_into_buffer(&pid.to_string(), "comm").is_ok() {
            std::str::from_utf8(&self.read_buffer)
                .ok()
                .map(|s| s.trim().to_string()) // only allocate the small trimmed string
        } else {
            None
        }
    }

    /// The "Hunter" Loop: Scans /proc manually to find the best kill candidate
    /// This avoids large allocations by reusing internal buffers.
    fn find_champion(&mut self, ctx: &RuntimeContext) -> Option<Champion> {
        let current_uid = Uid::effective();
        let is_root = current_uid.is_root();
        let my_pid = std::process::id();

        let mut current_champion: Option<Champion> = None;

        // Manual /proc implementation using std::fs::read_dir
        let entries = match fs::read_dir("/proc") {
            Ok(iter) => iter,
            Err(e) => {
                logging::emit(&SentinelEvent::KillSequenceAborted {
                    reason: format!("Failed to read /proc: {}", e),
                });
                return None;
            }
        };

        for entry in entries {
            if let Ok(entry) = entry {
                // Get filename (PID)
                let file_name = entry.file_name();
                let file_name_str = match file_name.to_str() {
                    Some(s) => s,
                    None => continue,
                };

                // Filter 1: Must be PID (numeric)
                let pid: u32 = match file_name_str.parse() {
                    Ok(p) => p,
                    Err(_) => continue,
                };

                // Filter 2: Never kill self
                if pid == my_pid {
                    continue;
                }

                // Filter 3: Ownership Check (if not root)
                if !is_root {
                    use std::os::unix::fs::MetadataExt;
                    // Avoid stat call if possible, but we need UID. entry.metadata() is cached from readdir? No, usually distinct.
                    if let Ok(meta) = entry.metadata() {
                        if meta.uid() != current_uid.as_raw() {
                            continue;
                        }
                    } else {
                        continue;
                    }
                }

                // ---------------------------------------------------------
                // Analyze Process
                // ---------------------------------------------------------

                // A. Determine Match Priority (Read cmdline)
                if self
                    .read_file_into_buffer(file_name_str, "cmdline")
                    .is_err()
                {
                    continue; // Process likely gone
                }

                // Replace nulls with spaces
                for b in self.read_buffer.iter_mut() {
                    if *b == 0 {
                        *b = 32;
                    }
                }

                // Cow::Borrowed if UTF-8, Owned if not.
                let cmdline_cow = String::from_utf8_lossy(&self.read_buffer);

                // Check Ignored
                let mut ignored = false;
                for pat in &ctx.ignore_names_regex {
                    if pat.matches(&cmdline_cow) {
                        ignored = true;
                        break;
                    }
                }
                if ignored {
                    continue;
                }

                // Calculate Match Index
                let mut match_index = usize::MAX;
                for (idx, pat) in ctx.kill_targets_regex.iter().enumerate() {
                    if pat.matches(&cmdline_cow) {
                        match_index = idx;
                        break;
                    }
                }

                // Check vs Current Champion (Optimization)
                if let Some(champ) = &current_champion {
                    if match_index > champ.match_index {
                        continue;
                    }
                }

                // B. Calculate Score & RSS
                let mut rss = 0;
                let mut score = 0;

                match ctx.kill_strategy {
                    KillStrategy::LargestRss => {
                        // Read statm for RSS
                        if self.read_file_into_buffer(file_name_str, "statm").is_ok() {
                            // format: total resident share ...
                            if let Ok(s) = std::str::from_utf8(&self.read_buffer) {
                                let mut parts = s.split_whitespace();
                                if let Some(_total) = parts.next() {
                                    if let Some(res) = parts.next() {
                                        if let Ok(pages) = res.parse::<u64>() {
                                            rss = pages * self.page_size;
                                        }
                                    }
                                }
                            }
                        }
                        score = rss;
                    }
                    KillStrategy::HighestOomScore => {
                        // Read oom_score
                        if self
                            .read_file_into_buffer(file_name_str, "oom_score")
                            .is_ok()
                        {
                            if let Ok(s) = std::str::from_utf8(&self.read_buffer) {
                                if let Ok(val) = s.trim().parse::<i32>() {
                                    score = val as u64;
                                }
                            }
                        }
                    }
                }

                // Final Comparison
                if let Some(champ) = &current_champion {
                    if match_index == champ.match_index {
                        if score <= champ.score {
                            continue;
                        }
                    } else if match_index > champ.match_index {
                        continue;
                    }
                }

                // C. Become the Champion (Read stat for Start Time)
                if self.read_file_into_buffer(file_name_str, "stat").is_ok() {
                    if let Ok(s) = std::str::from_utf8(&self.read_buffer) {
                        // Robust parsing: "pid (comm) state ppid ..."
                        // Use split_once on ") " to correctly handle ')' in comm
                        if let Some((_before, after_comm)) = s.split_once(") ") {
                            // fields in after_comm:
                            // 0:state ... 19:starttime (index 19 in this slice? No, count carefully)
                            // Global stat fields:
                            // 1: pid, 2: comm, 3: state, ..., 22: starttime
                            // after_comm starts at field 3 (state).
                            // So index 0 = field 3.
                            // We want field 22.
                            // Offset = 22 - 3 = 19.
                            // So .nth(19) is correct.

                            if let Some(start_time_str) = after_comm.split_whitespace().nth(19) {
                                if let Ok(st) = start_time_str.parse::<u64>() {
                                    current_champion = Some(Champion {
                                        pid,
                                        score,
                                        rss,
                                        match_index,
                                        start_time: st,
                                        // Name removed to avoid allocation
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        // Post-Loop: If strategy was OOM Score, we might have 0 RSS in the champion.
        if let Some(ref mut champ) = current_champion {
            if champ.rss == 0 {
                if self
                    .read_file_into_buffer(&champ.pid.to_string(), "statm")
                    .is_ok()
                {
                    if let Ok(s) = std::str::from_utf8(&self.read_buffer) {
                        let mut parts = s.split_whitespace();
                        if let Some(_total) = parts.next() {
                            if let Some(res) = parts.next() {
                                if let Ok(pages) = res.parse::<u64>() {
                                    champ.rss = pages * self.page_size;
                                }
                            }
                        }
                    }
                }
            }
        }

        current_champion
    }

    fn read_file_into_buffer(&mut self, pid_str: &str, file: &str) -> std::io::Result<usize> {
        self.path_buffer.clear();
        write!(self.path_buffer, "/proc/{}/{}", pid_str, file).unwrap();

        let mut f = File::open(&self.path_buffer)?;

        // Zero-allocation read: reuse capacity
        self.read_buffer.clear();
        let capacity = self.read_buffer.capacity();

        // Safety: We treat the buffer as uninitialized (though it was 0-filled or has old data).
        // File::read will overwrite.
        unsafe {
            self.read_buffer.set_len(capacity);
        }

        let bytes_read = f.read(&mut self.read_buffer)?;

        unsafe {
            self.read_buffer.set_len(bytes_read);
        }

        Ok(bytes_read)
    }

    fn kill_process(&mut self, ctx: &RuntimeContext, victim: &Champion, name: &str) -> Option<u64> {
        let nix_pid = NixPid::from_raw(victim.pid as i32);

        // 1. Send SIGTERM
        if let Err(e) = kill(nix_pid, Signal::SIGTERM) {
            if e == nix::errno::Errno::ESRCH {
                logging::emit(&SentinelEvent::KillCandidateIgnored {
                    pid: victim.pid,
                    reason: "ESRCH (Already gone)".to_string(),
                });
                return Some(victim.rss);
            }
            logging::emit(&SentinelEvent::KillSequenceAborted {
                reason: format!("Failed to send SIGTERM to {}: {}", victim.pid, e),
            });
            return None;
        }

        thread::sleep(Duration::from_millis(ctx.sigterm_wait_ms));

        // 2. Verify Identity (PID Reuse Check)
        if self
            .read_file_into_buffer(&victim.pid.to_string(), "stat")
            .is_ok()
        {
            if let Ok(s) = std::str::from_utf8(&self.read_buffer) {
                if let Some((_before, after_comm)) = s.split_once(") ") {
                    if let Some(start_time_str) = after_comm.split_whitespace().nth(19) {
                        if let Ok(new_st) = start_time_str.parse::<u64>() {
                            if new_st != victim.start_time {
                                logging::emit(&SentinelEvent::KillCandidateIgnored {
                                    pid: victim.pid,
                                    reason: "PID Reuse detected during wait".to_string(),
                                });
                                return Some(victim.rss);
                            }
                        }
                    }
                }
            }
        } else {
            // Process GONE
            logging::emit(&SentinelEvent::KillExecuted {
                pid: victim.pid,
                process_name: name.to_string(),
                strategy: "SIGTERM".to_string(),
                rss_freed: victim.rss,
            });
            return Some(victim.rss);
        }

        // 3. SIGKILL
        if let Err(e) = kill(nix_pid, Signal::SIGKILL) {
            logging::emit(&SentinelEvent::KillSequenceAborted {
                reason: format!("Failed to send SIGKILL to {}: {}", victim.pid, e),
            });
            return None;
        }

        logging::emit(&SentinelEvent::KillExecuted {
            pid: victim.pid,
            process_name: name.to_string(),
            strategy: "SIGKILL".to_string(),
            rss_freed: victim.rss,
        });
        Some(victim.rss)
    }
}
