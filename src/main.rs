mod config;
mod config_error;
mod events;
mod killer;
mod logging; // Added
mod monitor;
mod psi;
mod system;
mod utils;

use clap::Parser;

use nix::sys::signal::{SigHandler, Signal, signal};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::exit;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::sleep;
use std::time::Duration;

use crate::config::{Config, RuntimeContext};
use crate::events::{LogLevel, LogMode, SentinelEvent};
use crate::killer::Killer;
use crate::monitor::{Monitor, MonitorStatus};
use crate::system::get_systemd_unit; // Added

static RUNNING: AtomicBool = AtomicBool::new(true);

extern "C" fn handle_shutdown_signal(_: i32) {
    RUNNING.store(false, Ordering::SeqCst);
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Optional Path to configuration file
    #[arg(long, short = 'c', value_name = "FILE")]
    config: Option<PathBuf>,

    /// Optional Log Format. Defaults to "compact".
    #[arg(long, value_name = "LOG_FORMAT", default_value = "compact")]
    log_format: LogMode,

    /// Optional Log Level. Defaults to "info".
    #[arg(long, value_name = "LOG_LEVEL", default_value = "info")]
    log_level: LogLevel,

    /// Run in "Dry Run" mode. Monitors memory but does not kill any processes.
    #[arg(long)]
    no_kill: bool,

    /// Optional Path to print configuration to. Defaults to stdout.
    #[arg(long, value_name = "FILE", num_args(0..=1), default_missing_value = "-")]
    print_config: Option<PathBuf>,

    /// Optional Path to print systemd user unit to. Defaults to stdout.
    #[arg(long, value_name = "FILE", num_args(0..=1), default_missing_value = "-")]
    print_systemd_user_unit: Option<PathBuf>,
}

fn handle_output(path_arg: Option<PathBuf>, content: &str) {
    if let Some(path) = path_arg {
        // Check for '-' to represent stdout piping
        if path.to_string_lossy() == "-" {
            println!("{}", content);
        } else {
            logging::emit(&SentinelEvent::Message {
                level: LogLevel::Debug,
                text: format!("Writing content to file: {:?}", path),
            });
            match fs::File::create(&path).and_then(|mut file| file.write_all(content.as_bytes())) {
                Ok(_) => logging::emit(&SentinelEvent::Message {
                    level: LogLevel::Debug,
                    text: format!("Successfully wrote to {:?}", path),
                }),
                Err(e) => {
                    logging::emit(&SentinelEvent::Message {
                        level: LogLevel::Error,
                        text: format!("Error writing to file {:?}: {}", path, e),
                    });
                    exit(1);
                }
            }
        }
        exit(0);
    }
}

fn main() {
    let args = Cli::parse();

    logging::set_logging_mode(args.log_format);
    logging::set_logging_level(args.log_level);

    // Register signal handlers
    unsafe {
        let handler = SigHandler::Handler(handle_shutdown_signal);
        if let Err(e) = signal(Signal::SIGTERM, handler) {
            logging::emit(&SentinelEvent::Message {
                level: LogLevel::Error,
                text: format!("Failed to register SIGTERM handler: {}", e),
            });
        }
        if let Err(e) = signal(Signal::SIGINT, handler) {
            logging::emit(&SentinelEvent::Message {
                level: LogLevel::Error,
                text: format!("Failed to register SIGINT handler: {}", e),
            });
        }
    }

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

    let ctx = match Config::load(args.config) {
        Ok(c) => c,
        Err(e) => {
            logging::emit(&SentinelEvent::Message {
                level: LogLevel::Error,
                text: format!("Configuration Error: {}", e),
            });
            exit(e.exit_code());
        }
    };

    run_loop(ctx, args.no_kill);
}

fn run_loop(ctx: RuntimeContext, no_kill: bool) {
    let mut monitor = Monitor::new();
    let mut killer = Killer::new();

    logging::emit(&SentinelEvent::Startup {
        interval_ms: ctx.check_interval_ms,
    });

    while RUNNING.load(Ordering::SeqCst) {
        match monitor.check(&ctx) {
            MonitorStatus::Normal => {}
            MonitorStatus::Warn => {}
            MonitorStatus::Kill(event) => {
                logging::emit(&event);

                if no_kill {
                    logging::emit(&SentinelEvent::Message {
                        level: LogLevel::Info,
                        text: "--no-kill active. Skipping kill sequence.".to_string(),
                    });
                } else {
                    if let SentinelEvent::KillTriggered { amount_needed, .. } = &event {
                        if let Some(needed) = *amount_needed {
                            killer.kill_sequence(&ctx, Some(needed));
                        } else {
                            logging::emit(&SentinelEvent::KillSequenceAborted {
                                reason: "Kill triggered but amount_needed is None/Zero".to_string(),
                            });
                        }
                    } else {
                        logging::emit(&SentinelEvent::Message {
                            level: LogLevel::Error,
                            text: "Monitor returned non-KillTriggered event in Kill status"
                                .to_string(),
                        });
                    }
                }
            }
        }
        sleep(Duration::from_millis(ctx.check_interval_ms));
    }

    logging::emit(&SentinelEvent::Message {
        level: LogLevel::Info,
        text: "Exiting ram-sentinel.".to_string(),
    });
}
