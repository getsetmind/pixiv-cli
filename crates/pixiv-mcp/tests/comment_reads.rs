#[path = "support/implemented_catalog.rs"]
mod implemented_catalog;
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
    bodies: std::collections::BTreeMap<String, Value>,
    tools: std::collections::BTreeMap<String, Value>,
    cases: Vec<Case>,
}
#[derive(Deserialize)]
struct Case {
    #[serde(default)]
    unknown_identity: bool,
    name: String,
    arguments: Value,
    tool_name: String,
    body_keys: Vec<String>,
    result: Value,
    calls: usize,
    requests: usize,
    rpc_error: String,
    queries: Option<Vec<String>>,
}
#[derive(Clone)]
struct Fixture {
    authorization: &'static str,
    tool_name: String,
    bodies: Vec<Value>,
    queries: Vec<String>,
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
        }
        assert_eq!(request.method.as_str(), "GET");
        assert_eq!(
            request.url,
            match self.tool_name.as_str() {
                "illust_comments" => "https://app-api.pixiv.net/v3/illust/comments",
                "novel_comments" => "https://app-api.pixiv.net/v2/novel/comments",
                _ => panic!("unexpected tool"),
            }
        );
        assert_eq!(
            request
                .headers
                .iter()
                .find(|(name, _)| name == "Authorization")
                .unwrap()
                .1,
            self.authorization
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
async fn comment_read_matches_go_schemas_comments_errors_and_saved_account_stdio() {
    let contract: Contract = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/mcp-comment-reads.json"
    ))
    .unwrap();
    assert_eq!(contract.cases.len(), 72);
    for name in ["illust_comments", "novel_comments"] {
        assert_eq!(
            pixiv_mcp::comment_read_tool(pixiv_mcp::CommentRead::from_name(name).unwrap()),
            contract.tools[name]
        );
    }
    for case in contract.cases {
        let fixture = Fixture {
            authorization: "Bearer fixture-access",
            tool_name: case.tool_name.clone(),
            bodies: case
                .body_keys
                .iter()
                .map(|key| contract.bodies[key].clone())
                .collect(),
            queries: case.queries.unwrap_or_default(),
            requests: Arc::new(Mutex::new(0)),
        };
        let credentials = pixiv_sdk::oauth::refresh(&fixture, "fixture-refresh")
            .await
            .unwrap();
        let client = if case.unknown_identity {
            Client::with_transport("fixture-access", fixture.clone())
        } else {
            Client::from_credentials(&credentials, fixture.clone())
        };
        if case.rpc_error.is_empty() {
            let result = serde_json::to_value(
                pixiv_mcp::comment_read(
                    &client,
                    pixiv_mcp::CommentRead::from_name(&case.tool_name).unwrap(),
                    if case.arguments.is_null() {
                        pixiv_mcp::CommentReadInput::default()
                    } else {
                        serde_json::from_value(case.arguments.clone()).unwrap()
                    },
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
        let messages = [
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}),
            json!({"jsonrpc":"2.0","id":3,"method":"tools/list"}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":case.tool_name,"arguments":case.arguments}}),
        ];
        let input = messages
            .iter()
            .map(|message| message.to_string() + "\n")
            .collect::<String>();
        for saved in [false, true] {
            if saved && case.unknown_identity {
                continue;
            }
            *fixture.requests.lock().unwrap() = 0;
            let mut output = vec![];
            if saved {
                let saved_fixture = Fixture {
                    authorization: "Bearer fixture-access-42",
                    ..fixture.clone()
                };
                let accounts = saved_account::saved_execution(saved_fixture);
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
            implemented_catalog::assert_catalog(
                responses.iter().find(|value| value["id"] == 3).unwrap()["result"]["tools"]
                    .as_array()
                    .unwrap(),
            );
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
                usize::from(case.requests > 0 || case.unknown_identity),
                "{}",
                case.name
            );
        }
    }
}
