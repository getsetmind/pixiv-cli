#[path = "support/auth_accounts.rs"]
mod auth_support;
use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn auth_transfer_preserves_go_root_startup_stdio_and_saved_bytes() {
    let cases: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/cli-auth-transfer.json"
    ))
    .unwrap();
    for case in cases {
        if case["short_write"].as_bool() == Some(true)
            || case["read_error"].as_bool() == Some(true)
            || case["write_error"].as_bool() == Some(true)
            || case["diagnostics_error"].as_bool() == Some(true)
        {
            continue;
        }
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(".pixiv-cli/config.toml");
        if let Some(before) = case["before"].as_str() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, before).unwrap();
        }
        auth_support::seed(&case, home.path());
        let mut command = Command::new(env!("CARGO_BIN_EXE_pixiv"));
        command.arg("auth").args(
            case["args"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|value| value.as_str().unwrap()),
        );
        for key in [
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
        ] {
            command.env_remove(key);
        }
        command
            .env("HOME", home.path())
            .env("USERPROFILE", home.path());
        for (key, value) in case["environment"].as_object().unwrap() {
            command.env(key, value.as_str().unwrap());
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let _ = child
            .stdin
            .take()
            .unwrap()
            .write_all(case["input"].as_str().unwrap().as_bytes());
        let output = child.wait_with_output().unwrap();
        let normalize = |bytes: &[u8]| {
            String::from_utf8(bytes.to_vec())
                .unwrap()
                .replace(&home.path().to_string_lossy().to_string(), "<HOME>")
        };
        assert_eq!(
            output.status.code(),
            Some(case["exit"].as_i64().unwrap() as i32),
            "{}",
            case["name"]
        );
        assert_eq!(
            normalize(&output.stdout),
            case["stdout"].as_str().unwrap(),
            "{}",
            case["name"]
        );
        assert_eq!(
            normalize(&output.stderr),
            case["stderr"].as_str().unwrap(),
            "{}",
            case["name"]
        );
        assert_eq!(
            path.exists(),
            case["config"].as_bool().unwrap(),
            "{}",
            case["name"]
        );
        if path.exists() {
            assert_eq!(
                std::fs::read(&path).unwrap(),
                case["after"].as_str().unwrap().as_bytes(),
                "{}",
                case["name"]
            );
        }
        auth_support::assert_states(&case, home.path());
        assert_eq!(
            home.path().join(".pixiv-cli/pixiv-cli.db").exists(),
            case["database"].as_bool().unwrap(),
            "{}",
            case["name"]
        );
    }
}
