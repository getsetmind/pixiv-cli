use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn user_owner_startup_preserves_distinct_inputs_flags_and_validation_order() {
    let cases: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/user-compat-startup.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 61);
    for case in cases {
        let home = tempfile::tempdir().unwrap();
        if let Some(before) = case["before"].as_str() {
            std::fs::create_dir(home.path().join(".pixiv-cli")).unwrap();
            std::fs::write(home.path().join(".pixiv-cli/config.toml"), before).unwrap();
        }
        let mut child = Command::new(env!("CARGO_BIN_EXE_pixiv"))
            .arg("user")
            .args(
                case["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|arg| arg.as_str().unwrap()),
            )
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .env("HTTPS_PROXY", "")
            .env_remove("https_proxy")
            .env("REQUEST_INTERVAL", "0")
            .env("PIXIV_ACCESS_TOKEN", "")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(case["input"].as_str().unwrap().as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(case["exit"].as_i64().unwrap() as i32),
            "{}",
            case["args"]
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            case["stdout"].as_str().unwrap(),
            "{}",
            case["args"]
        );
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            case["stderr"].as_str().unwrap(),
            "{}",
            case["args"]
        );
        assert_eq!(
            home.path().join(".pixiv-cli/config.toml").exists(),
            case["config"].as_bool().unwrap(),
            "{}",
            case["args"]
        );
        assert_eq!(
            home.path().join(".pixiv-cli/pixiv-cli.db").exists(),
            case["database"].as_bool().unwrap(),
            "{}",
            case["args"]
        );
    }
}
