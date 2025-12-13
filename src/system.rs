pub fn get_systemd_unit() -> &'static str {
    r#"[Unit]
Description=RAM Sentinel - Userspace OOM Prevention Daemon
Documentation=https://github.com/benedict/ram-sentinel
After=graphical-session.target.target

[Service]
Type=simple
ExecStart=%h/.cargo/bin/ram-sentinel
Restart=on-failure
RestartSec=5s
Nice=-10
OOMScoreAdjust=-1000

[Install]
WantedBy=default.target
"#
}
