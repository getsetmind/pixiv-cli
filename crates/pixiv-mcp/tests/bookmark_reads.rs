#[path = "support/implemented_catalog.rs"]
mod implemented_catalog;
use pixiv_sdk::{
    Client,
    transport::{Request, Response, Transport},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
#[path = "support/user_works_saved.rs"]
mod saved_account;
#[derive(Deserialize)]
struct Contract {
    tools: std::collections::BTreeMap<String, Value>,
    cases: Vec<Case>,
}
#[derive(Deserialize)]
struct Case {
    #[serde(default)]
    no_identity: bool,
    name: String,
    arguments: Value,
    tool_name: String,
    bodies: Vec<Value>,
    result: Value,
    calls: usize,
    requests: usize,
    rpc_error: String,
    rpc_code: i64,
    queries: Option<Vec<String>>,
    paths: Option<Vec<String>>,
}
#[derive(Clone)]
struct Fixture {
    bodies: Vec<Value>,
    queries: Vec<String>,
    paths: Vec<String>,
    requests: Arc<Mutex<usize>>,
}
impl Transport for Fixture {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        if request.operation == "Open" {
            return Ok(Response {
                status: 200,
                retry_after: None,
                body: json!({"access_token":"fixture-access","refresh_token":"fixture-rotated","expires_in":3600,"user":{"id":42}}),
            });
        };
        assert_eq!(request.method.as_str(), "GET");
        let mut requests = self.requests.lock().unwrap();
        assert_eq!(
            request.url,
            format!("https://app-api.pixiv.net{}", self.paths[*requests])
        );
        let mut parameters = request.parameters.clone();
        parameters.sort();
        let query = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(parameters)
            .finish();
        assert_eq!(query, self.queries[*requests]);
        let body = self.bodies[*requests].clone();
        *requests += 1;
        Ok(Response {
            status: 200,
            retry_after: None,
            body,
        })
    }
}
#[tokio::test]
async fn bookmark_reads_match_go_schemas_records_errors_defaults_and_saved_account_stdio() {
    let contract: Contract = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/mcp-bookmark-reads.json"
    ))
    .unwrap();
    for kind in [
        pixiv_mcp::BookmarkRead::ArtworkDetail,
        pixiv_mcp::BookmarkRead::NovelDetail,
        pixiv_mcp::BookmarkRead::ArtworkTags,
        pixiv_mcp::BookmarkRead::NovelTags,
        pixiv_mcp::BookmarkRead::AllTags,
    ] {
        let tool = pixiv_mcp::bookmark_read_tool(kind);
        assert_eq!(tool, contract.tools[tool["name"].as_str().unwrap()]);
    }
    assert_eq!(contract.cases.len(), 202);
    for case in contract.cases {
        let fixture = Fixture {
            bodies: case.bodies,
            queries: case.queries.unwrap_or_default(),
            paths: case.paths.unwrap_or_default(),
            requests: Arc::new(Mutex::new(0)),
        };
        let credentials = pixiv_sdk::oauth::refresh(&fixture, "fixture-refresh")
            .await
            .unwrap();
        let client = if case.no_identity {
            Client::with_transport("fixture-access", fixture.clone())
        } else {
            Client::from_credentials(&credentials, fixture.clone())
        };
        if case.rpc_error.is_empty() && case.arguments.is_object() {
            let result = serde_json::to_value(
                pixiv_mcp::bookmark_read(
                    &client,
                    pixiv_mcp::BookmarkRead::from_name(&case.tool_name).unwrap(),
                    serde_json::from_value(case.arguments.clone()).unwrap(),
                )
                .await,
            )
            .unwrap();
            assert_eq!(result, case.result, "{} direct", case.name);
            assert_eq!(
                *fixture.requests.lock().unwrap(),
                case.requests,
                "{} direct requests",
                case.name
            );
        }
        if case.rpc_error.contains("validating tool output") {
            let result = serde_json::to_value(
                pixiv_mcp::bookmark_read(
                    &client,
                    pixiv_mcp::BookmarkRead::ArtworkDetail,
                    serde_json::from_value(case.arguments.clone()).unwrap(),
                )
                .await,
            )
            .unwrap();
            assert!(
                result["structuredContent"]["tags"].is_null(),
                "{} direct null contract",
                case.name
            );
            assert_ne!(
                result["isError"], true,
                "{} direct successful result",
                case.name
            );
            assert_eq!(*fixture.requests.lock().unwrap(), case.requests);
        }
        let messages = [
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":case.tool_name,"arguments":case.arguments}}),
        ];
        let input = messages
            .iter()
            .map(|message| message.to_string() + "\n")
            .collect::<String>();
        for saved in [false, true] {
            if saved && case.no_identity {
                continue;
            }
            *fixture.requests.lock().unwrap() = 0;
            let mut output = vec![];
            if saved {
                let accounts = saved_account::saved_execution(fixture.clone());
                pixiv_mcp::stdio::serve_saved(&accounts.execution, input.as_bytes(), &mut output)
                    .await
                    .unwrap();
                assert_eq!(
                    accounts._opens.load(std::sync::atomic::Ordering::SeqCst),
                    case.calls,
                    "{} saved account acquisition",
                    case.name
                );
            } else {
                pixiv_mcp::stdio::serve(&client, input.as_bytes(), &mut output)
                    .await
                    .unwrap();
            }
            let responses = String::from_utf8(output)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str::<Value>(line).unwrap())
                .collect::<Vec<_>>();
            let response = responses.iter().find(|value| value["id"] == 2).unwrap();
            if case.rpc_error.is_empty() {
                assert_eq!(
                    response["result"], case.result,
                    "{} saved={saved}",
                    case.name
                );
            } else {
                assert_eq!(response["error"]["code"], case.rpc_code, "{}", case.name);
                assert_eq!(
                    response["error"]["message"],
                    case.rpc_error
                        .strip_prefix("calling \"tools/call\": ")
                        .unwrap(),
                    "{}",
                    case.name
                );
            }
            assert_eq!(
                *fixture.requests.lock().unwrap(),
                case.requests,
                "{} saved={saved}",
                case.name
            );
        }
    }
}

#[tokio::test]
async fn bookmark_reads_append_catalog_without_reordering_existing_tools() {
    let fixture = Fixture {
        bodies: vec![],
        queries: vec![],
        paths: vec![],
        requests: Arc::new(Mutex::new(0)),
    };
    let client = Client::with_transport("fixture-access", fixture);
    let messages = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    ];
    let input = messages
        .iter()
        .map(|message| message.to_string() + "\n")
        .collect::<String>();
    let mut output = vec![];
    pixiv_mcp::stdio::serve(&client, input.as_bytes(), &mut output)
        .await
        .unwrap();
    let responses = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    let tools = responses.iter().find(|r| r["id"] == 2).unwrap()["result"]["tools"]
        .as_array()
        .unwrap();
    implemented_catalog::assert_catalog(tools);
}
