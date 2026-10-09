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
    queries: Option<Vec<String>>,
}
#[derive(Clone)]
struct Fixture {
    tool_name: String,
    bodies: Vec<Value>,
    queries: Vec<String>,
    requests: Arc<Mutex<usize>>,
}
impl Transport for Fixture {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        assert_eq!(request.method.as_str(), "GET");
        assert_eq!(
            request.url,
            if self.tool_name == "illust_related" {
                "https://app-api.pixiv.net/v2/illust/related"
            } else {
                "https://app-api.pixiv.net/v1/illust/recommended"
            }
        );
        let mut requests = self.requests.lock().unwrap();
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
async fn artwork_feed_matches_go_schemas_records_errors_and_saved_account_stdio() {
    let contract: Contract = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/mcp-artwork-feed.json"
    ))
    .unwrap();
    assert_eq!(
        pixiv_mcp::illust_related_tool(),
        contract.tools["illust_related"]
    );
    assert_eq!(
        pixiv_mcp::illust_recommended_tool(),
        contract.tools["illust_recommended"]
    );
    assert_eq!(contract.cases.len(), 114);
    for case in contract.cases {
        let fixture = Fixture {
            tool_name: case.tool_name.clone(),
            bodies: case.bodies,
            queries: case.queries.unwrap_or_default(),
            requests: Arc::new(Mutex::new(0)),
        };
        let client = Client::with_transport("fixture-access", fixture.clone());
        if case.rpc_error.is_empty() {
            let result = serde_json::to_value(if case.tool_name == "illust_related" {
                pixiv_mcp::illust_related(
                    &client,
                    serde_json::from_value(case.arguments.clone()).unwrap(),
                )
                .await
            } else {
                pixiv_mcp::illust_recommended(
                    &client,
                    serde_json::from_value(case.arguments.clone()).unwrap(),
                )
                .await
            })
            .unwrap();
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
            assert_eq!(case.calls, usize::from(case.requests > 0), "{}", case.name);
        }
    }
}
