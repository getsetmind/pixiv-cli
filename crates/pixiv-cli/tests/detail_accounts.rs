use serde::Deserialize;
use std::process::Command;

#[derive(Deserialize)]
struct Case {
    name: String,
    config: String,
    json: bool,
    stdout: String,
    stderr: String,
    exit: i32,
}

#[test]
fn detail_process_matches_go_saved_account_configuration_errors_without_changing_settings() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/detail-accounts.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 12);
    for case in cases {
        let home = tempfile::tempdir().unwrap();
        let directory = home.path().join(".pixiv-cli");
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("config.toml");
        std::fs::write(&path, &case.config).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_pixiv"));
        command
            .args(["detail", "42"])
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .env("PIXIV_ACCESS_TOKEN", "")
            .env_remove("https_proxy")
            .env("HTTPS_PROXY", "")
            .env("REQUEST_INTERVAL", "0");
        if case.json {
            command.arg("--json");
        }
        let output = command.output().unwrap();
        assert_eq!(output.status.code(), Some(case.exit), "{}", case.name);
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            case.stdout,
            "{}",
            case.name
        );
        if case.json {
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&output.stderr).unwrap(),
                serde_json::from_str::<serde_json::Value>(&case.stderr).unwrap(),
                "{}",
                case.name
            );
            assert_eq!(
                output.stderr.iter().filter(|byte| **byte == b'\n').count(),
                1
            );
        } else {
            assert_eq!(
                String::from_utf8(output.stderr).unwrap(),
                case.stderr,
                "{}",
                case.name
            );
        }
        assert_eq!(std::fs::read(&path).unwrap(), case.config.as_bytes());
        assert!(!home.path().join(".pixiv-cli-rs").exists());
    }
}

#[test]
fn detail_startup_matches_go_initialization_input_order_and_database_creation() {
    let cases: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/detail-startup.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 10);
    for case in cases {
        let home = tempfile::tempdir().unwrap();
        let directory = home.path().join(".pixiv-cli");
        let path = directory.join("config.toml");
        if let Some(before) = case["before"].as_str() {
            std::fs::create_dir(&directory).unwrap();
            std::fs::write(&path, before).unwrap();
        }
        let mut command = Command::new(env!("CARGO_BIN_EXE_pixiv"));
        command
            .args(["detail", case["input"].as_str().unwrap()])
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .env("PIXIV_ACCESS_TOKEN", "")
            .env_remove("https_proxy")
            .env("HTTPS_PROXY", "")
            .env("REQUEST_INTERVAL", "0");
        if case["json"] == true {
            command.arg("--json");
        }
        let output = command.output().unwrap();
        assert_eq!(
            output.status.code(),
            case["exit"].as_i64().map(|exit| exit as i32),
            "{}",
            case["name"]
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            case["stdout"].as_str().unwrap()
        );
        if case["json"] == true {
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&output.stderr).unwrap(),
                serde_json::from_str::<serde_json::Value>(case["stderr"].as_str().unwrap())
                    .unwrap(),
                "{}",
                case["name"]
            );
        } else {
            assert_eq!(
                String::from_utf8(output.stderr).unwrap(),
                case["stderr"].as_str().unwrap(),
                "{}",
                case["name"]
            );
        }
        assert_eq!(
            std::fs::read(&path).unwrap(),
            case["after"].as_str().unwrap().as_bytes(),
            "{}",
            case["name"]
        );
        assert_eq!(
            directory.join("pixiv-cli.db").exists(),
            case["database"].as_bool().unwrap(),
            "{}",
            case["name"]
        );
    }
}
