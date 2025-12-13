mod config;
mod config_error;
mod killer;
mod monitor;
mod psi;
mod system;
mod utils;

use clap::Parser;
use env_logger::Env;
use log::{debug, error, info, warn};
use notify_rust::Notification;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::exit;
use std::thread::sleep;
use std::time::Duration;
use std::sync::atomic::{AtomicBool, Ordering};
use nix::sys::signal::{signal, SigHandler, Signal};

use crate::config::{Config, MemoryConfigParsed, RuntimeContext};
use crate::killer::Killer;
use crate::monitor::{KillReason, Monitor, MonitorStatus};
use crate::system::get_systemd_unit;

static RUNNING: AtomicBool = AtomicBool::new(true);

extern "C" fn handle_shutdown_signal(_: i32) {
    RUNNING.store(false, Ordering::SeqCst);
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[arg(long, value_name = "FILE")]
    config: Option<PathBuf>,

    #[arg(long)]
    no_kill: bool,

    #[arg(long, value_name = "FILE", num_args(0..=1), default_missing_value = "-")]
    print_config: Option<PathBuf>,

    #[arg(long, value_name = "FILE", num_args(0..=1), default_missing_value = "-")]
    print_systemd_user_unit: Option<PathBuf>,
}

fn handle_output(path_arg: Option<PathBuf>, content: &str) {
    if let Some(path) = path_arg {
        // Check for '-' to represent stdout piping
        if path.to_string_lossy() == "-" {
            println!("{}", content);
        } else {
            info!("Writing content to file: {:?}", path);
            match fs::File::create(&path).and_then(|mut file| file.write_all(content.as_bytes())) {
                Ok(_) => debug!("Successfully wrote to {:?}", path),
                Err(e) => {
                    error!("Error writing to file {:?}: {}", path, e);
                    exit(1);
                }
            }
        }
        exit(0);
    }
}

fn main() {
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();
    
    // Register signal handlers
    unsafe {
        let handler = SigHandler::Handler(handle_shutdown_signal);
        if let Err(e) = signal(Signal::SIGTERM, handler) {
            error!("Failed to register SIGTERM handler: {}", e);
        }
        if let Err(e) = signal(Signal::SIGINT, handler) {
            error!("Failed to register SIGINT handler: {}", e);
        }
    }

    let args = Cli::parse();

    // --- Handle Utility Flags ---
    if args.print_systemd_user_unit.is_some() {
        let unit_content: String = get_systemd_unit();
        handle_output(args.print_systemd_user_unit, &unit_content);
        return;
    }
    if args.print_config.is_some() {
        let defaults = Config::sane_defaults();
        let yaml_content = serde_yaml::to_string(&defaults)
            .expect("FATAL: Failed to serialize default configuration");
        handle_output(args.print_config, &yaml_content);
        return;
    }

    // Normal startup
    let ctx = match Config::load(args.config) {
        Ok(c) => c,
        Err(e) => {
            error!("{}", e);
            exit(e.exit_code());
        }
    };

    run_loop(ctx, args.no_kill);
}

fn run_loop(ctx: RuntimeContext, no_kill: bool) {
    let mut monitor = Monitor::new();
    let mut killer = Killer::new();

    info!(
        "ram-sentinel started. Interval: {}ms",
        ctx.check_interval_ms
    );

    while RUNNING.load(Ordering::SeqCst) {
        match monitor.check(&ctx) {
            MonitorStatus::Normal => {
                debug!("Status: Normal");
            }
            MonitorStatus::Warn(msg) => {
                warn!("{}", msg);
                send_notification("Low Memory Warning", &msg, "dialog-warning");
            }
            MonitorStatus::Kill(reason) => {
                let reason_desc = format!("{:?}", reason);
                error!("Kill Triggered: {}", reason_desc);

                if no_kill {
                    info!("--no-kill active. Skipping kill sequence.");
                } else {
                    let amount_needed = match reason {
                        KillReason::PsiPressure(_, amount) => Some(amount),
                        KillReason::LowMemory(_) => {
                            if let Some(config) = &ctx.ram {
                                monitor.refresh_memory();
                                calc_needed(config, monitor.get_system().available_memory(), monitor.get_system().total_memory())
                            } else {
                                None
                            }
                        }
                        KillReason::LowSwap(_) => {
                            if let Some(config) = &ctx.swap {
                                monitor.refresh_memory();
                                calc_needed(config, monitor.get_system().free_swap(), monitor.get_system().total_swap())
                            } else {
                                None
                            }
                        }
                    };

                    if amount_needed.is_none() {
                        info!("Kill sequence aborted: calculated needed memory is 0 (system likely recovered).");
                        continue;
                    }

                    killer.kill_sequence(&ctx, &reason_desc, amount_needed);
                }
            }
        }

        sleep(Duration::from_millis(ctx.check_interval_ms));
    }
    
    info!("Exiting ram-sentinel.");
}

fn calc_needed(config: &MemoryConfigParsed, current_free: u64, total: u64) -> Option<u64> {
    let mut target = 0;

    if let Some(bytes) = config.kill_min_free_bytes {
        target = bytes;
    } else if let Some(percent) = config.kill_min_free_percent {
        target = (total as f64 * (percent as f64 / 100.0)) as u64;
    }

    if target > current_free {
        Some(target - current_free)
    } else {
        None
    }
}

fn send_notification(title: &str, body: &str, icon: &str) {
    let _ = Notification::new()
        .summary(title)
        .body(body)
        .icon(icon)
        .show();
}
