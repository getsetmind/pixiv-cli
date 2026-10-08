use pixiv_mcp::{search_illust_tool, stdio::serve};
use pixiv_sdk::{
    Client, Result,
    transport::{Request, Response, Transport},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

type Query = BTreeMap<String, Vec<String>>;

#[derive(Deserialize)]
struct Contract {
    tool: Value,
    cases: Vec<Case>,
}
#[derive(Deserialize)]
struct Case {
    name: String,
    arguments: Value,
    bodies: Vec<Value>,
    result: Value,
    queries: Vec<BTreeMap<String, Vec<String>>>,
    rpc_error: String,
}
#[derive(Clone)]
struct Fixture {
    bodies: Vec<Value>,
    seen: Arc<Mutex<Vec<Query>>>,
    database: Option<Arc<Mutex<pixiv_app::database::Database>>>,
}
impl Transport for Fixture {
    async fn send(&self, request: Request) -> Result<Response> {
        if let Some(database) = &self.database {
            if request.operation == "Open" {
                assert!(
                    request
                        .parameters
                        .iter()
                        .any(|(key, value)| key == "refresh_token" && value == "fixture-refresh")
                );
                return Ok(Response {
                    status: 200,
                    retry_after: None,
                    body: json!({"access_token":"fixture-access","refresh_token":"fixture-rotated","expires_in":3600,"user":{"id":42}}),
                });
            }
            let account = database.lock().unwrap().get_pixiv(42).unwrap();
            assert_eq!(account.credential_revision, 2);
            assert_eq!(account.refresh_token_copy(), b"fixture-rotated");
        }
        assert_eq!(request.method, "GET");
        assert_eq!(request.url, "https://app-api.pixiv.net/v1/search/illust");
        let mut query = BTreeMap::<String, Vec<String>>::new();
        for (key, value) in request.parameters {
            query.entry(key).or_default().push(value);
        }
        let index = match query
            .get("offset")
            .and_then(|values| values.first())
            .map(String::as_str)
        {
            Some("30") => 1,
            Some("60") => 2,
            _ => 0,
        };
        self.seen.lock().unwrap().push(query);
        Ok(Response {
            status: 200,
            retry_after: None,
            body: self.bodies.get(index).unwrap_or(&self.bodies[0]).clone(),
        })
    }
}
#[tokio::test]
async fn search_tool_matches_go_schema_logical_pages_filters_and_bookmark_strategies() {
    let contract: Contract = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/mcp-search.json"
    ))
    .unwrap();
    assert_eq!(search_illust_tool(), contract.tool);
    assert_eq!(contract.cases.len(), 144);
    for case in contract.cases {
        for saved in [false, true] {
            use pixiv_app::{
                config::Store,
                database::{Database, PixivAccount},
                execution::Execution,
            };
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("config.toml");
            std::fs::write(&path, "").unwrap();
            let mut database = Database::open(directory.path()).unwrap();
            database
                .save_pixiv_credential(&PixivAccount::new(42, "fixture", b"fixture-refresh"))
                .unwrap();
            let database = Arc::new(Mutex::new(database));
            let seen = Arc::new(Mutex::new(vec![]));
            let fixture = Fixture {
                bodies: case.bodies.clone(),
                seen: seen.clone(),
                database: if saved { Some(database.clone()) } else { None },
            };
            let client = Client::with_transport("fixture-access", fixture.clone());
            let init = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}});
            let call = json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"search_illust","arguments":case.arguments}});
            let input = init.to_string() + "\n" + &call.to_string() + "\n";
            let mut output = vec![];
            if saved {
                let execution =
                    Execution::new(Store::new(path), database, move |_| Ok(fixture.clone()));
                pixiv_mcp::stdio::serve_saved(&execution, input.as_bytes(), &mut output)
                    .await
                    .unwrap();
            } else {
                serve(&client, input.as_bytes(), &mut output).await.unwrap();
            }
            let responses: Vec<Value> = String::from_utf8(output)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            let actual = responses
                .iter()
                .find(|response| response["id"] == 7)
                .unwrap();
            if case.rpc_error.is_empty() {
                assert_eq!(actual["result"], case.result, "{}", case.name);
            } else {
                assert_eq!(actual["error"]["code"], -32602, "{}", case.name);
                assert_eq!(
                    actual["error"]["message"],
                    case.rpc_error
                        .strip_prefix("calling \"tools/call\": ")
                        .unwrap(),
                    "{}",
                    case.name
                );
            }
            assert_eq!(
                *seen.lock().unwrap(),
                case.queries,
                "{} requests",
                case.name
            );
        }
    }
}
