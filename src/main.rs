mod config;
mod monitor;
mod killer;
mod system;

use clap::Parser;
use std::path::PathBuf;
use std::thread::sleep;
use std::time::Duration;
use log::{info, warn, error, debug};
use notify_rust::Notification;
use std::fs;
use std::io::Write;
use std::process::exit;

use crate::config::{Config, RuntimeContext, MemoryConfig};
use crate::monitor::{Monitor, MonitorStatus, KillReason};
use crate::killer::Killer;
use crate::system::get_systemd_unit;
use byte_unit::Byte;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[arg(long, value_name = "FILE")]
    config: Option<PathBuf>,

    #[arg(long)]
    no_kill: bool,

    #[arg(long, value_name = "FILE")]
    print_config: Option<PathBuf>,

    #[arg(long)]
    print_systemd_user_unit: bool,
}

fn main() {
    env_logger::init();
    let args = Cli::parse();

    // Handle Utility Flags
    if args.print_systemd_user_unit {
        println!("{}", get_systemd_unit());
        return;
    }

    if let Some(path) = args.print_config {
        let defaults = Config::sane_defaults();
        let yaml = serde_yaml::to_string(&defaults).expect("Failed to serialize config");
        
        let mut file = fs::File::create(&path).unwrap_or_else(|e| {
             eprintln!("Error creating file {:?}: {}", path, e);
             exit(1);
        });
        file.write_all(yaml.as_bytes()).unwrap();
        return;
    }

    // Normal startup
    let ctx = Config::load(args.config);
    
    run_loop(ctx, args.no_kill);
}

fn run_loop(ctx: RuntimeContext, no_kill: bool) {
    let mut monitor = Monitor::new();
    let mut killer = Killer::new();

    info!("ram-sentinel started. Interval: {}ms", ctx.config.check_interval_ms);

    loop {
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
                         KillReason::LowMemory(available) => {
                             if let Some(config) = &ctx.config.ram {
                                  calc_needed(config, available, monitor.get_system().total_memory())
                             } else { None }
                         }
                         KillReason::LowSwap(free) => {
                             if let Some(config) = &ctx.config.swap {
                                  calc_needed(config, free, monitor.get_system().total_swap())
                             } else { None }
                         }
                     };
                     
                     killer.kill_sequence(&ctx, &reason_desc, amount_needed);
                }
            }
        }
        
        sleep(Duration::from_millis(ctx.config.check_interval_ms));
    }
}

fn calc_needed(config: &MemoryConfig, current_free: u64, total: u64) -> Option<u64> {
    let mut target = 0;
    
    if let Some(bytes_str) = &config.kill_min_free_bytes {
        if let Ok(bytes) = Byte::parse_str(bytes_str, true) {
             target = bytes.as_u64();
        }
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