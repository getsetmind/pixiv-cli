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
    tool: String,
    arguments: Value,
    status: u16,
    body: String,
    result: Value,
    calls: usize,
    requests: Vec<String>,
    rpc_error: String,
}
#[derive(Clone)]
struct Fixture {
    status: u16,
    body: String,
    seen: Arc<Mutex<Vec<String>>>,
}
impl Transport for Fixture {
    async fn send(&self, mut request: Request) -> pixiv_sdk::Result<Response> {
        assert_eq!(request.method.as_str(), "POST");
        request.parameters.sort_by(|a, b| a.0.cmp(&b.0));
        let form = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(&request.parameters)
            .finish();
        self.seen.lock().unwrap().push(format!(
            "POST {} {form}",
            request
                .url
                .strip_prefix("https://app-api.pixiv.net")
                .unwrap()
        ));
        Ok(Response {
            status: self.status,
            retry_after: Some(chrono::TimeDelta::zero()),
            body: serde_json::from_str(&self.body)
                .unwrap_or_else(|_| Value::String(self.body.clone())),
        })
    }
}
#[tokio::test]
async fn comment_mutations_match_go_schemas_results_forms_and_saved_stdio() {
    let contract: Contract = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/mcp-comment-mutations.json"
    ))
    .unwrap();
    assert_eq!(contract.cases.len(), 99);
    for (name, schema) in contract.tools {
        assert_eq!(
            pixiv_mcp::comment_mutation_tool(pixiv_mcp::CommentMutation::from_name(&name).unwrap()),
            schema
        );
    }
    for case in contract.cases {
        let action = pixiv_mcp::CommentMutation::from_name(&case.tool).unwrap();
        let fixture = Fixture {
            status: case.status,
            body: case.body,
            seen: Arc::new(Mutex::new(vec![])),
        };
        let client = Client::with_transport("fixture-access", fixture.clone());
        if case.rpc_error.is_empty() {
            let result = pixiv_mcp::comment_mutation(
                &client,
                action,
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
                *fixture.seen.lock().unwrap(),
                case.requests,
                "{} direct requests",
                case.name
            );
        }
        for saved in [false, true] {
            fixture.seen.lock().unwrap().clear();
            let messages = [
                json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}),
                json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":case.tool,"arguments":case.arguments}}),
            ];
            let input = messages
                .iter()
                .map(|message| message.to_string() + "\n")
                .collect::<String>();
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
            let response = responses
                .iter()
                .find(|response| response["id"] == 2)
                .unwrap();
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
                    "{} saved={saved}",
                    case.name
                );
            }
            assert_eq!(
                *fixture.seen.lock().unwrap(),
                case.requests,
                "{} saved={saved}",
                case.name
            );
            assert_eq!(
                case.calls,
                usize::from(case.rpc_error.is_empty()),
                "{}",
                case.name
            );
        }
    }
}
