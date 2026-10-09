#[path = "support/interrupt_shutdown.rs"]
mod support;
use pixiv_app::{
    config::Store,
    database::{Database, PixivAccount},
    execution::Execution,
    lifecycle::Context,
};
use pixiv_mcp::download::{DownloadExecutor, DownloadFuture, SaveClientFactory};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use support::{State, SyntheticSaveClient, SyntheticTransport};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/interrupt_shutdown.json")).unwrap()
}
#[tokio::test]
async fn idle_root_shutdown_matches_go() {
    check_row(&fixture()["cases"][0]).await;
}
#[tokio::test]
async fn completed_root_shutdown_disposes_request_context_after_live_download() {
    check_row(&fixture()["cases"][1]).await;
}
#[tokio::test]
async fn closing_root_still_accepts_request_cancellation_notification() {
    check_row(&fixture()["cases"][2]).await;
}
#[tokio::test]
async fn normal_eof_finishes_with_uncanceled_root_context() {
    check_row(&fixture()["cases"][3]).await;
}
async fn check_row(row: &Value) {
    let eof = row["name"] == "normal-eof";
    let active = row["name"] != "idle" && !eof;
    let notification = row["name"] == "active-notification-cancelled";
    let root = tempfile::tempdir().unwrap();
    let dest = root.path().join("downloads");
    std::fs::create_dir(&dest).unwrap();
    let config = root.path().join("config.toml");
    std::fs::write(&config, "").unwrap();
    let mut database = Database::open(root.path()).unwrap();
    database
        .save_pixiv_credential(&PixivAccount::new(42, "fixture", b"fixture-refresh"))
        .unwrap();
    let state = Arc::new(State::default());
    let factory_state = state.clone();
    let execution = Arc::new(Execution::new(
        Store::new(config),
        Arc::new(Mutex::new(database)),
        move |_| Ok(SyntheticTransport(factory_state.clone())),
    ));
    let operation = execution.clone();
    let observed = state.clone();
    let path = dest.to_string_lossy().into_owned();
    let executor = move |context: Context, input| -> DownloadFuture {
        *observed.request.lock().unwrap() = Some(context.clone());
        let execution = operation.clone();
        let observed = observed.clone();
        let path = path.clone();
        Box::pin(async move {
            let factory: Arc<SaveClientFactory<SyntheticTransport>> =
                Arc::new(|client| Arc::new(SyntheticSaveClient(client)));
            let result = pixiv_mcp::download::saved_download(
                &execution, &context, &path, input, None, factory,
            )
            .await;
            *observed.result.lock().unwrap() = Some(serde_json::to_value(&result).unwrap());
            observed.events.lock().unwrap().push("tool_completed");
            result
        })
    };
    let context = Context::new();
    let (peer, server) = tokio::io::duplex(16384);
    let (read, mut output) = tokio::io::split(server);
    let (peer_read, mut peer_write) = tokio::io::split(peer);
    let mut run = Box::pin(pixiv_mcp::stdio::serve_saved_with_download_context(
        &execution,
        None,
        &executor as &DownloadExecutor,
        &context,
        read,
        &mut output,
    ));
    let exchange = async {
        peer_write.write_all(format!("{}\n{}\n",json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"fixture","version":"0"}}}),json!({"jsonrpc":"2.0","method":"notifications/initialized"})).as_bytes()).await.unwrap();
        let mut reader = BufReader::new(peer_read);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        let mut initialization: Value = serde_json::from_str(&line).unwrap();
        // The Go harness registers only download on a fixture server; production identity is covered separately.
        initialization["result"]
            .as_object_mut()
            .unwrap()
            .remove("instructions");
        initialization["result"]["serverInfo"] = json!({"name":"fixture","version":"0"});
        assert_eq!(json!([initialization]), row["stdio"]);
        if active {
            peer_write.write_all(format!("{}\n",json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"download","arguments":{"srcs":["https://i.pximg.net/prefix.png","https://i.pximg.net/pending.png"]}}})).as_bytes()).await.unwrap();
            state.started.notified().await;
            let waiting = row["events"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["name"] == "body_waiting")
                .unwrap();
            assert_eq!(
                support::counts(&dest),
                (
                    waiting["published"].as_u64().unwrap() as usize,
                    waiting["temporary"].as_u64().unwrap() as usize
                )
            );
            assert!(
                state
                    .request
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .error()
                    .is_none()
            );
        }
        if eof {
            peer_write.shutdown().await.unwrap();
        } else {
            context.cancel();
        }
        if active {
            tokio::time::sleep(Duration::from_millis(50)).await;
            assert!(
                state
                    .request
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .error()
                    .is_none()
            );
            assert!(state.result.lock().unwrap().is_none());
            assert_eq!(support::counts(&dest), (1, 1));
            assert_eq!(*state.events.lock().unwrap(), vec!["body_waiting"]);
            peer_write
                .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"ping\"}\n")
                .await
                .unwrap();
            if notification {
                peer_write.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{\"requestId\":7}}\n").await.unwrap();
                tokio::time::sleep(Duration::from_millis(50)).await;
                assert!(
                    state.result.lock().unwrap().is_some(),
                    "closing session must still process notifications/cancelled"
                );
            } else {
                state.release.notify_one();
            }
        }
        reader
    };
    let (result, mut reader) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(&mut run, exchange)
    })
    .await
    .unwrap();
    if eof {
        result.unwrap();
        assert!(context.error().is_none());
    } else {
        let error = result.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
        assert_eq!(error.to_string(), "context canceled");
    }
    assert_eq!(support::files(&dest), row["files"]);
    if active {
        assert_eq!(
            *state.events.lock().unwrap(),
            vec!["body_waiting", "body_closed", "tool_completed"]
        );
        let mut actual = state.result.lock().unwrap().clone().unwrap();
        support::normalize(&mut actual, dest.to_str().unwrap());
        let wanted = row["events"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == "tool_result")
            .unwrap();
        assert_eq!(actual, wanted["result"]);
        assert_eq!(
            state
                .request
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .error()
                .map(|error| error.to_string())
                .unwrap_or_default(),
            row["post_run_request_error"].as_str().unwrap()
        );
        let lease = tokio::time::timeout(
            Duration::from_secs(1),
            execution.open_client(&Context::new(), 0, None),
        )
        .await
        .unwrap()
        .unwrap();
        lease.close().unwrap();
    }
    drop(run);
    drop(output);
    let mut rest = String::new();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), reader.read_line(&mut rest))
            .await
            .unwrap()
            .unwrap(),
        0
    );
}
