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
    tool: Value,
    cases: Vec<Case>,
}
#[derive(Deserialize)]
struct Case {
    name: String,
    arguments: Value,
    body: Value,
    result: Value,
    calls: usize,
    requests: usize,
    rpc_error: String,
}
#[derive(Clone)]
struct Fixture {
    body: Value,
    requests: Arc<Mutex<usize>>,
}
impl Transport for Fixture {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        assert_eq!(request.method.as_str(), "GET");
        assert_eq!(request.url, "https://app-api.pixiv.net/v2/novel/detail");
        *self.requests.lock().unwrap() += 1;
        Ok(Response {
            status: 200,
            retry_after: None,
            body: self.body.clone(),
        })
    }
}
#[tokio::test]
async fn novel_detail_matches_go_schema_records_errors_and_saved_account_stdio() {
    let contract: Contract = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/mcp-novel-detail.json"
    ))
    .unwrap();
    assert_eq!(pixiv_mcp::novel_detail_tool(), contract.tool);
    assert_eq!(contract.cases.len(), 43);
    for case in contract.cases {
        let fixture = Fixture {
            body: case.body,
            requests: Arc::new(Mutex::new(0)),
        };
        let client = Client::with_transport("fixture-access", fixture.clone());
        if case.rpc_error.is_empty() {
            let result = pixiv_mcp::novel_detail(
                &client,
                serde_json::from_value(case.arguments.clone()).unwrap(),
            )
            .await;
            assert_eq!(
                serde_json::to_value(result).unwrap(),
                case.result,
                "{} direct",
                case.name
            );
            assert_eq!(
                *fixture.requests.lock().unwrap(),
                case.requests,
                "{} direct requests",
                case.name
            );
        }
        let messages = [
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"novel_detail","arguments":case.arguments}}),
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
            assert_eq!(case.calls, case.requests, "{}", case.name);
        }
    }
}
