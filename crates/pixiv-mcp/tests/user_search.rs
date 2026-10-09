use pixiv_sdk::{
    Client,
    transport::{Request, Response, Transport},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};

#[path = "support/saved_account.rs"]
mod saved_account;

#[derive(Deserialize)]
struct Contract {
    schema: Value,
    cases: Vec<Case>,
}
#[derive(Deserialize)]
struct Case {
    name: String,
    arguments: Value,
    bodies: Vec<Value>,
    result: Value,
    queries: Vec<BTreeMap<String, Vec<String>>>,
    calls: usize,
    rpc_error: String,
}
type Queries = Arc<std::sync::Mutex<Vec<BTreeMap<String, Vec<String>>>>>;
#[derive(Clone)]
struct Fixture {
    bodies: Vec<Value>,
    queries: Queries,
}
impl Transport for Fixture {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        assert_eq!(request.method.as_str(), "GET");
        assert_eq!(request.url, "https://app-api.pixiv.net/v1/search/user");
        let mut query = BTreeMap::<String, Vec<String>>::new();
        for (key, value) in request.parameters {
            query.entry(key).or_default().push(value);
        }
        let index = query
            .get("offset")
            .map_or(0, |values| values[0].parse::<usize>().unwrap() / 30);
        self.queries.lock().unwrap().push(query);
        Ok(Response {
            status: 200,
            retry_after: None,
            body: self.bodies[index].clone(),
        })
    }
}
fn responses(output: Vec<u8>) -> Vec<Value> {
    String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
fn check_response(case: &Case, output: Vec<u8>) {
    let responses = responses(output);
    let response = responses.iter().find(|value| value["id"] == 2).unwrap();
    if case.rpc_error.is_empty() {
        assert_eq!(response["result"], case.result, "{}", case.name);
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
}
#[tokio::test]
async fn user_search_preserves_go_schema_records_filters_pages_and_saved_account_validation() {
    let contract: Contract = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/mcp-user-search.json"
    ))
    .unwrap();
    assert_eq!(pixiv_mcp::search_user_tool(), contract.schema);
    assert_eq!(contract.cases.len(), 182);
    for case in contract.cases {
        let fixture = Fixture {
            bodies: case.bodies.clone(),
            queries: Arc::new(std::sync::Mutex::new(vec![])),
        };
        let client = Client::with_transport("fixture-access", fixture.clone());
        if case.rpc_error.is_empty() {
            let direct = pixiv_mcp::search_user(
                &client,
                serde_json::from_value(case.arguments.clone()).unwrap(),
            )
            .await;
            assert_eq!(
                serde_json::to_value(direct).unwrap(),
                case.result,
                "{}",
                case.name
            );
            assert_eq!(
                *fixture.queries.lock().unwrap(),
                case.queries,
                "{}",
                case.name
            );
            fixture.queries.lock().unwrap().clear();
        }
        let messages = [
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_user","arguments":case.arguments}}),
        ];
        let input = messages
            .iter()
            .map(|message| message.to_string() + "\n")
            .collect::<String>();
        let mut output = vec![];
        pixiv_mcp::stdio::serve(&client, input.as_bytes(), &mut output)
            .await
            .unwrap();
        check_response(&case, output);
        assert_eq!(
            *fixture.queries.lock().unwrap(),
            case.queries,
            "{}",
            case.name
        );
        fixture.queries.lock().unwrap().clear();
        let accounts = saved_account::saved_execution(fixture.clone());
        let mut output = vec![];
        pixiv_mcp::stdio::serve_saved(&accounts.execution, input.as_bytes(), &mut output)
            .await
            .unwrap();
        check_response(&case, output);
        assert_eq!(
            *fixture.queries.lock().unwrap(),
            case.queries,
            "{}",
            case.name
        );
        assert_eq!(
            accounts._opens.load(std::sync::atomic::Ordering::SeqCst),
            case.calls,
            "{}",
            case.name
        );
    }
}
