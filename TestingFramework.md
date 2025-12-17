# Testing Framework Design

`ram-sentinel` thrives in the chaotic reality of Linux desktops: ever-changing `/proc` entries, PSI averaging windows, process exit races, cgroup quirks, and the kernel's own OOM reaper lurking in the background. Traditional unit tests mock away the very mess we need to master, leading to brittle, feel-good coverage that might crumble in real pressure.

Instead, we prove correctness through **comprehensive, reproducible end-to-end integration testing** in a genuine OS environment. The mission: ironclad evidence that ram-sentinel always snipes the *right* culprits, spares your workflow, and never causes collateral damage, **at the right time**.

## Core Philosophy

- Test the **exact release binaries** that users will run—no mocks, no stubs, no excuses.
- Simulate realistic workloads: sleeping browser tabs, steady RAM hogs, spiky bursts, mixed apps.
- Execute in an isolated, minimal VM for predictable, controllable memory pressure.
- Rely on **structured JSON logging** as the oracle for assertions and timelines.
- Fully automate orchestration, execution, and pass/fail reporting.

This catches the sneaky regressions unit tests miss: PSI lag, cmdline reads during process death, scan-timing races, and more.

## Repository Structure (Planned Workspace)

```
/
├── Cargo.toml             # Workspace root
├── ram-sentinel/          # Main crate (published)
│   ├── Cargo.toml
│   ├── src/main.rs        # Daemon binary entrypoint
│   ├── src/lib.rs         # Exposes events.rs + core types for testing
│   └── ...
├── testing-framework/     # Logical grouping. Internal crates, not published
│   ├── troublemaker/
│   │   ├── Cargo.toml
│   │   └── src/main.rs    # Configurable real-world workloads simulation
│   ├── orchestrator/
│   │   ├── Cargo.toml
│   │   └── src/main.rs    # The brain: scenario runner + assertions
│   └── shared/
│       ├── Cargo.toml
│       └── src/lib.rs     # Shared logging types, enums, and troublemaker config contracts
└── TestingFramework.md    # This doc
```

## Components

### 1. troublemaker
**A configurable binary that impersonates real-world memory offenders**

- Launched multiple times with varying behaviors via ENV vars and symlink tricks (e.g., symlink `/usr/bin/chromium --type=renderer` → troublemaker binary).
- Behaviors controlled entirely by environment variables.
- Example modes:
  - `SLEEPY_TAB`: Allocate big RSS, then idle or madvise pages away.
  - `LINEAR_RAMP`: Steady, relentless mmap/mlock growth.
  - `SPIKY`: Random allocation bursts + CPU activity.
  - `ACTIVE_RENDERER`: Hold memory + periodic light work to mimic a busy tab.
  - `MAIN_BROWSER`: Spawn child "renderer" troublemakers for hierarchy simulation.
- Emits its own structured JSON logs for perfect correlation with sentinel events.

### 2. orchestrator

**The conductor of chaos**

- Spawns the ram-sentinel daemon with custom config overrides.
- Launches fleets of troublemaker instances (mixed behaviors, counts, cmdlines).
- Real-time tails JSON logs from sentinel and all troublemakers.
- Continuously samples host `/proc/meminfo` and `/proc/pressure/memory`.
- Executes predefined **scenarios** (examples below).
- Performs strict assertions:
  - Which PIDs were killed, and in what order?
  - Did kills happen promptly after threshold breach?
  - Were innocents spared?
  - Did warnings/notifications fire correctly?
- Outputs a clear pass/fail report for each scenario ran.

### 3. shared

A tiny internal crate holding the exact `enum`s/`struct`s for logging events and troublemaker configuration. Both orchestrator and troublemaker depend on it—single source of truth.

## Example Scenarios

These will be scripted and automated:

1. **Sleeping Tabs Massacre**
   20 sleepy renderer tabs + 1 active main browser + running IDE.  
   *Expected*: Only sleepy renderers die (oldest/lowest-priority first); main process and IDE untouched.

2. **Active vs Inactive Prioritization**  
   10 active renderers + 10 sleepy ones under escalating pressure.  
   *Expected*: Sleepers sacrificed first; actives spared until absolutely necessary.

3. **Mixed Workload Protection**
   Heavy compiler build + IDE + 30 browser tabs.  
   *Expected*: Browser renderers go down heroically before touching build processes.

4. **Race Condition Hunt**
   Troublemakers exiting rapidly during sentinel scans.  
   *Expected*: No crashes, no attempts to kill dead PIDs, graceful handling.

## Execution Environment

- VM (e.g., Ubuntu via `virsh`/libvirt)
- Fixed specs: 2 vCPU, 4 GB RAM. The ideal reproducable pressure cooker.
- Start from a clean snapshot for perfect reproducibility.
- Binaries SCP'd or pre-installed; orchestrator drives everything.

This framework will let us evolve ram-sentinel fearlessly: adding new patterns, root mode, cgroup support, while upholding the unbreakable promise of **surgical, reliable OOM prevention**.

Feedback, ideas, and contributions to build this out are **very** welcome! 🔥