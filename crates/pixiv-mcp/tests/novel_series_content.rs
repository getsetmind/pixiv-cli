use pixiv_sdk::{
    Client,
    transport::{Request, Response, Transport},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
#[path = "support/saved_account.rs"]
mod saved_account;
#[derive(Deserialize)]
struct Contract {
    tools: std::collections::BTreeMap<String, Value>,
    cases: Vec<Case>,
}
#[derive(Deserialize)]
struct Case {
    name: String,
    arguments: Value,
    tool_name: String,
    bodies: Vec<Value>,
    result: Value,
    calls: usize,
    requests: usize,
    rpc_error: String,
}
#[derive(Clone)]
struct Fixture {
    bodies: Vec<Value>,
    requests: Arc<Mutex<usize>>,
}
impl Transport for Fixture {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        assert_eq!(request.method.as_str(), "GET");
        assert_eq!(request.url, "https://app-api.pixiv.net/v2/novel/series");
        assert!(
            request
                .parameters
                .contains(&("series_id".into(), "21".into()))
                || request.parameters.iter().any(|(key, _)| key == "series_id")
        );
        let mut requests = self.requests.lock().unwrap();
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
async fn novel_series_content_match_go_schemas_records_errors_and_saved_account_stdio() {
    let contract: Contract = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/mcp-novel-series-content.json"
    ))
    .unwrap();
    assert_eq!(
        pixiv_mcp::novel_series_tool(),
        contract.tools["novel_series"]
    );
    assert_eq!(
        pixiv_mcp::novel_content_tool(),
        contract.tools["novel_content"]
    );
    assert_eq!(contract.cases.len(), 50);
    for case in contract.cases {
        let fixture = Fixture {
            bodies: case.bodies,
            requests: Arc::new(Mutex::new(0)),
        };
        let client = Client::with_transport("fixture-access", fixture.clone());
        if case.rpc_error.is_empty() {
            let result = if case.tool_name == "novel_series" {
                serde_json::to_value(
                    pixiv_mcp::novel_series(
                        &client,
                        serde_json::from_value(case.arguments.clone()).unwrap(),
                    )
                    .await,
                )
                .unwrap()
            } else {
                serde_json::to_value(
                    pixiv_mcp::novel_content(
                        &client,
                        serde_json::from_value(case.arguments.clone()).unwrap(),
                    )
                    .await,
                )
                .unwrap()
            };
            assert_eq!(result, case.result, "{} direct", case.name);
            assert_eq!(
                *fixture.requests.lock().unwrap(),
                case.requests,
                "{} direct requests",
                case.name
            );
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
            *fixture.requests.lock().unwrap() = 0;
            let mut output = vec![];
            if saved {
                let accounts = saved_account::saved_execution(fixture.clone());
                pixiv_mcp::stdio::serve_saved(&accounts.execution, input.as_bytes(), &mut output)
                    .await
                    .unwrap();
                assert_eq!(
                    accounts._opens.load(std::sync::atomic::Ordering::SeqCst),
                    usize::from(case.calls > 0),
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
                assert_eq!(response["error"]["code"], -32602, "{}", case.name);
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
            assert_eq!(
                case.calls,
                usize::from(
                    case.requests > 0
                        || (case.tool_name == "novel_content"
                            && case.rpc_error.is_empty()
                            && case.arguments["novel_id"].as_i64().unwrap() > 0)
                ),
                "{}",
                case.name
            );
        }
    }
}

#[tokio::test]
async fn stdio_lists_both_new_tools_and_retains_existing_tools() {
    let contract: Contract = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/mcp-novel-series-content.json"
    ))
    .unwrap();
    let fixture = Fixture {
        bodies: vec![],
        requests: Arc::new(Mutex::new(0)),
    };
    let client = Client::with_transport("fixture-access", fixture.clone());
    let input = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{},\"clientInfo\":{\"name\":\"fixture\",\"version\":\"1\"}}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{}}\n"
    );
    let mut output = vec![];
    pixiv_mcp::stdio::serve(&client, input.as_bytes(), &mut output)
        .await
        .unwrap();
    let responses = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    let tools = responses
        .iter()
        .find(|response| response["id"] == 2)
        .unwrap()["result"]["tools"]
        .as_array()
        .unwrap();
    let names = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        names,
        std::collections::BTreeSet::from([
            "illust_detail",
            "search_illust",
            "trending_tags_illust",
            "illust_ranking",
            "novel_detail",
            "novel_series",
            "novel_content",
            "search_novel",
            "user_detail",
            "search_user",
            "add_bookmark",
            "remove_bookmark",
            "add_novel_bookmark",
            "remove_novel_bookmark",
            "follow_user",
            "unfollow_user",
            "illust_series",
            "illust_related",
            "illust_recommended",
            "recommended",
            "user_artworks",
            "user_novels",
            "user_following",
            "user_followers",
            "related_users",
            "blocked_users"
        ])
    );
    assert_eq!(tools.len(), 26);
    assert_eq!(tools[19]["name"], "recommended");
    assert_eq!(tools[14]["name"], "novel_series");
    assert_eq!(tools[15]["name"], "novel_content");
    assert_eq!(tools[16]["name"], "illust_series");
    for (name, expected) in contract.tools {
        assert_eq!(
            *tools.iter().find(|tool| tool["name"] == name).unwrap(),
            expected
        );
    }
    assert_eq!(*fixture.requests.lock().unwrap(), 0);
}
