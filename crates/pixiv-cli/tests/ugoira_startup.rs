#![cfg(target_os = "linux")]

use serde_json::Value;
use std::{
    io::{self, Write},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const ARGUMENT_ROWS: [&str; 12] = [
    "missing",
    "missing-piped-artwork",
    "missing-piped-record",
    "extra",
    "unknown-long",
    "unknown-short",
    "ndjson-rejected",
    "missing-before-malformed",
    "extra-before-malformed",
    "unknown-before-malformed",
    "help",
    "help-before-malformed",
];

struct OwnedChild(Option<Child>);

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn fixture() -> Value {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/cli-ugoira-startup.json")).unwrap();
    assert_eq!(
        fixture["reference"],
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    assert_eq!(fixture["sources"].as_object().unwrap().len(), 14);
    assert_eq!(fixture["rows"].as_array().unwrap().len(), 52);
    fixture
}

fn assert_startup(row: &Value) {
    let label = row["name"].as_str().unwrap();
    assert_ne!(row["comparison"], "go-only", "{label}");
    assert_eq!(row["stdin_reads"], 0, "{label}");
    assert_eq!(row["network_calls"], 0, "{label}");
    let home = tempfile::tempdir().unwrap();
    let data = home.path().join(".pixiv-cli");
    let config = data.join("config.toml");
    let temp = home.path().join("tmp");
    std::fs::create_dir(&temp).unwrap();
    if let Some(before) = row["before"].as_str() {
        std::fs::create_dir(&data).unwrap();
        std::fs::write(&config, before).unwrap();
    }
    let mut command = Command::new(env!("CARGO_BIN_EXE_pixiv"));
    command
        .args(
            row["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|arg| arg.as_str().unwrap()),
        )
        .env_clear()
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("TMPDIR", &temp)
        .env("PATH", home.path())
        .current_dir(home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in row["env"].as_object().unwrap() {
        command.env(key, value.as_str().unwrap());
    }
    let mut owned = OwnedChild(Some(command.spawn().unwrap()));
    let child = owned.0.as_mut().unwrap();
    let mut input = child.stdin.take().unwrap();
    if let Err(error) = input.write_all(row["input"].as_str().unwrap().as_bytes()) {
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe, "{label}: {error}");
    }
    // Keeping stdin open distinguishes immediate argv validation from EOF fallback.
    let until = Instant::now() + Duration::from_secs(5);
    while child.try_wait().unwrap().is_none() {
        assert!(
            Instant::now() < until,
            "{label} waited for unexpected input"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    drop(input);
    let output = owned.0.take().unwrap().wait_with_output().unwrap();
    let normalize_home = |bytes: &[u8]| {
        String::from_utf8(bytes.to_vec())
            .unwrap()
            .replace(home.path().to_str().unwrap(), "<HOME>")
    };
    assert_eq!(
        output.status.code(),
        Some(row["exit"].as_i64().unwrap() as i32),
        "{label}"
    );
    if row["comparison"] == "exact" {
        assert_eq!(
            normalize_home(&output.stdout),
            row["stdout"].as_str().unwrap(),
            "{label}"
        );
    } else {
        assert_eq!(row["comparison"], "startup-state", "{label}");
        assert!(!output.stdout.is_empty(), "{label} omitted help");
    }
    let stderr = normalize_home(&output.stderr);
    let expected_stderr = row["stderr"].as_str().unwrap();
    if expected_stderr.starts_with('{') {
        assert!(stderr.ends_with('\n'), "{label}");
        assert_eq!(stderr.lines().count(), 1, "{label}");
        assert_eq!(
            serde_json::from_str::<Value>(&stderr).unwrap(),
            serde_json::from_str::<Value>(expected_stderr).unwrap(),
            "{label}"
        );
    } else {
        assert_eq!(stderr, expected_stderr, "{label}");
    }
    assert_eq!(config.exists(), row["config"].as_bool().unwrap(), "{label}");
    assert_eq!(
        data.join("pixiv-cli.db").exists(),
        row["database"].as_bool().unwrap(),
        "{label}"
    );
    if config.exists() {
        assert_eq!(
            std::fs::read_to_string(config).unwrap(),
            row["after"].as_str().unwrap(),
            "{label}"
        );
    }
}

#[test]
fn ugoira_process_preserves_go_argument_preflight_and_help_startup_state() {
    let fixture = fixture();
    let mut compared = 0;
    for row in fixture["rows"].as_array().unwrap() {
        if ARGUMENT_ROWS.contains(&row["name"].as_str().unwrap()) {
            assert_startup(row);
            compared += 1;
        }
    }
    assert_eq!(compared, 12);
}

#[test]
fn ugoira_process_preserves_go_saved_account_configuration_proxy_and_machine_errors() {
    let fixture = fixture();
    let mut compared = 0;
    for row in fixture["rows"].as_array().unwrap() {
        if row["comparison"] == "exact" && !ARGUMENT_ROWS.contains(&row["name"].as_str().unwrap()) {
            assert_startup(row);
            compared += 1;
        }
    }
    assert_eq!(compared, 35);
}

#[test]
fn ugoira_process_does_not_use_piped_artwork_or_records_as_a_source() {
    let fixture = fixture();
    let mut compared = 0;
    for row in fixture["rows"].as_array().unwrap() {
        if !row["input"].as_str().unwrap().is_empty() {
            assert_startup(row);
            compared += 1;
        }
    }
    assert_eq!(compared, 3);
}
