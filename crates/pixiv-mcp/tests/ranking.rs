use pixiv_sdk::{
    Client,
    transport::{Request, Response, Transport},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

#[path = "support/saved_account.rs"]
mod saved_account;

#[derive(Deserialize)]
struct Contract {
    schema: Value,
    cases: Vec<Case>,
}
#[derive(Deserialize)]
struct Case {
    input: Value,
    bodies: Vec<Value>,
    result: Value,
    queries: Vec<std::collections::BTreeMap<String, Vec<String>>>,
}
type Queries = Arc<std::sync::Mutex<Vec<std::collections::BTreeMap<String, Vec<String>>>>>;
#[derive(Clone)]
struct Fixture {
    bodies: Vec<Value>,
    queries: Queries,
}
impl Transport for Fixture {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        assert_eq!(request.method.as_str(), "GET");
        assert_eq!(request.url, "https://app-api.pixiv.net/v1/illust/ranking");
        let mut query = std::collections::BTreeMap::<String, Vec<String>>::new();
        for (key, value) in request.parameters {
            query.entry(key).or_default().push(value);
        }
        let index = usize::from(self.bodies.len() > 1 && query.contains_key("offset"));
        self.queries.lock().unwrap().push(query);
        Ok(Response {
            status: 200,
            retry_after: None,
            body: self.bodies[index].clone(),
        })
    }
}
#[tokio::test]
async fn ranking_tool_preserves_go_schema_filter_windows_results_and_saved_accounts() {
    let contract: Contract = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/mcp-ranking.json"
    ))
    .unwrap();
    assert_eq!(pixiv_mcp::illust_ranking_tool(), contract.schema);
    for case in contract.cases {
        let fixture = Fixture {
            bodies: case.bodies,
            queries: Arc::new(std::sync::Mutex::new(vec![])),
        };
        let client = Client::with_transport("fixture-access", fixture.clone());
        let direct =
            pixiv_mcp::illust_ranking(&client, serde_json::from_value(case.input.clone()).unwrap())
                .await;
        assert_eq!(serde_json::to_value(direct).unwrap(), case.result);
        assert_eq!(*fixture.queries.lock().unwrap(), case.queries);
        fixture.queries.lock().unwrap().clear();
        let messages = [
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"illust_ranking","arguments":case.input}}),
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
        assert_eq!(
            responses.iter().find(|value| value["id"] == 2).unwrap()["result"],
            case.result
        );
        assert_eq!(*fixture.queries.lock().unwrap(), case.queries);
        fixture.queries.lock().unwrap().clear();
        let accounts = saved_account::saved_execution(fixture.clone());
        let mut output = vec![];
        pixiv_mcp::stdio::serve_saved(&accounts.execution, input.as_bytes(), &mut output)
            .await
            .unwrap();
        let responses = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            responses.iter().find(|value| value["id"] == 2).unwrap()["result"],
            case.result
        );
        assert_eq!(*fixture.queries.lock().unwrap(), case.queries);
    }
}
