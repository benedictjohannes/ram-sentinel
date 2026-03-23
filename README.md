# RAM Sentinel 🛡️

🚧 [Pre-release](#roadmap) — Contributions and Feedback welcome!

**The Surgical Memory Guardian for Linux Desktops.**

> *Stop nuking my entire browser just  because I'm opening too many tabs!*

[![Build Status](https://img.shields.io/github/actions/workflow/status/benedictjohannes/ram-sentinel/releases.yml)](https://github.com/benedictjohannes/ram-sentinel/actions)
[![Crates.io](https://img.shields.io/crates/v/ram-sentinel)](https://crates.io/crates/ram-sentinel)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT) 

**ram-sentinel** is a userspace OOM (Out-of-Memory) prevention daemon designed specifically for modern workstations. Unlike `earlyoom` or `nohang` which often act as blunt instruments (killing your heaviest app, usually your Browser or IDE), `ram-sentinel` uses **Surgical Process Targeting** by checking both *cmdline* and *cgroups* to surgically remove specific low-value targets (like browser renderer tabs) before threatening your main workflow. We also support preemptive system stutter prevention by leveraging **Pressure Stall Information (PSI)**.

It runs as a standard user (`systemd --user`), requires no root privileges, and talks to you via desktop notifications. In my desktop, it takes up <5MB RSS.

## 🚀 Why You Need This

If you are tired of your system freezing for 30 seconds before suddenly closing your entire game or browser, this tool is for you.

| Feature         | Standard OOM Killer (systemd-oomd)                           | RAM Sentinel                                                               |
| :-------------- | :----------------------------------------------------------- | :------------------------------------------------------------------------- |
| **Targeting**   | Kills the parent process (Largest RSS). **Bye bye Browser.** | Targeted snipe. Kills `type=renderer` tabs first, or targets by **Cgroup v2** path. **Keeps Browser alive.** |
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

For the best "Anti-Freeze" experience, we recommend using the provided [config.example.toml](config.example.toml) as a starting point. It includes pre-tuned settings for:
- **Surgical Targeting**: Kills browser renderers and dev tools before touching your IDE.
- **PSI (Pressure Stall Information)**: Acts when the system starts stuttering, preventing hard lockups.
- **Ignore List**: Protects your desktop environment and essential background services.

To use it:
```bash
cp config.example.toml ~/.config/ram-sentinel.toml
# edit to your liking
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
allow_kill_outside_targets = false
ignore_names = ["ram-sentinel"]
kill_strategy = "highest_oom_score"
```

### 📖 Full Configuration Reference

For a detailed explanation of every available option and advanced targeting examples, please refer to the fully commented **[config.example.toml](config.example.toml)**.

### 🔍 Validating your Configuration

You can verify your configuration file (or the default one) using the `--check-config` flag. This will load the config, validate all values and regex patterns, and print the effective configuration that `ram-sentinel` will use.

```bash
# Check your default config
ram-sentinel --check-config

# Check a specific config file
ram-sentinel --config my-test-config.toml --check-config
```

-----

## 🧠 Design Philosophy

`ram-sentinel` is built on the **Safety First** doctrine.

1.  **Priority Queues:** We define a priority system for processes. `kill_targets` are "Second Class Citizens"—they are always sacrificed first. Your main apps are only touched if shedding the expendables didn't solve the memory crisis.
2.  **Identity Verification:** Before sending the final `SIGKILL`, the sentinel verifies that the PID's `create_time` matches the victim it selected 3 seconds ago. This prevents the "PID Reuse" race condition where a guardian accidentally kills a brand new process that grabbed the dead victim's PID.
3.  **Strict Override:** Configuration follows a "Manual Override" logic. If you set a specific Byte limit (`500MB`), the vague Percentage limit (`5%`) is ignored. You get exactly what you ask for.

> `ram-sentinel` is heavily inspired by the excellent [`earlyoom`](https://github.com/rfjakob/earlyoom) and the modern [`systemd-oomd`](https://www.freedesktop.org/software/systemd/man/systemd-oomd.service.html), implementing many features I wished they had (like surgical dual-layer targeting and fine grained userspace notification). For a deeper dive into the architectural decisions, see [GEMINI.md](GEMINI.md).

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

- **Root Mode**: Optional system service for managing all system processes.
- **Notification Proxy**: Helper functionality to push notifications from the root daemon to user sessions.

The goal? Make ram-sentinel the go-to guardian for all things Linux: desktops, workstations, and servers. Okay, well, maybe not Android 😖