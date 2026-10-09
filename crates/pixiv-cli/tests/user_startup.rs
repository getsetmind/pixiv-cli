use std::process::Command;

#[test]
fn user_detail_startup_matches_go_initialization_input_order_and_database_creation() {
    let cases: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/user-startup.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 24);
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
            .args(["detail", "--type=user", case["input"].as_str().unwrap()])
            .args(
                case["flags"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|value| value.as_str().unwrap()),
            )
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
