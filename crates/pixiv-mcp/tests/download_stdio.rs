#[path = "support/download_direct.rs"]
mod support;
use pixiv_app::lifecycle::Context;
use pixiv_mcp::download::{DownloadExecutor, DownloadFuture};
use pixiv_sdk::Client;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use support::{
    FixtureSaveClient, FixtureTransport, ManualCancelTransport, ManualFixtureSaveClient,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn initialize() -> String {
    format!(
        "{}\n{}\n",
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"})
    )
}
fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/download_direct.json")).unwrap()
}
fn normalize(value: &mut Value, root: &str) {
    match value {
        Value::String(text) => *text = text.replace(root, "<ROOT>"),
        Value::Array(values) => values.iter_mut().for_each(|v| normalize(v, root)),
        Value::Object(values) => values.values_mut().for_each(|v| normalize(v, root)),
        _ => {}
    }
}
#[tokio::test]
async fn resource_enabled_stdio_routes_all_frozen_download_requests_and_preserves_read_only_api() {
    for row in fixture()["cases"].as_array().unwrap() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().to_string_lossy().into_owned();
        let executor = move |context: Context, input| -> DownloadFuture {
            let path = path.clone();
            Box::pin(async move {
                let client = FixtureSaveClient(Arc::new(Client::with_transport(
                    "fixture-access",
                    FixtureTransport {
                        context: context.clone(),
                        requests: Arc::new(Mutex::new(Vec::new())),
                    },
                )));
                pixiv_mcp::download::download(&context, &client, &path, input).await
            })
        };
        let client = Client::with_transport(
            "fixture-access",
            FixtureTransport {
                context: Context::new(),
                requests: Arc::new(Mutex::new(Vec::new())),
            },
        );
        let input = format!(
            "{}{}\n",
            initialize(),
            json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"download","arguments":row["arguments"]}})
        );
        let mut output = Vec::new();
        pixiv_mcp::stdio::serve_with_download(
            &client,
            &executor as &DownloadExecutor,
            input.as_bytes(),
            &mut output,
        )
        .await
        .unwrap();
        let mut actual: Value = std::str::from_utf8(&output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .find(|value| value["id"] == 7)
            .unwrap();
        normalize(&mut actual, root.path().to_str().unwrap());
        assert_eq!(actual, row["response"], "{}", row["name"]);
    }
}
#[tokio::test]
async fn cancelled_download_notification_preserves_published_prefix_and_normal_rpc_result() {
    let data = fixture();
    let row = data["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "partial-cancel")
        .unwrap();
    let root = tempfile::tempdir().unwrap();
    let path = root.path().to_string_lossy().into_owned();
    let entered = Arc::new(tokio::sync::Notify::new());
    let readiness = entered.clone();
    let executor = move |context: Context, input| -> DownloadFuture {
        let path = path.clone();
        let entered = entered.clone();
        Box::pin(async move {
            let client = ManualFixtureSaveClient(Arc::new(Client::with_transport(
                "fixture-access",
                ManualCancelTransport {
                    inner: FixtureTransport {
                        context: context.clone(),
                        requests: Arc::new(Mutex::new(Vec::new())),
                    },
                    started: entered,
                },
            )));
            pixiv_mcp::download::download(&context, &client, &path, input).await
        })
    };
    let (mut peer, server) = tokio::io::duplex(8192);
    let (read, mut write) = tokio::io::split(server);
    let client = Client::with_transport(
        "fixture-access",
        FixtureTransport {
            context: Context::new(),
            requests: Arc::new(Mutex::new(Vec::new())),
        },
    );
    let run = pixiv_mcp::stdio::serve_with_download(
        &client,
        &executor as &DownloadExecutor,
        read,
        &mut write,
    );
    let exchange = async {
        peer.write_all(initialize().as_bytes()).await.unwrap();
        peer.write_all(format!("{}\n",json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"download","arguments":row["arguments"]}})).as_bytes()).await.unwrap();
        readiness.notified().await;
        peer.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{\"requestId\":7}}\n").await.unwrap();
        let mut peer = BufReader::new(peer);
        let mut actual = loop {
            let mut line = String::new();
            assert_ne!(peer.read_line(&mut line).await.unwrap(), 0);
            let value: Value = serde_json::from_str(&line).unwrap();
            if value["id"] == 7 {
                break value;
            }
        };
        normalize(&mut actual, root.path().to_str().unwrap());
        assert_eq!(actual, row["response"]);
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    };
    let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(run, exchange)
    })
    .await
    .unwrap();
    result.unwrap();
}
