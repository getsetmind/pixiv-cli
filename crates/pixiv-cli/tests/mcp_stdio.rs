use serde_json::{Value, json};
use std::{
    io::Write,
    process::{Command, Stdio},
};

#[test]
fn mcp_process_exchanges_jsonrpc_without_stdout_diagnostics_or_credentials() {
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
    assert_eq!(by_id("list")["result"]["tools"][1]["name"], "search_illust");
    assert_eq!(
        by_id("list")["result"]["tools"][2],
        pixiv_mcp::trending_tags_illust_tool()
    );
    assert_eq!(
        by_id("list")["result"]["tools"][3],
        pixiv_mcp::illust_ranking_tool()
    );
    assert_eq!(
        by_id("list")["result"]["tools"][4],
        pixiv_mcp::novel_detail_tool()
    );
    assert_eq!(
        by_id("list")["result"]["tools"][5],
        pixiv_mcp::search_novel_tool()
    );
    assert_eq!(
        by_id("list")["result"]["tools"][6],
        pixiv_mcp::user_detail_tool()
    );
    assert_eq!(
        by_id("list")["result"]["tools"][7],
        pixiv_mcp::search_user_tool()
    );
    assert_eq!(
        by_id("list")["result"]["tools"].as_array().unwrap().len(),
        20
    );
    assert_eq!(
        by_id("list")["result"]["tools"][19],
        pixiv_mcp::recommended_tool()
    );
    for (index, name) in [
        "add_bookmark",
        "remove_bookmark",
        "add_novel_bookmark",
        "remove_novel_bookmark",
        "follow_user",
        "unfollow_user",
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(by_id("list")["result"]["tools"][index + 8]["name"], name);
        assert_eq!(
            by_id("list")["result"]["tools"][index + 8]["inputSchema"]["type"],
            "object"
        );
    }
    let contract: Value = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/mcp-novel-series-content.json"
    ))
    .unwrap();
    for (index, name) in [(14, "novel_series"), (15, "novel_content")] {
        assert_eq!(by_id("list")["result"]["tools"][index]["name"], name);
        assert_eq!(
            by_id("list")["result"]["tools"][index],
            contract["tools"][name]
        );
    }
    assert_eq!(
        by_id("list")["result"]["tools"][16],
        pixiv_mcp::illust_series_tool()
    );
    assert_eq!(
        by_id("list")["result"]["tools"][17],
        pixiv_mcp::illust_related_tool()
    );
    assert_eq!(
        by_id("list")["result"]["tools"][18],
        pixiv_mcp::illust_recommended_tool()
    );
    assert_eq!(
        by_id("invalid")["result"]["content"][0]["text"],
        "Error: provide exactly one of illust_id or url"
    );
    assert_eq!(
        by_id("auth")["result"]["content"][0]["text"],
        "Error: pixiv:auth: unauthorized: no pixiv account is authenticated"
    );
    assert_eq!(
        by_id("auth")["result"]["structuredContent"],
        json!({"records":[]})
    );
}

#[test]
fn mcp_process_preserves_go_saved_account_errors_for_detail_and_search() {
    let cases: Vec<Value> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/mcp-accounts.json"
    ))
    .unwrap();
    for case in cases {
        let home = tempfile::tempdir().unwrap();
        let directory = home.path().join(".pixiv-cli");
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(
            directory.join("config.toml"),
            case["config"].as_str().unwrap(),
        )
        .unwrap();
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
        let mut input = child.stdin.take().unwrap();
        for message in [
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"process-test","version":"0"}}}),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":case["tool"],"arguments":case["arguments"]}}),
        ] {
            writeln!(input, "{message}").unwrap();
        }
        drop(input);
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{case}");
        assert!(
            output.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let responses: Vec<Value> = String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let response = responses
            .iter()
            .find(|response| response["id"] == 7)
            .unwrap();
        assert_eq!(
            response["result"], case["result"],
            "{} / {}",
            case["name"], case["tool"]
        );
        assert!(directory.join("pixiv-cli.db").exists(), "{case}");
    }
}
