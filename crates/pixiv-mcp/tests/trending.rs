use pixiv_sdk::{
    Client,
    transport::{Request, Response, Transport},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[path = "support/saved_account.rs"]
mod saved_account;

#[derive(Deserialize)]
struct Contract {
    schema: Value,
    cases: Vec<Case>,
}
#[derive(Deserialize)]
struct Case {
    body: Value,
    result: Value,
    requests: usize,
}
#[derive(Clone)]
struct Fixture {
    body: Value,
    requests: Arc<AtomicUsize>,
}
impl Transport for Fixture {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        assert_eq!(request.method.as_str(), "GET");
        assert_eq!(
            request.url,
            "https://app-api.pixiv.net/v1/trending-tags/illust"
        );
        assert!(request.parameters.is_empty());
        self.requests.fetch_add(1, Ordering::SeqCst);
        Ok(Response {
            status: 200,
            retry_after: None,
            body: self.body.clone(),
        })
    }
}
#[tokio::test]
async fn trending_tool_schema_and_rpc_results_match_go() {
    let contract: Contract = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/mcp-trending.json"
    ))
    .unwrap();
    assert_eq!(pixiv_mcp::trending_tags_illust_tool(), contract.schema);
    for case in contract.cases {
        let fixture = Fixture {
            body: case.body,
            requests: Arc::new(AtomicUsize::new(0)),
        };
        let client = Client::with_transport("fixture-access", fixture.clone());
        let direct = pixiv_mcp::trending_tags_illust(&client).await;
        assert_eq!(serde_json::to_value(direct).unwrap(), case.result);
        assert_eq!(fixture.requests.swap(0, Ordering::SeqCst), case.requests);
        let messages = [
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"trending_tags_illust","arguments":{}}}),
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
        assert_eq!(fixture.requests.load(Ordering::SeqCst), case.requests);
        fixture.requests.store(0, Ordering::SeqCst);
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
        assert_eq!(fixture.requests.load(Ordering::SeqCst), case.requests);
    }
}
