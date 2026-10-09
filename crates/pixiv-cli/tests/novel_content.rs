use std::process::Command;

#[test]
fn content_process_preserves_go_prefetch_validation_and_startup_order() {
    let cases: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/cli-novel-content.json"
    ))
    .unwrap();
    for case in cases {
        let home = tempfile::tempdir().unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_pixiv"))
            .arg("detail")
            .args(
                case["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|value| value.as_str().unwrap()),
            )
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .env_remove("https_proxy")
            .env("HTTPS_PROXY", "")
            .env("REQUEST_INTERVAL", "0")
            .env("PIXIV_ACCESS_TOKEN", "")
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            case["startup_exit"].as_i64().map(|value| value as i32),
            "{}",
            case["args"]
        );
        assert!(output.stdout.is_empty());
        let expected = case["startup_stderr"].as_str().unwrap();
        if expected.starts_with('{') {
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&output.stderr).unwrap(),
                serde_json::from_str::<serde_json::Value>(expected).unwrap(),
                "{}",
                case["args"]
            );
        } else {
            assert_eq!(output.stderr, expected.as_bytes(), "{}", case["args"]);
        }
        assert_eq!(
            home.path().join(".pixiv-cli/config.toml").exists(),
            case["config"].as_bool().unwrap()
        );
        assert_eq!(
            home.path().join(".pixiv-cli/pixiv-cli.db").exists(),
            case["database"].as_bool().unwrap()
        );
        assert_eq!(case["builds"], 0);
        assert_eq!(case["accounts"], 0);
        assert_eq!(case["fetches"], 0);
    }
}
