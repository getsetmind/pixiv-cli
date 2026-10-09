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
            body: Value::String(self.body.clone()),
        })
    }
}
#[tokio::test]
async fn bookmarks_and_follows_match_go_schemas_results_forms_and_saved_stdio_without_replay() {
    let contract: Contract = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/mcp-bookmark-follow.json"
    ))
    .unwrap();
    assert_eq!(contract.cases.len(), 172);
    for (name, schema) in contract.tools {
        assert_eq!(
            pixiv_mcp::mutation_tool(pixiv_mcp::MutationAction::from_name(&name).unwrap()),
            schema
        );
    }
    for case in contract.cases {
        let action = pixiv_mcp::MutationAction::from_name(&case.tool).unwrap();
        let fixture = Fixture {
            status: case.status,
            body: case.body,
            seen: Arc::new(Mutex::new(vec![])),
        };
        let client = Client::with_transport("fixture-access", fixture.clone());
        if case.rpc_error.is_empty() {
            let result = pixiv_mcp::mutate(
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

#[derive(Clone)]
struct PoolFixture {
    seen: Arc<Mutex<Vec<String>>>,
}
impl Transport for PoolFixture {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        if request.operation == "Open" {
            let refresh = &request
                .parameters
                .iter()
                .find(|(key, _)| key == "refresh_token")
                .unwrap()
                .1;
            let id = refresh.rsplit('-').next().unwrap().parse::<i64>().unwrap();
            return Ok(Response {
                status: 200,
                retry_after: None,
                body: json!({"access_token":format!("fixture-access-{id}"),"refresh_token":format!("fixture-rotated-{id}"),"expires_in":3600,"user":{"id":id}}),
            });
        }
        assert_eq!(request.method.as_str(), "POST");
        self.seen.lock().unwrap().push(request.operation.to_owned());
        Ok(Response {
            status: 429,
            retry_after: Some(chrono::TimeDelta::zero()),
            body: json!({}),
        })
    }
}
#[tokio::test]
async fn saved_pool_mutations_do_not_replay_rate_limited_requests() {
    use pixiv_app::{
        config::Store,
        database::{Database, PixivAccount},
        execution::Execution,
    };
    let contract: Contract = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/mcp-bookmark-follow.json"
    ))
    .unwrap();
    for name in [
        "add_bookmark",
        "remove_bookmark",
        "add_novel_bookmark",
        "remove_novel_bookmark",
        "follow_user",
        "unfollow_user",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(
            &path,
            "[account_pool]\nenabled = true\nstrategy = 'round_robin'\n",
        )
        .unwrap();
        let mut database = Database::open(directory.path()).unwrap();
        for id in [42, 43] {
            database
                .save_pixiv_credential(&PixivAccount::new(
                    id,
                    "fixture",
                    format!("fixture-refresh-{id}").as_bytes(),
                ))
                .unwrap();
        }
        database.set_all_pixiv_schedulable(true).unwrap();
        let seen = Arc::new(Mutex::new(vec![]));
        let fixture = PoolFixture { seen: seen.clone() };
        let execution = Execution::new(
            Store::new(path),
            Arc::new(Mutex::new(database)),
            move |_| Ok(fixture.clone()),
        );
        let field = if name.contains("novel") {
            "novel_id"
        } else if name.contains("user") {
            "user_id"
        } else {
            "illust_id"
        };
        let mut arguments = json!({});
        arguments[field] = json!(42);
        let messages = [
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":name,"arguments":arguments}}),
        ];
        let input = messages
            .iter()
            .map(|message| message.to_string() + "\n")
            .collect::<String>();
        let mut output = vec![];
        pixiv_mcp::stdio::serve_saved(&execution, input.as_bytes(), &mut output)
            .await
            .unwrap();
        let responses = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        let result = &responses
            .iter()
            .find(|response| response["id"] == 2)
            .unwrap()["result"];
        let expected = contract
            .cases
            .iter()
            .find(|case| case.tool == name && case.status == 429)
            .unwrap();
        assert_eq!(*result, expected.result, "{name} pooled result");
        assert_eq!(seen.lock().unwrap().len(), 1, "{name} must not replay");
    }
}
