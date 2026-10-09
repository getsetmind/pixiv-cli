use serde::Deserialize;
use std::process::Command;

#[derive(Deserialize)]
struct Case {
    name: String,
    args: Vec<String>,
    before: Option<String>,
    after: String,
    database: bool,
    stdout: String,
    stderr: String,
    exit: i32,
}

#[test]
fn search_startup_preserves_go_account_errors_settings_and_validation_order() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/search-accounts.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 48);
    for case in cases {
        let home = tempfile::tempdir().unwrap();
        let directory = home.path().join(".pixiv-cli");
        let path = directory.join("config.toml");
        if let Some(before) = &case.before {
            std::fs::create_dir(&directory).unwrap();
            std::fs::write(&path, before).unwrap();
        }
        let output = Command::new(env!("CARGO_BIN_EXE_pixiv"))
            .arg("search")
            .args(&case.args)
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .env("PIXIV_ACCESS_TOKEN", "")
            .env_remove("https_proxy")
            .env("HTTPS_PROXY", "")
            .env("REQUEST_INTERVAL", "0")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(case.exit), "{}", case.name);
        assert_eq!(output.stdout, case.stdout.as_bytes(), "{}", case.name);
        if case
            .args
            .iter()
            .any(|arg| arg == "--json" || arg == "--ndjson")
        {
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&output.stderr).unwrap(),
                serde_json::from_str::<serde_json::Value>(&case.stderr).unwrap(),
                "{}",
                case.name
            );
        } else {
            assert_eq!(output.stderr, case.stderr.as_bytes(), "{}", case.name);
        }
        assert_eq!(
            std::fs::read(&path).unwrap(),
            case.after.as_bytes(),
            "{}",
            case.name
        );
        assert_eq!(
            directory.join("pixiv-cli.db").exists(),
            case.database,
            "{}",
            case.name
        );
    }
}
