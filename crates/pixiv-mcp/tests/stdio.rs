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
            pixiv_mcp::search_illust_tool()
        ])
    );
}
