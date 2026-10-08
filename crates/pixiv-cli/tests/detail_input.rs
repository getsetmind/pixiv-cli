use serde::Deserialize;
use serde_json::Value;
use std::process::Command;
#[derive(Deserialize)]
struct Case {
    input: String,
    json: bool,
    id: i64,
    builds: u32,
    stdout: String,
    stderr: String,
    exit: i32,
}
#[test]
fn detail_inputs_match_go_process_errors_and_validate_before_client_configuration() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/detail-input.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 122);
    for case in cases {
        assert_eq!(case.builds > 0, case.id > 0);
        let mut command = Command::new(env!("CARGO_BIN_EXE_pixiv"));
        command.arg("detail");
        if case.json {
            command.arg("--json");
        }
        command
            .args(["--", &case.input])
            .env("PIXIV_ACCESS_TOKEN", "")
            .env_remove("https_proxy")
            .env_remove("HTTPS_PROXY");
        if case.builds == 0 {
            command.env("https_proxy", "invalid proxy fixture");
        }
        let output = command.output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(case.exit),
            "input={}",
            case.input
        );
        assert_eq!(String::from_utf8(output.stdout).unwrap(), case.stdout);
        if case.json {
            assert_eq!(
                serde_json::from_slice::<Value>(&output.stderr).unwrap(),
                serde_json::from_str::<Value>(&case.stderr).unwrap(),
                "input={}",
                case.input
            );
            assert_eq!(
                output.stderr.iter().filter(|byte| **byte == b'\n').count(),
                1
            );
        } else {
            assert_eq!(
                String::from_utf8(output.stderr).unwrap(),
                case.stderr,
                "input={}",
                case.input
            );
        }
    }
}
