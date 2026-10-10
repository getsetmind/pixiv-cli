use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[test]
fn saved_cli_random_registration_preflight_and_no_account_match_frozen_go() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../pixiv-mcp/tests/fixtures/download_random_options.json"
    ))
    .unwrap();
    let home = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_pixiv"))
        .arg("mcp")
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("PIXIV_ACCESS_TOKEN", "")
        .env_remove("https_proxy")
        .env_remove("HTTPS_PROXY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let read = |mut stream: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).unwrap();
            bytes
        })
    };
    let out = read(Box::new(stdout));
    let err = read(Box::new(stderr));
    let mut input = child.stdin.take().unwrap();
    let requests = fixture["cases"].as_array().unwrap().clone();
    let writer = std::thread::spawn(move || {
        writeln!(input, "{}", json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"owned-random-process","version":"0"}}})).unwrap();
        writeln!(
            input,
            "{}",
            json!({"jsonrpc":"2.0","id":"list","method":"tools/list","params":{}})
        )
        .unwrap();
        for (index, row) in requests.iter().enumerate() {
            let mut request: Value =
                serde_json::from_str(row["request"].as_str().unwrap()).unwrap();
            request["id"] = json!(index + 100);
            writeln!(input, "{request}").unwrap();
        }
        drop(input);
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("owned MCP random process did not finish");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    writer.join().unwrap();
    assert!(status.success());
    assert!(err.join().unwrap().is_empty());
    let output = String::from_utf8(out.join().unwrap()).unwrap();
    let responses: Vec<Value> = output
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        responses.len(),
        fixture["cases"].as_array().unwrap().len() + 2
    );
    let tools = &responses.iter().find(|row| row["id"] == "list").unwrap()["result"]["tools"];
    assert_eq!(
        tools
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "download_random_from_recommendation")
            .unwrap(),
        &fixture["tool"]
    );
    for (index, row) in fixture["cases"].as_array().unwrap().iter().enumerate() {
        let mut expected = row["response"].clone();
        expected["id"] = json!(index + 100);
        assert_eq!(
            responses
                .iter()
                .find(|response| response["id"] == index + 100)
                .unwrap(),
            &expected,
            "{}",
            row["name"]
        );
    }
}
