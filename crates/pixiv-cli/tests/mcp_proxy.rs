use serde_json::{Value, json};
use std::{
    io::Write,
    process::{Command, Stdio},
};

#[test]
fn mcp_proxy_flags_match_go_startup_validation_and_tool_account_requests() {
    let cases: Vec<Value> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/mcp-proxy.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 10);
    for case in cases {
        let home = tempfile::tempdir().unwrap();
        let directory = home.path().join(".pixiv-cli");
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("config.toml");
        std::fs::write(&path, case["config"].as_str().unwrap()).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_pixiv"))
            .arg("mcp")
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
            .env("REQUEST_INTERVAL", "0")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        if !case["results"].as_array().unwrap().is_empty() {
            let init = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"migration","version":"0"}}});
            let detail = json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"illust_detail","arguments":{"illust_id":42}}});
            let search = json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"search_illust","arguments":{"word":"fixture"}}});
            let _ = writeln!(input, "{init}\n{detail}\n{search}");
        }
        drop(input);
        let output = child.wait_with_output().unwrap();
        assert_eq!(
            output.status.code().map(i64::from),
            case["exit"].as_i64(),
            "{}",
            case["name"]
        );
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            case["stderr"].as_str().unwrap(),
            "{}",
            case["name"]
        );
        if case["results"].as_array().unwrap().is_empty() {
            assert_eq!(
                String::from_utf8(output.stdout).unwrap(),
                case["stdout"].as_str().unwrap(),
                "{}",
                case["name"]
            );
        } else {
            let responses: Vec<Value> = String::from_utf8(output.stdout)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert_eq!(responses.len(), 3, "{}", case["name"]);
            for (index, expected) in case["results"].as_array().unwrap().iter().enumerate() {
                let response = responses
                    .iter()
                    .find(|response| response["id"] == index + 2)
                    .unwrap();
                assert_eq!(response["result"], *expected, "{} / {index}", case["name"]);
            }
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
