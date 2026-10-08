use serde_json::Value;
use std::process::Command;

#[test]
fn detail_proxy_flags_match_go_presence_precedence_and_diagnostic_order() {
    let cases: Vec<Value> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/detail-proxy.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 22);
    for case in cases {
        let home = tempfile::tempdir().unwrap();
        let directory = home.path().join(".pixiv-cli");
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("config.toml");
        std::fs::write(&path, case["config"].as_str().unwrap()).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_pixiv"));
        command
            .args(["detail", case["input"].as_str().unwrap()])
            .args(
                case["flags"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|flag| flag.as_str().unwrap()),
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
            output.status.code().map(i64::from),
            case["exit"].as_i64(),
            "{}",
            case["name"]
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            case["stdout"].as_str().unwrap(),
            "{}",
            case["name"]
        );
        if case["json"] == true {
            assert_eq!(
                serde_json::from_slice::<Value>(&output.stderr).unwrap(),
                serde_json::from_str::<Value>(case["stderr"].as_str().unwrap()).unwrap(),
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
            case["config"].as_str().unwrap().as_bytes()
        );
        assert_eq!(
            directory.join("pixiv-cli.db").exists(),
            case["database"].as_bool().unwrap(),
            "{}",
            case["name"]
        );
    }
}
