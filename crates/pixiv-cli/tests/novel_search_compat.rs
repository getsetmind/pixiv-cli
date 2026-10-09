use serde::Deserialize;
use serde_json::Value;
use std::process::Command;
#[derive(Deserialize)]
struct Case {
    args: Vec<String>,
    startup_stderr: String,
    startup_input: String,
    startup_args: Vec<String>,
    startup_exit: i32,
    config: bool,
    database: bool,
}
fn equal_output(actual: &[u8], expected: &str) {
    if expected.starts_with('{') {
        assert_eq!(
            serde_json::from_slice::<Value>(actual).unwrap(),
            serde_json::from_str::<Value>(expected).unwrap()
        );
    } else {
        assert_eq!(actual, expected.as_bytes());
    }
}
#[test]
fn novel_search_compatibility_preserves_go_flags_outputs_and_startup_order() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/cli-novel-search-compat.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 99);
    for case in cases {
        let home = tempfile::tempdir().unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_pixiv"));
        command
            .args(["novel", "search"])
            .args(&case.startup_args)
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .env("HTTPS_PROXY", "")
            .env("REQUEST_INTERVAL", "0")
            .env("PIXIV_ACCESS_TOKEN", "")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = command.spawn().unwrap();
        std::io::Write::write_all(
            &mut child.stdin.take().unwrap(),
            case.startup_input.as_bytes(),
        )
        .unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(case.startup_exit),
            "{:?}",
            case.args
        );
        assert!(output.stdout.is_empty());
        equal_output(&output.stderr, &case.startup_stderr);
        assert_eq!(
            home.path().join(".pixiv-cli/config.toml").exists(),
            case.config
        );
        assert_eq!(
            home.path().join(".pixiv-cli/pixiv-cli.db").exists(),
            case.database
        );
    }
}
