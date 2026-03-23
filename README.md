# RAM Sentinel 🛡️

🚧 [Pre-release](#roadmap) — Contributions and Feedback welcome!

**The Surgical Memory Guardian for Linux Desktops.**

> *Stop nuking my entire browser just  because I'm opening too many tabs!*

[![Build Status](https://img.shields.io/github/actions/workflow/status/benedictjohannes/ram-sentinel/releases.yml)](https://github.com/benedictjohannes/ram-sentinel/actions)
[![Crates.io](https://img.shields.io/crates/v/ram-sentinel)](https://crates.io/crates/ram-sentinel)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT) 

**ram-sentinel** is a userspace OOM (Out-of-Memory) prevention daemon designed specifically for modern workstations. Unlike `earlyoom` or `nohang` which often act as blunt instruments (killing your heaviest app, usually your Browser or IDE), `ram-sentinel` uses **Surgical Cmdline Process Targeting** and **Pressure Stall Information (PSI)** to surgically remove specific low-value targets (like browser renderer tabs) before threatening your main workflow.

It runs as a standard user (`systemd --user`), requires no root privileges, and talks to you via desktop notifications. In my desktop, it takes up <5MB RSS.

## 🚀 Why You Need This

If you are tired of your system freezing for 30 seconds before suddenly closing your entire game or browser, this tool is for you.

| Feature         | Standard OOM Killer (systemd-oomd)                           | RAM Sentinel                                                               |
| :-------------- | :----------------------------------------------------------- | :------------------------------------------------------------------------- |
| **Targeting**   | Kills the parent process (Largest RSS). **Bye bye Browser.** | Targeted snipe. Kills `type=renderer` tabs first. **Keeps Browser alive.** |
| **Trigger**     | RAM Full (Too late) or blunt heuristic.                      | **PSI (Pressure)**: Acts when system *stutters*, not just when full.       |
| **Safety**      | Can kill PID that was just reused (Race condition).          | **PID Identity Check**: Verifies process start time before killing.        |
| **UX**          | Silent death.                                                | **Notifications**: Warns you *before* killing. Tells you *what* it killed. |
| **Permissions** | Root required.                                               | **Userspace**: Runs as YOU.                                                |

---

## ⚡ Quick Start (Sane Defaults)

You don't have to configure anything to get started. `ram-sentinel` ships with **[Sane Defaults](#sane-defaults-🛡️)** that work for 99% of desktop users.

### Installation

#### Using Cargo

```bash
cargo install ram-sentinel
```

#### Binary download

```bash
curl -L https://github.com/benedictjohannes/ram-sentinel/releases/latest/download/ram-sentinel-linux-amd64 -o ram-sentinel && \
chmod +x ram-sentinel && \
sudo mv ram-sentinel /usr/local/bin/
```


### Run immediately

```bash
ram-sentinel
```

### Enable as a Service (Recommended)

Generate a systemd unit and enable it:

```bash
# Preview the unit file 
ram-sentinel --print-systemd-user-unit

# Install and Enable
mkdir -p ~/.config/systemd/user/
ram-sentinel --print-systemd-user-unit ~/.config/systemd/user/ram-sentinel.service
# edit if necessary
systemctl --user daemon-reload
systemctl --user enable --now ram-sentinel
```

-----

## ⚙️ Configuration

`ram-sentinel` looks for a config file in `$XDG_CONFIG_HOME/ram-sentinel.toml` (usually `~/.config/ram-sentinel.toml`).

### 🌟 Recommended Configuration

*Use this if you want the "Anti-Freeze" experience.* This enables the PSI monitor to kill runaway processes when the system starts thrashing (lagging), even if you technically have free RAM.

```toml
[ram]
warn_min_free_bytes = "500M"
# Safety net: Kill if RAM drops below 5%
kill_min_free_percent = 5.0 

[psi]
# Warn when system feels "heavy" (stuttering)
warn_max_percent = 40.0
# EMERGENCY: Kill when system freezes (mouse lag/thrashing)
kill_max_percent = 85.0
amount_to_free = "400M"

check_interval_ms = 1000
warn_reset_ms = 30000
sigterm_wait_ms = 2500
ignore_names = [
  # Broad matches to protect your shell/environment
  "kwin",
  "plasma",
  "gnome-shell",
  "sshd"
]
kill_targets = [
  # TIER 1 PRIORITY: The Expendables
  # These are killed FIRST.
  "type=renderer",       # Chrome/Electron tabs
  "-contentproc",        # Firefox tabs
  "^/usr/bin/node"      # Strict prefix match for processes you want to target first, e.g. for local node scripts
]
kill_strategy = "highest_oom_score"
```

### Sane Defaults 🛡️

If no config file is found, `ram-sentinel` loads this configuration automatically. It is conservative and focuses on preventing hard lockups.

```toml
[ram]
warn_min_free_percent = 10.0
kill_min_free_percent = 5.0

[psi]
# PSI disabled by default to be safe

check_interval_ms = 1000
warn_reset_ms = 30000
sigterm_wait_ms = 5000
kill_targets = [
  "type=renderer",
  "-contentproc"
]
ignore_names = []
kill_strategy = "highest_oom_score"
```

### 📖 Full Configuration Reference

Detailed explanation of every available option.

```toml
# --- RAM LIMITS ---
# Triggers if Available RAM falls below these values.
# NOTE: If 'bytes' is set, it OVERRIDES 'percent'.
[ram]
warn_min_free_bytes = "1G"      # Warn if < 1GB free
warn_min_free_percent = 10.0  # (Ignored if bytes is set)
kill_min_free_bytes = "250M"    # Kill if < 250MB free
kill_min_free_percent = 5.0   # (Ignored if bytes is set)

# --- SWAP LIMITS ---
# Same logic as RAM.
[swap]
warn_min_free_percent = 20.0
kill_min_free_percent = 5.0

# --- PSI (PRESSURE STALL INFORMATION) ---
# Requires Linux Kernel 4.20+ with CONFIG_PSI=y
# "Pressure" = % of time tasks are stalled waiting for memory.
[psi]
warn_max_percent = 40.0      # Warn if system is stuttering (40% pressure)
kill_max_percent = 90.0      # Kill if system is frozen (90% pressure)
amount_to_free = "500M"        # If triggered, kill processes until 500MB is freed

# --- TIMING ---
check_interval_ms = 1000       # How often to poll system stats
warn_reset_ms = 30000          # Don't spam notifications more than every 30s
sigterm_wait_ms = 3000         # Wait 3s after SIGTERM before sending SIGKILL

# --- TARGETING STRATEGY ---
# 1. Regex: "/pattern/" matches Name or Command Line
# 2. Prefix: "^string" matches START of Command Line
# 3. Literal: "string" matches substring of Name
kill_targets = [
  "type=renderer",           # Priority 1: Browser tabs
  "/npm start/",             # Priority 2: NPM scripts
  "^/usr/bin/python"        # Priority 3: Python scripts
]

ignore_names = [
  "^Xorg",                   # Never kill Xorg
  "/wayland/"                # Never kill Wayland compositors
]

# Strategies: 'highest_oom_score' (recommended) or 'largest_rss'
kill_strategy = "highest_oom_score"
```

-----

## 🧠 Design Philosophy

`ram-sentinel` is built on the **Safety First** doctrine.

1.  **Priority Queues:** We define a priority system for processes. `kill_targets` are "Second Class Citizens"—they are always sacrificed first. Your main apps are only touched if shedding the expendables didn't solve the memory crisis.
2.  **Identity Verification:** Before sending the final `SIGKILL`, the sentinel verifies that the PID's `create_time` matches the victim it selected 3 seconds ago. This prevents the "PID Reuse" race condition where a guardian accidentally kills a brand new process that grabbed the dead victim's PID.
3.  **Strict Override:** Configuration follows a "Manual Override" logic. If you set a specific Byte limit (`500MB`), the vague Percentage limit (`5%`) is ignored. You get exactly what you ask for.

> `ram-sentinel` is heavily inspired by the excellent [`earlyoom`](https://github.com/rfjakob/earlyoom), implementing many features I wished it had (like surgical process targeting and fine grained tuning). For a deeper dive into the architectural decisions, see [GEMINI.md](GEMINI.md).

### Roadmap

We're in the exciting early phase. I've used ram-sentinel on my CachyOS KDE desktop myself and it appears to be solid. But expect refinements and breaking changes before 1.0. 

> Feedback, issues, and PRs are **very** welcome! Contributors wanted. 👋

#### 1. Comprehensive Integration Testing Framework

We deliberately skip traditional unit tests—mocking the wild west of `/proc`, PSI, and real-world process chaos just breeds false confidence.

Instead, we're building a full end-to-end behavioral testing suite:
- Runs the **exact release binaries** in a clean, reproducible environment (e.g., a minimal Ubuntu VM via virsh with 2 CPUs / 4GB RAM—the perfect choke point).
- Includes a **coordinator** to orchestrate scenarios and a **troublemaker** to simulate realistic culprits (sleeping renderer tabs, RAM-hungry spikes, mixed workloads).
- Parses structured logs, monitors live PSI/meminfo, and asserts **surgical precision**: "Did it snipe only the lazy tabs without touching the IDE?"

This gives us rock-solid, real-world proof that ram-sentinel delivers on its promises—no hype, just results. Conceptual details in [TestingFramework.md](TestingFramework.md).

#### 2. Full System-Level Daemon Mode

Once the testing framework is battle-hardened, we'll expand beyond userspace:

- Optional **root mode** as a proper system service.
- New CLI flag: `--listen [socket-path]` for the userspace daemon - lets the root daemon push notifications to your user session (so desktop pop-ups still work seamlessly).
- **Cgroup v2 awareness**: Layer surgical targeting on top of cgroup pressure metrics for better scoping in containerized/mixed setups. Inspired by tools like `systemd-oomd`—thanks for the blueprint!

The goal? Make ram-sentinel the go-to guardian for all things Linux: desktops, workstations, and servers. Okay, well, maybe not Android 😖