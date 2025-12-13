# RAM Sentinel - Architecture & Design Specification

**Role:** You are a Senior Rust Systems Engineer and Linux Kernel Specialist.
**Objective:** Construct `ram-sentinel`, a robust userspace memory guardian for Linux.

## 🎯 Project Goals

1.  **Userspace First:** Run as a standard user (no root required) to manage user-owned processes.
2.  **Safety & Stability:** Use `nix` for safe signal handling. Validate PID identity (via `create_time`) to prevent race conditions.
3.  **Intelligent Shedding:** Identify and kill "low-value" targets (e.g., specific browser tabs) before killing main applications.
4.  **Universal Metrics:** Monitor RAM, Swap, and Kernel PSI (Pressure Stall Information).
5.  **Feedback:** Notify the user via desktop notifications (D-Bus) before and after actions.

---

## 🛠️ Technical Specification

**Language:** Rust (2021 Edition)
**Key Crates:**
* `sysinfo` (0.30+): Process/Memory stats.
* `nix`: Signal handling (`SIGTERM`, `SIGKILL`).
* `serde`, `serde_yaml`: Strict configuration parsing.
* `regex`: Pattern matching.
* `byte-unit` (5.0+): Parsing "1GB", "500MB".
* `notify-rust`: Desktop notifications.
* `clap`: CLI argument parsing.

### 1. Configuration Architecture

The system uses a **Strict Priority** model for configuration.

* **Logic:** Explicit byte limits (e.g., `killMinFreeBytes`) always **override** percentage-based heuristics. If a byte limit is set, the percentage limit is ignored for that metric.
* **Validation:**
    * Fail fast (Exit Code 2-11) on invalid configs.
    * Ensure intervals are sane (100ms - 300s).
    * Pre-compile all regex patterns during load.

### 2. Targeting Logic (`killTargets` & `ignoreNames`)

The system identifies processes using a "Hit List" strategy.

**Matching Rules:**
1.  **Regex:** If string is enclosed in `/.../` (e.g., `/firefox-bin/`), treat as Regex. Check against Name and Command Line.
2.  **Prefix:** If string starts with `^` (e.g., `^/usr/lib/electron`), matches ONLY the start of the `cmd_line`.
3.  **Substring:** Otherwise, simple substring match against Name or Command Line.

**Priority Queue:**
* `killTargets` is an ordered list.
* Index 0 has the **highest kill priority**.
* Candidates matching early entries in `killTargets` are selected for termination before those matching later entries.
* General processes (non-matches) are only targeted if no `killTargets` are found.

### 3. Monitoring State Machine (`monitor.rs`)

The sensor loop checks metrics in strict order of urgency:

1.  **Kill Triggers:**
    * **RAM Hard Limit:** (Available < Limit).
    * **Swap Hard Limit:** (Free < Limit).
    * **PSI Pressure:** (Pressure % > `killMaxPercent`).
    * *Action:* Immediately enter Kill Sequence.

2.  **Warning Triggers:**
    * Check thresholds for RAM -> Swap -> PSI.
    * *Action:* Send notification (debounced by `warnResetMs`).

### 4. The Kill Sequence (`killer.rs`)

**Strategy:** "Safety First, Double Tap"
1.  **Discovery:** Scan processes. Filter out `ignoreNames`, Self, and Root processes (unless running as root).
2.  **Sorting:**
    * Primary Sort: `killTarget` match index (ascending).
    * Secondary Sort: `KillStrategy` (RSS size or OOM Score).
3.  **Execution:**
    * Send `SIGTERM`.
    * **Wait** `sigtermWaitMs` (give app time to save/close).
    * **Verify Identity:** Check if PID still exists AND `create_time` matches the recorded victim (prevents PID reuse attacks).
    * If running & verified: Send `SIGKILL`.
    * *Loop:* Continue killing until the calculated memory deficit is recovered.

---

## 📚 Development Protocol

**Phase 1: The Scaffold**
* Setup `main.rs`, `config.rs`, `monitor.rs`, `killer.rs`, `system.rs`.
* Implement `get_systemd_unit()` to generate a user service file.
* *Note:* Add comments in the unit file explaining that `Nice` and `OOMScoreAdjust` require capabilities configuration for non-root users.

**Phase 2: Configuration (`config.rs`)**
* Implement `RuntimeContext` to hold parsed config and compiled Regex.
* Use `byte-unit` to parse human-readable strings.
* Implement strict validation (fail on negative percents, 0 byte targets).

**Phase 3: The Sensor (`monitor.rs`)**
* Implement PSI reading from `/proc/pressure/memory`.
* Ensure the trigger logic respects the "Bytes > Percent" priority.

**Phase 4: The Executioner (`killer.rs`)**
* Implement the logic for `cmd_line` targeting.
* Implement the sorting logic: `Explicit Targets > General Population`.
* Implement the PID reuse check using `sysinfo` start times.

**Phase 5: The Interface (`main.rs`)**
* Add CLI flags: `--config`, `--no-kill` (dry run), `--print-default-config`.