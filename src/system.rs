use std::env;
use std::path::PathBuf;

pub fn get_systemd_unit() -> String {
    let path_result: Result<PathBuf, std::io::Error> = env::current_exe();

    let exec_start_path = match path_result {
        Ok(path_buf) => path_buf.to_string_lossy().into_owned(),
        Err(_e) => {
            let fallback_path: &str =
                "/usr/local/bin/ram-sentinel # Ensure ensure this path is correct";
            fallback_path.to_owned()
        }
    };
    let unit_file_content: String = format!(
        r#"[Unit]
Description=RAM Sentinel - OOM Prevention Daemon
Documentation=https://github.com/benedictjohannes/ram-sentinel
After=graphical-session.target

[Service]
Type=simple
{}
Restart=on-failure
RestartSec=5s
Nice=-10
OOMScoreAdjust=-1000

[Install]
WantedBy=default.target
"#,
        format!("ExecStart={}", exec_start_path),
    );
    unit_file_content
}
