#![cfg(target_os = "linux")]
use serde_json::Value;
use std::{
    io::Write,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[test]
fn direct_download_process_preserves_go_root_startup_errors_and_saved_state() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/download_startup.json")).unwrap();
    let mut compared = 0;
    for row in fixture.as_array().unwrap() {
        if !row["cleanup_error"].as_str().unwrap().is_empty()
            || row["supported"] == true
            || (row["read_error"] == true && row["stdin_reads"].as_u64().unwrap() > 0)
        {
            continue;
        }
        let home = tempfile::tempdir().unwrap();
        let data = home.path().join(".pixiv-cli");
        let config = data.join("config.toml");
        if let Some(before) = row["before"].as_str() {
            std::fs::create_dir(&data).unwrap();
            std::fs::write(&config, before).unwrap();
        }
        let mut command = Command::new(env!("CARGO_BIN_EXE_pixiv"));
        command.args(
            row["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap()),
        );
        for name in [
            "PIXIV_ACCESS_TOKEN",
            "PIXIV_REFRESH_TOKEN",
            "HTTPS_PROXY",
            "https_proxy",
            "HTTP_PROXY",
            "http_proxy",
            "ALL_PROXY",
            "DOWNLOAD_PATH",
            "FILENAME_TEMPLATE",
            "DIRECTORY_TEMPLATE",
            "PIXIV_REQUEST_INTERVAL",
            "PIXIV_LOG_LEVEL",
            "PIXIV_LOG_FORMAT",
            "SAUCENAO_API_KEY",
            "PIXIV_CLIENT_DIR",
            "PIXIV_CONFIG_DIR",
            "PIXIV_DATA_DIR",
            "XDG_DATA_HOME",
            "XDG_CONFIG_HOME",
        ] {
            command.env_remove(name);
        }
        let mut child = command
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .env("PATH", home.path())
            .current_dir(home.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        input
            .write_all(row["input"].as_str().unwrap().as_bytes())
            .unwrap();
        let held = if row["stdin_reads"].as_u64().unwrap() == 0 {
            Some(input)
        } else {
            drop(input);
            None
        };
        let until = Instant::now() + Duration::from_secs(5);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() > until {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("{} waited for unexpected input", row["name"])
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        drop(held);
        let output = child.wait_with_output().unwrap();
        let normalize = |bytes: &[u8]| {
            String::from_utf8(bytes.to_vec())
                .unwrap()
                .replace(home.path().to_str().unwrap(), "<HOME>")
        };
        assert_eq!(
            output.status.code(),
            Some(row["exit"].as_i64().unwrap() as i32),
            "{}",
            row["name"]
        );
        let stdout = normalize(&output.stdout);
        if row["name"] == "root-help-download-leaf" {
            let lines: Vec<_> = stdout
                .lines()
                .filter(|line| line.trim_start().starts_with("download "))
                .collect();
            assert_eq!(lines.len(), 1, "{}", row["name"]);
            let line = lines[0];
            assert_eq!(
                format!("{line}\n"),
                row["stdout"].as_str().unwrap(),
                "{}",
                row["name"]
            );
        } else {
            assert_eq!(stdout, row["stdout"].as_str().unwrap(), "{}", row["name"]);
        }
        assert_eq!(
            normalize(&output.stderr),
            row["stderr"].as_str().unwrap(),
            "{}",
            row["name"]
        );
        assert_eq!(
            config.exists(),
            row["config"].as_bool().unwrap(),
            "{}",
            row["name"]
        );
        assert_eq!(
            data.join("pixiv-cli.db").exists(),
            row["database"].as_bool().unwrap(),
            "{}",
            row["name"]
        );
        if config.exists() {
            assert_eq!(
                std::fs::read_to_string(&config).unwrap(),
                row["after"].as_str().unwrap(),
                "{}",
                row["name"]
            );
        }
        compared += 1;
    }
    assert_eq!(compared, 23);
}
