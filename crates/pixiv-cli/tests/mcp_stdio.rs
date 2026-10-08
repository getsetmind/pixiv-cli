use serde_json::{Value, json};
use std::{
    io::Write,
    process::{Command, Stdio},
};

#[test]
fn mcp_process_exchanges_jsonrpc_without_stdout_diagnostics_or_credentials() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_pixiv"))
        .arg("mcp")
        .env("PIXIV_ACCESS_TOKEN", "")
        .env_remove("https_proxy")
        .env_remove("HTTPS_PROXY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let messages = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"process-test","version":"0"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":"list","method":"tools/list","params":{}}),
        json!({"jsonrpc":"2.0","id":"invalid","method":"tools/call","params":{"name":"illust_detail","arguments":{}}}),
        json!({"jsonrpc":"2.0","id":"auth","method":"tools/call","params":{"name":"illust_detail","arguments":{"illust_id":42}}}),
    ];
    let mut input = child.stdin.take().unwrap();
    for message in messages {
        writeln!(input, "{message}").unwrap();
    }
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let responses = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(responses.len(), 4);
    let by_id = |id: &str| {
        responses
            .iter()
            .find(|response| response["id"] == id)
            .unwrap()
    };
    assert_eq!(by_id("list")["result"]["tools"][0]["name"], "illust_detail");
    assert_eq!(
        by_id("invalid")["result"]["content"][0]["text"],
        "Error: provide exactly one of illust_id or url"
    );
    assert_eq!(
        by_id("auth")["result"]["content"][0]["text"],
        "Error: pixiv:Artwork: unauthorized"
    );
    assert_eq!(
        by_id("auth")["result"]["structuredContent"],
        json!({"records":[]})
    );
}
