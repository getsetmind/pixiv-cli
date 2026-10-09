use pixiv_mcp::stdio::serve;
use pixiv_sdk::{
    Client, Result,
    transport::{Request, Response, Transport},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Mutex;

#[derive(Deserialize)]
struct Case {
    name: String,
    request: Value,
    response: Value,
    requests: usize,
    calls: usize,
}
struct Fixture<'a> {
    body: Value,
    requests: &'a Mutex<usize>,
}
impl Transport for Fixture<'_> {
    async fn send(&self, _: Request) -> Result<Response> {
        *self.requests.lock().unwrap() += 1;
        Ok(Response {
            status: 200,
            retry_after: None,
            body: self.body.clone(),
        })
    }
}
fn initialize(id: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"migration","version":"0"}}})
}

#[derive(Deserialize)]
struct DetailContract {
    cases: Vec<DetailCase>,
}
#[derive(Deserialize)]
struct DetailCase {
    name: String,
    arguments: Value,
    body: Value,
    result: Value,
    requests: usize,
    rpc_error: String,
    calls: usize,
}

#[derive(Clone)]
struct SavedFixture {
    body: Value,
    requests: std::sync::Arc<Mutex<usize>>,
    opens: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    database: std::sync::Arc<Mutex<pixiv_app::database::Database>>,
}
impl Transport for SavedFixture {
    async fn send(&self, request: Request) -> Result<Response> {
        if request.operation == "Open" {
            self.opens.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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
        let account = self.database.lock().unwrap().get_pixiv(42).unwrap();
        assert_eq!(account.credential_revision, 2);
        assert_eq!(account.refresh_token_copy(), b"fixture-rotated");
        *self.requests.lock().unwrap() += 1;
        Ok(Response {
            status: 200,
            retry_after: None,
            body: self.body.clone(),
        })
    }
}

#[tokio::test]
async fn saved_account_stdio_detail_preserves_go_results_and_persists_refresh_before_content() {
    use pixiv_app::{
        config::Store,
        database::{Database, PixivAccount},
        execution::Execution,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let contract: DetailContract = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/mcp-detail.json"
    ))
    .unwrap();
    assert_eq!(contract.cases.len(), 33);
    for case in contract.cases {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(&path, "").unwrap();
        let mut database = Database::open(directory.path()).unwrap();
        database
            .save_pixiv_credential(&PixivAccount::new(42, "fixture", b"fixture-refresh"))
            .unwrap();
        let database = Arc::new(Mutex::new(database));
        let requests = Arc::new(Mutex::new(0));
        let opens = Arc::new(AtomicUsize::new(0));
        let fixture = SavedFixture {
            body: case.body,
            requests: requests.clone(),
            opens: opens.clone(),
            database: database.clone(),
        };
        let execution = Execution::new(Store::new(path), database, move |_| Ok(fixture.clone()));
        let call = json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"illust_detail","arguments":case.arguments}});
        let input = initialize(1.into()).to_string() + "\n" + &call.to_string() + "\n";
        let mut output = Vec::new();
        pixiv_mcp::stdio::serve_saved(&execution, input.as_bytes(), &mut output)
            .await
            .unwrap();
        let responses = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        let response = responses
            .iter()
            .find(|response| response["id"] == 7)
            .unwrap();
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
        assert_eq!(*requests.lock().unwrap(), case.requests, "{}", case.name);
        assert_eq!(opens.load(Ordering::SeqCst), case.calls, "{}", case.name);
    }
}

#[tokio::test]
async fn stdio_detail_calls_match_all_go_records_and_numeric_input_rejection() {
    let contract: DetailContract = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/mcp-detail.json"
    ))
    .unwrap();
    assert_eq!(contract.cases.len(), 33);
    for case in contract.cases {
        let requests = Mutex::new(0);
        let client = Client::with_transport(
            "fixture-access",
            Fixture {
                body: case.body,
                requests: &requests,
            },
        );
        let call = json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"illust_detail","arguments":case.arguments}});
        let input = initialize(1.into()).to_string() + "\n" + &call.to_string() + "\n";
        let mut output = vec![];
        serve(&client, input.as_bytes(), &mut output).await.unwrap();
        let responses = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        let response = responses
            .iter()
            .find(|response| response["id"] == 7)
            .unwrap();
        if case.rpc_error.is_empty() {
            assert_eq!(response["result"], case.result, "{}", case.name);
        } else {
            assert_eq!(response["error"]["code"], -32602);
            assert_eq!(
                response["error"]["message"],
                case.rpc_error
                    .strip_prefix("calling \"tools/call\": ")
                    .unwrap(),
                "{}",
                case.name
            );
        }
        assert_eq!(*requests.lock().unwrap(), case.requests, "{}", case.name);
    }
}

#[tokio::test]
async fn saved_proxy_override_reaches_detail_and_search_account_connections() {
    use pixiv_app::{
        config::Store,
        database::{Database, PixivAccount},
        execution::Execution,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let detail: Value = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/mcp-detail.json"
    ))
    .unwrap();
    let search: Value = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/mcp-search.json"
    ))
    .unwrap();
    let detail = detail["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["requests"] == 1 && case["result"]["isError"] != true)
        .unwrap();
    let search = search["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| {
            case["queries"].as_array().unwrap().len() == 1 && case["result"]["isError"] != true
        })
        .unwrap();
    for (tool, case, body) in [
        ("illust_detail", detail, &detail["body"]),
        ("search_illust", search, &search["bodies"][0]),
    ] {
        for proxy in [None, Some(""), Some("http://override.invalid")] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("config.toml");
            std::fs::write(
                &path,
                "[pixiv.network]\nproxy_url = 'http://configured.invalid'\n",
            )
            .unwrap();
            let mut database = Database::open(directory.path()).unwrap();
            database
                .save_pixiv_credential(&PixivAccount::new(42, "fixture", b"fixture-refresh"))
                .unwrap();
            let database = Arc::new(Mutex::new(database));
            let requests = Arc::new(Mutex::new(0));
            let opens = Arc::new(AtomicUsize::new(0));
            let fixture = SavedFixture {
                body: body.clone(),
                requests: requests.clone(),
                opens: opens.clone(),
                database: database.clone(),
            };
            let connections = Arc::new(Mutex::new(vec![]));
            let selected = connections.clone();
            let execution = Execution::new(Store::new(path), database, move |connection| {
                selected.lock().unwrap().push(connection.proxy().to_owned());
                Ok(fixture.clone())
            });
            let call = json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":tool,"arguments":case["arguments"]}});
            let input = initialize(1.into()).to_string() + "\n" + &call.to_string() + "\n";
            let mut output = Vec::new();
            pixiv_mcp::stdio::serve_saved_with_proxy(
                &execution,
                proxy,
                input.as_bytes(),
                &mut output,
            )
            .await
            .unwrap();
            let responses: Vec<Value> = String::from_utf8(output)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            let response = responses
                .iter()
                .find(|response| response["id"] == 7)
                .unwrap();
            assert_eq!(response["result"], case["result"], "{tool} {proxy:?}");
            assert_eq!(
                *connections.lock().unwrap(),
                [proxy.unwrap_or("http://configured.invalid")]
            );
            assert_eq!(opens.load(Ordering::SeqCst), 1);
            assert_eq!(*requests.lock().unwrap(), 1);
        }
    }
}
#[tokio::test]
async fn stdio_matches_go_initialization_protocol_and_detail_argument_errors() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/mcp-rpc.json"
    ))
    .unwrap();
    let artwork: Vec<Value> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/artwork-detail.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 30);
    for case in cases.into_iter().filter(|case| case.name != "cancelled") {
        let requests = Mutex::new(0);
        let client = Client::with_transport(
            "fixture-access",
            Fixture {
                body: artwork[0]["body"].clone(),
                requests: &requests,
            },
        );
        let mut messages = vec![];
        if !case.name.starts_with("initialize-") {
            messages.push(initialize(1.into()));
            messages.push(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        }
        messages.push(case.request.clone());
        let input = messages
            .into_iter()
            .map(|message| message.to_string() + "\n")
            .collect::<String>();
        let mut output = vec![];
        serve(&client, input.as_bytes(), &mut output).await.unwrap();
        let responses = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        let response = responses
            .iter()
            .find(|response| response["id"] == case.request["id"])
            .unwrap();
        assert_eq!(response, &case.response, "{}", case.name);
        assert_eq!(*requests.lock().unwrap(), case.requests, "{}", case.name);
        assert_eq!(case.calls, case.requests, "{}", case.name);
        assert_eq!(
            responses.len(),
            if case.name.starts_with("initialize-") {
                1
            } else {
                2
            }
        );
    }
}

struct Blocked {
    started: tokio::sync::Notify,
    dropped: std::sync::atomic::AtomicUsize,
}

#[derive(Clone)]
struct SavedCancellation {
    blocked: std::sync::Arc<Blocked>,
    requests: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    opens: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    body: Value,
}
impl Transport for SavedCancellation {
    async fn send(&self, request: Request) -> Result<Response> {
        use std::sync::atomic::Ordering;
        if request.operation == "Open" {
            let attempt = self.opens.fetch_add(1, Ordering::SeqCst);
            let expected = if attempt == 0 {
                "fixture-refresh"
            } else {
                "fixture-rotated"
            };
            assert!(
                request
                    .parameters
                    .iter()
                    .any(|(key, value)| key == "refresh_token" && value == expected)
            );
            return Ok(Response {
                status: 200,
                retry_after: None,
                body: json!({"access_token":"fixture-access","refresh_token":"fixture-rotated","expires_in":3600,"user":{"id":42}}),
            });
        }
        if self.requests.fetch_add(1, Ordering::SeqCst) == 0 {
            let _guard = RequestGuard(&self.blocked.dropped);
            self.blocked.started.notify_one();
            return std::future::pending().await;
        }
        Ok(Response {
            status: 200,
            retry_after: None,
            body: self.body.clone(),
        })
    }
}

#[tokio::test]
async fn saved_account_cancellation_releases_gate_and_reuses_persisted_refresh() {
    use pixiv_app::{
        config::Store,
        database::{Database, PixivAccount},
        execution::Execution,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/mcp-rpc.json"
    ))
    .unwrap();
    let cancelled = cases
        .into_iter()
        .find(|case| case.name == "cancelled")
        .unwrap();
    let detail: DetailContract = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/mcp-detail.json"
    ))
    .unwrap();
    let detail = detail
        .cases
        .into_iter()
        .find(|case| {
            case.arguments == json!({"illust_id":42})
                && case.requests == 1
                && case.rpc_error.is_empty()
        })
        .unwrap();
    for pooled in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(
            &path,
            if pooled {
                "[account_pool]\nenabled = true\nstrategy = 'round_robin'\n"
            } else {
                ""
            },
        )
        .unwrap();
        let mut database = Database::open(directory.path()).unwrap();
        database
            .save_pixiv_credential(&PixivAccount::new(42, "fixture", b"fixture-refresh"))
            .unwrap();
        if pooled {
            database.set_all_pixiv_schedulable(true).unwrap();
        }
        let database = Arc::new(Mutex::new(database));
        let blocked = Arc::new(Blocked {
            started: tokio::sync::Notify::new(),
            dropped: AtomicUsize::new(0),
        });
        let fixture = SavedCancellation {
            blocked: blocked.clone(),
            requests: Arc::new(AtomicUsize::new(0)),
            opens: Arc::new(AtomicUsize::new(0)),
            body: detail.body.clone(),
        };
        let transport = fixture.clone();
        let execution = Execution::new(Store::new(path), database.clone(), move |_| {
            Ok(transport.clone())
        });
        let (server, peer) = tokio::io::duplex(4096);
        let (server_read, mut server_write) = tokio::io::split(server);
        let (peer_read, mut peer_write) = tokio::io::split(peer);
        let server = pixiv_mcp::stdio::serve_saved(&execution, server_read, &mut server_write);
        let peer = async {
            let mut lines = BufReader::new(peer_read).lines();
            peer_write
                .write_all((initialize(1.into()).to_string() + "\n").as_bytes())
                .await
                .unwrap();
            let _ = lines.next_line().await.unwrap().unwrap();
            peer_write
                .write_all((cancelled.request.to_string() + "\n").as_bytes())
                .await
                .unwrap();
            blocked.started.notified().await;
            assert_eq!(
                database
                    .lock()
                    .unwrap()
                    .get_pixiv(42)
                    .unwrap()
                    .credential_revision,
                2
            );
            peer_write.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{\"requestId\":7}}\n").await.unwrap();
            let response: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(response, cancelled.response);
            assert_eq!(blocked.dropped.load(Ordering::SeqCst), 1);
            let account = database.lock().unwrap().get_pixiv(42).unwrap();
            assert_eq!(account.pool_frozen_until, None);
            assert_eq!(account.pool_last_selected, pooled);
            let call = json!({"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"illust_detail","arguments":detail.arguments}});
            peer_write
                .write_all((call.to_string() + "\n").as_bytes())
                .await
                .unwrap();
            let response: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(response["result"], detail.result);
            peer_write.shutdown().await.unwrap();
        };
        let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            tokio::join!(server, peer)
        })
        .await
        .unwrap();
        result.unwrap();
        assert_eq!(fixture.opens.load(Ordering::SeqCst), 2);
        assert_eq!(fixture.requests.load(Ordering::SeqCst), 2);
        assert_eq!(
            database
                .lock()
                .unwrap()
                .get_pixiv(42)
                .unwrap()
                .credential_revision,
            3
        );
    }
}
struct RequestGuard<'a>(&'a std::sync::atomic::AtomicUsize);
impl Drop for RequestGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}
impl Transport for &Blocked {
    async fn send(&self, _: Request) -> Result<Response> {
        let _guard = RequestGuard(&self.dropped);
        self.started.notify_one();
        std::future::pending().await
    }
}

#[tokio::test]
async fn stdio_cancels_inflight_io_while_ping_remains_responsive() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/mcp-rpc.json"
    ))
    .unwrap();
    let case = cases
        .into_iter()
        .find(|case| case.name == "cancelled")
        .unwrap();
    let transport = Blocked {
        started: tokio::sync::Notify::new(),
        dropped: std::sync::atomic::AtomicUsize::new(0),
    };
    let client = Client::with_transport("fixture-access", &transport);
    let (server, peer) = tokio::io::duplex(4096);
    let (server_read, mut server_write) = tokio::io::split(server);
    let (peer_read, mut peer_write) = tokio::io::split(peer);
    let server = serve(&client, server_read, &mut server_write);
    let peer = async {
        let mut lines = BufReader::new(peer_read).lines();
        peer_write
            .write_all((initialize(1.into()).to_string() + "\n").as_bytes())
            .await
            .unwrap();
        let _ = lines.next_line().await.unwrap().unwrap();
        peer_write
            .write_all((case.request.to_string() + "\n").as_bytes())
            .await
            .unwrap();
        transport.started.notified().await;
        peer_write
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":\"ping\",\"method\":\"ping\",\"params\":{}}\n")
            .await
            .unwrap();
        let ping: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(ping, json!({"jsonrpc":"2.0","id":"ping","result":{}}));
        peer_write.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{\"requestId\":7}}\n").await.unwrap();
        let response: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(response, case.response);
        peer_write.shutdown().await.unwrap();
    };
    let (server, ()) = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        tokio::join!(server, peer)
    })
    .await
    .unwrap();
    server.unwrap();
    assert_eq!(
        transport.dropped.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
}

#[tokio::test]
async fn stdio_publishes_implemented_tool_metadata_and_preserves_request_ids() {
    let requests = Mutex::new(0);
    let client = Client::with_transport(
        "",
        Fixture {
            body: Value::Null,
            requests: &requests,
        },
    );
    let messages = [
        initialize("session".into()),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":"list","method":"tools/list","params":{}}),
    ];
    let input = messages
        .iter()
        .map(|message| message.to_string() + "\n")
        .collect::<String>();
    let mut output = vec![];
    serve(&client, input.as_bytes(), &mut output).await.unwrap();
    let responses = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(responses[1]["id"], "list");
    assert_eq!(
        responses[1]["result"]["tools"],
        json!([
            pixiv_mcp::illust_detail_tool(),
            pixiv_mcp::search_illust_tool(),
            pixiv_mcp::trending_tags_illust_tool(),
            pixiv_mcp::illust_ranking_tool(),
            pixiv_mcp::novel_detail_tool(),
            pixiv_mcp::search_novel_tool(),
            pixiv_mcp::user_detail_tool(),
            pixiv_mcp::search_user_tool(),
            pixiv_mcp::mutation_tool(pixiv_mcp::MutationAction::AddBookmark),
            pixiv_mcp::mutation_tool(pixiv_mcp::MutationAction::RemoveBookmark),
            pixiv_mcp::mutation_tool(pixiv_mcp::MutationAction::AddNovelBookmark),
            pixiv_mcp::mutation_tool(pixiv_mcp::MutationAction::RemoveNovelBookmark),
            pixiv_mcp::mutation_tool(pixiv_mcp::MutationAction::FollowUser),
            pixiv_mcp::mutation_tool(pixiv_mcp::MutationAction::UnfollowUser),
            pixiv_mcp::novel_series_tool(),
            pixiv_mcp::novel_content_tool(),
            pixiv_mcp::illust_series_tool(),
            pixiv_mcp::illust_related_tool(),
            pixiv_mcp::illust_recommended_tool(),
            pixiv_mcp::recommended_tool(),
            pixiv_mcp::user_artworks_tool(),
            pixiv_mcp::user_novels_tool(),
            pixiv_mcp::user_relationship_tool(pixiv_mcp::UserRelationship::Following),
            pixiv_mcp::user_relationship_tool(pixiv_mcp::UserRelationship::Followers),
            pixiv_mcp::user_relationship_tool(pixiv_mcp::UserRelationship::Related),
            pixiv_mcp::user_relationship_tool(pixiv_mcp::UserRelationship::Blocked),
            pixiv_mcp::bookmark_list_tool(pixiv_mcp::BookmarkList::Artwork),
            pixiv_mcp::bookmark_list_tool(pixiv_mcp::BookmarkList::Novel),
            pixiv_mcp::bookmark_list_tool(pixiv_mcp::BookmarkList::All),
        ])
    );
}
