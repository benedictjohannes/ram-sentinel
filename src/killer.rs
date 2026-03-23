use crate::config::{KillStrategy, RuntimeContext};
use crate::events::SentinelEvent;
use crate::logging;
use nix::sys::signal::{Signal, kill};
use nix::unistd::{Pid as NixPid, SysconfVar, Uid, sysconf};
use std::fmt::Write; // For writing to path_buffer / reason_buffer
use std::fs::{self, File};
use std::io::{self, Read};
use std::thread;
use std::time::Duration;

pub struct Killer {
    // Buffers for zero-allocation logic
    read_buffer: Vec<u8>,
    path_buffer: String,
    name_buffer: String,
    /// Pre-allocated scratch space for dynamic abort/ignore reason strings.
    /// Formatted with `write!` before being passed as `&str` to SentinelEvent.
    reason_buffer: String,
    page_size: u64,
    /// Buffer to track processes that failed to kill during a single sequence.
    /// This prevents an unkillable process from stalling the guardian forever.
    failed_pids: [u32; 64],
    /// Count of failed kills in the current sequence.
    failed_count: usize,
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
            name_buffer: String::with_capacity(128),
            reason_buffer: String::with_capacity(256),
            page_size,
            failed_pids: [0; 64],
            failed_count: 0,
        }
    }

    pub fn kill_sequence(&mut self, ctx: &RuntimeContext, mut amount_needed: Option<u64>) {
        // Zero out the skip buffer and counter at the start of each sequence
        self.failed_count = 0;
        self.failed_pids.fill(0);

        loop {
            self.reason_buffer.clear();
            // 1. Scan /proc and find the best candidate ("The Champion")
            let champion_opt = self.find_champion(ctx);

            let Some(champion) = champion_opt else {
                self.log_abort(format_args!("No eligible kill candidates found!"));
                break;
            };

            // Fetch name for logging (on-demand, after scan loop)
            self.get_process_name(champion.pid);

            // Pass &name_buffer directly — zero copy.
            // NOTE: reason_buffer may also be populated below; they are separate fields so no aliasing.
            logging::emit(&SentinelEvent::KillCandidateSelected {
                pid: champion.pid,
                process_name: &self.name_buffer,
                score: champion.score,
                rss: champion.rss,
                match_index: champion.match_index,
            });

            // 2. Kill Logic
            // kill_process needs &mut self (for read_file_into_buffer in identity check),
            // so we must clone the name first. This single allocation happens only once
            // per kill iteration — outside the hot discovery loop.
            let name_owned = self.name_buffer.clone();

            match self.kill_process(ctx, &champion, &name_owned) {
                KillOutcome::Freed(freed_bytes) => {
                    if let Some(needed) = amount_needed {
                        if freed_bytes >= needed {
                            self.log_abort(format_args!(
                                "Freed {} bytes. Target reached.",
                                freed_bytes
                            ));
                            break;
                        } else {
                            amount_needed = Some(needed - freed_bytes);
                            // Continue to kill next candidate
                        }
                    } else {
                        // If no specific amount was requested, stop after one kill
                        break;
                    }
                }
                KillOutcome::PidReused => {
                    // The "victim" slot was already filled by a new process.
                    // This is not a failure — re-run find_champion to pick a fresh target.
                    logging::emit(&SentinelEvent::KillCandidateIgnored {
                        pid: champion.pid,
                        reason: "PID Reuse detected during wait — retrying.",
                    });
                    continue;
                }
                KillOutcome::Failed => {
                    if self.failed_count < self.failed_pids.len() {
                        self.failed_pids[self.failed_count] = champion.pid;
                        self.failed_count += 1;
                        
                        // We use the same reason allocation trick or just static strings to prevent allocations
                        logging::emit(&SentinelEvent::KillCandidateIgnored {
                            pid: champion.pid,
                            reason: "Kill failed with hard error. Skipping process.",
                        });
                        continue;
                    } else {
                        self.log_abort(format_args!(
                            "Failed to kill victim PID {} ({}). Max failures (64) reached. Aborting.",
                            champion.pid, name_owned
                        ));
                        break;
                    }
                }
            }
        }
    }

    fn get_process_name(&mut self, pid: u32) {
        self.name_buffer.clear();
        if self.read_file_into_buffer(pid, "comm").is_ok() {
            if let Ok(s) = std::str::from_utf8(&self.read_buffer) {
                self.name_buffer.push_str(s.trim());
            } else {
                self.name_buffer.push_str("unknown");
            }
        } else {
            self.name_buffer.push_str("unknown");
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
                let _ = write!(self.reason_buffer, "Failed to read /proc: {}", e);
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

                // Filter 3: Skip pids that failed to be killed earlier in this sequence
                let mut is_failed = false;
                for i in 0..self.failed_count {
                    if self.failed_pids[i] == pid {
                        is_failed = true;
                        break;
                    }
                }
                if is_failed {
                    continue;
                }

                // Filter 4: Ownership Check (if not root)
                if !is_root {
                    use std::os::unix::fs::MetadataExt;
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
                if !matches!(self.read_file_into_buffer(pid, "cmdline"), Ok(n) if n > 0) {
                    continue; // Skip empty cmdlines (kernel threads) or errors
                }

                // Replace nulls with spaces
                for b in self.read_buffer.iter_mut() {
                    if *b == 0 {
                        *b = 32;
                    }
                }

                // Strict allocation-free string ref
                let cmdline_str = match std::str::from_utf8(&self.read_buffer) {
                    Ok(s) => s,
                    Err(_) => continue,
                };

                // Check Ignored
                let mut ignored = false;
                for pat in &ctx.ignore_names_regex {
                    if pat.matches(cmdline_str) {
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
                    if pat.matches(cmdline_str) {
                        match_index = idx;
                        break;
                    }
                }

                // Priority Check: If we already have a champion with a better (lower) match index,
                // and this candidate has a worse (higher) index, skip early.
                if let Some(champ) = &current_champion {
                    if match_index > champ.match_index {
                        continue;
                    }
                }

                // B. Resource Check: Always read RSS to ensure it's captured while the process is live.
                let mut rss = 0;
                if self.read_file_into_buffer(pid, "statm").is_ok() {
                    if let Ok(s) = std::str::from_utf8(&self.read_buffer) {
                        let mut parts = s.split_whitespace();
                        if let (Some(_total), Some(res)) = (parts.next(), parts.next()) {
                            if let Ok(pages) = res.parse::<u64>() {
                                rss = pages * self.page_size;
                            }
                        }
                    }
                }

                // C. Calculate Score
                let score = match ctx.kill_strategy {
                    KillStrategy::LargestRss => rss,
                    KillStrategy::HighestOomScore => {
                        let mut s = 0;
                        if self.read_file_into_buffer(pid, "oom_score").is_ok() {
                            if let Ok(st) = std::str::from_utf8(&self.read_buffer) {
                                if let Ok(val) = st.trim().parse::<i32>() {
                                    s = val as u64;
                                }
                            }
                        }
                        s
                    }
                };

                // Final Comparison vs Current Champion.
                // At this point match_index <= champ.match_index (ensured by early skip above).
                // If equal index, only promote if this candidate has a strictly higher score.
                if let Some(champ) = &current_champion {
                    if match_index == champ.match_index && score <= champ.score {
                        continue;
                    }
                    // If match_index < champ.match_index: implicit promotion (fall through).
                }

                // D. Become the Champion (Read stat for Start Time)
                // Field 22 (starttime) in /proc/[pid]/stat. After splitting on ") " (which
                // skips past the comm field), the remaining fields are 0-indexed as:
                // 0=state, 1=ppid, ..., 19=starttime.
                if self.read_file_into_buffer(pid, "stat").is_ok() {
                    if let Ok(s) = std::str::from_utf8(&self.read_buffer) {
                        if let Some((_, after_comm)) = s.rsplit_once(") ") {
                            if let Some(start_time_str) = after_comm.split_whitespace().nth(19) {
                                if let Ok(st) = start_time_str.parse::<u64>() {
                                    current_champion = Some(Champion {
                                        pid,
                                        score,
                                        rss,
                                        match_index,
                                        start_time: st,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        current_champion
    }

    fn read_file_into_buffer(&mut self, pid: u32, file: &str) -> std::io::Result<usize> {
        self.path_buffer.clear();
        write!(self.path_buffer, "/proc/{}/{}", pid, file).unwrap();

        let mut f = File::open(&self.path_buffer)?;

        self.read_buffer.clear();
        let capacity = self.read_buffer.capacity();

        // Safety: Replaced unsafe set_len with safe resize which writes zeroes.
        self.read_buffer.resize(capacity, 0);

        let bytes_read = match f.read(&mut self.read_buffer) {
            Ok(n) => n,
            Err(e) => {
                self.read_buffer.clear();
                return Err(e);
            }
        };

        // Truncate to exactly what was read so all subsequent slice access is correct.
        self.read_buffer.truncate(bytes_read);

        Ok(bytes_read)
    }

    fn verify_identity(&mut self, pid: u32, expected_st: u64) -> IdentityCheck {
        match self.read_file_into_buffer(pid, "stat") {
            Ok(_) => {
                let verified = match std::str::from_utf8(&self.read_buffer) {
                    Ok(s) => s
                        .rsplit_once(") ")
                        .and_then(|(_, after_comm)| after_comm.split_whitespace().nth(19))
                        .and_then(|st_str| st_str.parse::<u64>().ok())
                        .map(|new_st| new_st == expected_st),
                    Err(_) => None,
                };

                match verified {
                    Some(true) => IdentityCheck::Match,
                    Some(false) => IdentityCheck::PidReused,
                    None => {
                        let _ = write!(
                            self.reason_buffer,
                            "Failed to verify process identity for PID {} (parse error).",
                            pid
                        );
                        IdentityCheck::Error
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => IdentityCheck::ProcessGone,
            Err(e) => {
                let _ = write!(
                    self.reason_buffer,
                    "Failed to verify process identity for PID {} ({}).",
                    pid, e
                );
                IdentityCheck::Error
            }
        }
    }

    fn kill_process(&mut self, ctx: &RuntimeContext, victim: &Champion, name: &str) -> KillOutcome {
        let nix_pid = NixPid::from_raw(victim.pid as i32);

        // 1. Initial Identity Check: Ensure it hasn't been reused since the scan
        match self.verify_identity(victim.pid, victim.start_time) {
            IdentityCheck::Match => {}
            IdentityCheck::PidReused => return KillOutcome::PidReused,
            IdentityCheck::ProcessGone => return KillOutcome::Freed(victim.rss),
            IdentityCheck::Error => return KillOutcome::Failed,
        }

        // 2. Send SIGTERM
        match kill(nix_pid, Signal::SIGTERM) {
            Ok(_) => {}
            Err(nix::errno::Errno::ESRCH) => {
                // Process already gone — treat as success (memory freed).
                logging::emit(&SentinelEvent::KillExecuted {
                    pid: victim.pid,
                    process_name: name,
                    strategy: "SIGTERM (immediate exit)",
                    rss_freed: victim.rss,
                });
                return KillOutcome::Freed(victim.rss);
            }
            Err(e) => {
                let _ = write!(
                    self.reason_buffer,
                    "Failed to send SIGTERM to {}: {}",
                    victim.pid, e
                );
                return KillOutcome::Failed;
            }
        }

        thread::sleep(Duration::from_millis(ctx.sigterm_wait_ms));

        // 3. Second Identity Check: Ensure it hasn't been reused during the SIGTERM wait
        match self.verify_identity(victim.pid, victim.start_time) {
            IdentityCheck::Match => {}
            IdentityCheck::PidReused => return KillOutcome::PidReused,
            IdentityCheck::ProcessGone => {
                // Process GONE — SIGTERM was enough.
                logging::emit(&SentinelEvent::KillExecuted {
                    pid: victim.pid,
                    process_name: name,
                    strategy: "SIGTERM",
                    rss_freed: victim.rss,
                });
                return KillOutcome::Freed(victim.rss);
            }
            IdentityCheck::Error => return KillOutcome::Failed,
        }

        // 4. SIGKILL
        match kill(nix_pid, Signal::SIGKILL) {
            Ok(_) => {}
            Err(nix::errno::Errno::ESRCH) => {
                // Process died between SIGTERM wait and SIGKILL — that's fine, memory is freed.
                logging::emit(&SentinelEvent::KillExecuted {
                    pid: victim.pid,
                    process_name: name,
                    strategy: "SIGTERM (late exit)",
                    rss_freed: victim.rss,
                });
                return KillOutcome::Freed(victim.rss);
            }
            Err(e) => {
                let _ = write!(
                    self.reason_buffer,
                    "Failed to send SIGKILL to {}: {}",
                    victim.pid, e
                );
                return KillOutcome::Failed;
            }
        }

        logging::emit(&SentinelEvent::KillExecuted {
            pid: victim.pid,
            process_name: name,
            strategy: "SIGKILL",
            rss_freed: victim.rss,
        });
        KillOutcome::Freed(victim.rss)
    }

    /// Internal helper to format a reason into `reason_buffer` and emit a `KillSequenceAborted` event.
    fn log_abort(&mut self, args: std::fmt::Arguments) {
        if self.reason_buffer.is_empty() {
            let _ = self.reason_buffer.write_fmt(args);
        }
        logging::emit(&SentinelEvent::KillSequenceAborted {
            reason: &self.reason_buffer,
        });
    }
}

enum IdentityCheck {
    Match,
    PidReused,
    ProcessGone,
    Error,
}

/// Result of a single kill attempt.
enum KillOutcome {
    /// Signal delivered; `u64` is the RSS we expect to be freed.
    Freed(u64),
    /// The target PID was reused by a new process before we could kill it.
    /// Caller should re-run find_champion instead of aborting.
    PidReused,
    /// A fatal error occurred (permission denied, etc.); caller should abort.
    Failed,
}
