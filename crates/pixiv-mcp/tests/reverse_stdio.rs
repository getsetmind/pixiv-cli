#[path = "support/reverse_stdio.rs"]
mod support;

use pixiv_app::{lifecycle::Context, reverse_search::Provider};
use pixiv_mcp::{reverse_search::ReverseExecutor, stdio::DownloadExecutors};
use serde_json::{Value, json};
use std::sync::atomic::Ordering;

#[tokio::test]
async fn saved_stdio_preserves_all_frozen_scalar_reverse_calls_without_sdk_or_searcher_close() {
    let fixture = support::fixture();
    for row in fixture["cases"].as_array().unwrap() {
        let searcher = support::FixtureSearcher::from_row(row);
        let reverse = ReverseExecutor::new(
            searcher.clone(),
            Provider::from(row["startup_provider"].as_str().unwrap()),
            row["startup_pixiv_only"].as_bool().unwrap(),
        );
        let saved = support::EmptyExecution::new();
        let input = support::frames(&[
            fixture["cancellation_and_reuse"]["sent"][0].clone(),
            fixture["cancellation_and_reuse"]["sent"][1].clone(),
            row["wire_request"].clone(),
        ]);
        let context = Context::new();
        let (peer, server) = tokio::io::duplex(32768);
        let (read, mut write) = tokio::io::split(server);
        let run = pixiv_mcp::stdio::serve_saved_with_reverse_and_downloads_context(
            &saved.execution,
            Some(fixture["account_https_proxy_override"].as_str().unwrap()),
            DownloadExecutors::default(),
            (row["unconfigured"] != true).then_some(&reverse),
            &context,
            read,
            &mut write,
        );
        let exchange = async {
            let mut peer = BufReader::new(peer);
            peer.get_mut().write_all(input.as_bytes()).await.unwrap();
            vec![receive(&mut peer).await, receive(&mut peer).await]
        };
        let (result, actual) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(run, exchange)
        })
        .await
        .unwrap();
        result.unwrap();
        assert_eq!(actual.len(), 2, "{}", row["name"]);
        assert_eq!(actual[0], fixture["initialization"], "{}", row["name"]);
        assert_eq!(actual[1], row["wire_response"], "{}", row["name"]);
        assert_eq!(
            *searcher.requests.lock().unwrap(),
            *row["searcher_requests"].as_array().unwrap(),
            "{}",
            row["name"]
        );
        assert_eq!(searcher.closed.load(Ordering::SeqCst), 0);
        assert_eq!(saved.execute_calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn stdio_lists_reverse_search_once_and_keeps_existing_tools() {
    let fixture = support::fixture();
    let saved = support::EmptyExecution::new();
    let input = support::frames(&[
        fixture["cancellation_and_reuse"]["sent"][0].clone(),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
    ]);
    let mut output = Vec::new();
    pixiv_mcp::stdio::serve_saved_with_reverse_and_downloads_context(
        &saved.execution,
        None,
        DownloadExecutors::default(),
        None,
        &Context::new(),
        input.as_bytes(),
        &mut output,
    )
    .await
    .unwrap();
    let frames = support::parse_frames(&output);
    let tools = frames[1]["result"]["tools"].as_array().unwrap();
    assert_eq!(
        tools
            .iter()
            .filter(|tool| tool["name"] == "reverse_search")
            .collect::<Vec<_>>(),
        vec![&fixture["tool"]]
    );
    for name in [
        "download",
        "download_random_from_recommendation",
        "illust_detail",
        "search_illust",
        "recommended",
    ] {
        assert!(tools.iter().any(|tool| tool["name"] == name), "{name}");
    }
    assert_eq!(saved.execute_calls.load(Ordering::SeqCst), 0);
}

use pixiv_app::reverse_search::{Error, SearchOutcome};
use std::{sync::Arc, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

async fn receive<R: tokio::io::AsyncBufRead + Unpin>(peer: &mut R) -> Value {
    let mut line = String::new();
    assert_ne!(peer.read_line(&mut line).await.unwrap(), 0);
    serde_json::from_str(&line).unwrap()
}

#[tokio::test]
async fn cancellation_notification_awaits_search_cleanup_preserves_results_and_allows_reuse() {
    let fixture = support::fixture();
    let expected = fixture["cancellation_and_reuse"].clone();
    let response = support::standard_response();
    let started = Arc::new(tokio::sync::Notify::new());
    let cleaned = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let readiness = started.clone();
    let finished = cleaned.clone();
    let searcher = support::FixtureSearcher::new(move |context, request| {
        let response = response.clone();
        let started = started.clone();
        let cleaned = cleaned.clone();
        Box::pin(async move {
            let error = if request.source.ends_with("cancel.png") {
                started.notify_one();
                let error = context.cancelled().await;
                tokio::task::yield_now().await;
                cleaned.store(true, Ordering::SeqCst);
                Some(error.into())
            } else {
                None
            };
            SearchOutcome { response, error }
        })
    });
    let reverse = ReverseExecutor::new(searcher.clone(), Provider::All, true);
    let saved = support::EmptyExecution::new();
    let context = Context::new();
    let (peer, server) = tokio::io::duplex(32768);
    let (read, mut write) = tokio::io::split(server);
    let run = pixiv_mcp::stdio::serve_saved_with_reverse_and_downloads_context(
        &saved.execution,
        None,
        DownloadExecutors::default(),
        Some(&reverse),
        &context,
        read,
        &mut write,
    );
    let exchange = async {
        let mut peer = BufReader::new(peer);
        let sent = expected["sent"].as_array().unwrap();
        peer.get_mut()
            .write_all(support::frames(&sent[..3]).as_bytes())
            .await
            .unwrap();
        assert_eq!(receive(&mut peer).await, expected["received"][0]);
        readiness.notified().await;
        peer.get_mut()
            .write_all(support::frames(&sent[3..4]).as_bytes())
            .await
            .unwrap();
        assert_eq!(receive(&mut peer).await, expected["received"][1]);
        assert!(finished.load(Ordering::SeqCst));
        assert_eq!(context.error(), None);
        peer.get_mut()
            .write_all(support::frames(&sent[4..]).as_bytes())
            .await
            .unwrap();
        assert_eq!(receive(&mut peer).await, expected["received"][2]);
    };
    let (result, ()) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(run, exchange)
    })
    .await
    .unwrap();
    result.unwrap();
    assert_eq!(
        *searcher.requests.lock().unwrap(),
        *expected["searcher_requests"].as_array().unwrap()
    );
    assert_eq!(searcher.closed.load(Ordering::SeqCst), 0);
    assert_eq!(saved.execute_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn concurrent_reverse_calls_finish_out_of_order_and_reuse_the_same_searcher() {
    let fixture = support::fixture();
    let expected = fixture["concurrent_and_reuse"].clone();
    let response = support::standard_response();
    let release = Arc::new(tokio::sync::Notify::new());
    let gate = release.clone();
    let searcher = support::FixtureSearcher::new(move |_, request| {
        let mut response = response.clone();
        let gate = gate.clone();
        Box::pin(async move {
            if request.provider == Provider::Ascii2dColor {
                gate.notified().await;
            }
            response.input.sha256 = format!("synthetic-{}", request.provider.as_str());
            SearchOutcome {
                response,
                error: None,
            }
        })
    });
    let reverse = ReverseExecutor::new(searcher.clone(), Provider::SauceNao, false);
    let saved = support::EmptyExecution::new();
    let context = Context::new();
    let (peer, server) = tokio::io::duplex(32768);
    let (read, mut write) = tokio::io::split(server);
    let run = pixiv_mcp::stdio::serve_saved_with_reverse_and_downloads_context(
        &saved.execution,
        None,
        DownloadExecutors::default(),
        Some(&reverse),
        &context,
        read,
        &mut write,
    );
    let exchange = async {
        let mut peer = BufReader::new(peer);
        let sent = expected["sent"].as_array().unwrap();
        peer.get_mut()
            .write_all(support::frames(&sent[..4]).as_bytes())
            .await
            .unwrap();
        assert_eq!(receive(&mut peer).await, expected["received"][0]);
        assert_eq!(receive(&mut peer).await, expected["received"][1]);
        release.notify_one();
        assert_eq!(receive(&mut peer).await, expected["received"][2]);
        peer.get_mut()
            .write_all(support::frames(&sent[4..]).as_bytes())
            .await
            .unwrap();
        assert_eq!(receive(&mut peer).await, expected["received"][3]);
    };
    let (result, ()) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(run, exchange)
    })
    .await
    .unwrap();
    result.unwrap();
    assert_eq!(
        *searcher.requests.lock().unwrap(),
        *expected["searcher_requests"].as_array().unwrap()
    );
    assert_eq!(searcher.closed.load(Ordering::SeqCst), 0);
    assert_eq!(saved.execute_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn parent_context_cancellation_propagates_metadata_and_waits_for_reverse_cleanup() {
    let fixture = support::fixture();
    let started = Arc::new(tokio::sync::Notify::new());
    let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let readiness = started.clone();
    let cleaned = finished.clone();
    let key = pixiv_sdk::context::ContextKey::new("reverse-parent-metadata");
    let search_key = key.clone();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let searcher = support::FixtureSearcher::new(move |context, _| {
        let started = started.clone();
        let finished = finished.clone();
        let key = search_key.clone();
        Box::pin(async move {
            assert_eq!(context.deadline(), Some(deadline));
            assert_eq!(
                *context.value(&key).unwrap().downcast::<String>().unwrap(),
                "inherited"
            );
            started.notify_one();
            let error = context.cancelled().await;
            tokio::task::yield_now().await;
            finished.store(true, Ordering::SeqCst);
            SearchOutcome::failure(Error::from(error))
        })
    });
    let reverse = ReverseExecutor::new(searcher.clone(), Provider::All, true);
    let saved = support::EmptyExecution::new();
    let context =
        Context::with_deadline(deadline).with_value(key, Arc::new(String::from("inherited")));
    let (mut peer, server) = tokio::io::duplex(32768);
    let (read, mut write) = tokio::io::split(server);
    let run = pixiv_mcp::stdio::serve_saved_with_reverse_and_downloads_context(
        &saved.execution,
        None,
        DownloadExecutors::default(),
        Some(&reverse),
        &context,
        read,
        &mut write,
    );
    let exchange = async {
        peer.write_all(
            support::frames(
                &fixture["cancellation_and_reuse"]["sent"]
                    .as_array()
                    .unwrap()[..3],
            )
            .as_bytes(),
        )
        .await
        .unwrap();
        readiness.notified().await;
        context.cancel();
        peer.shutdown().await.unwrap();
    };
    let (result, ()) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(run, exchange)
    })
    .await
    .unwrap();
    assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::Interrupted);
    assert!(cleaned.load(Ordering::SeqCst));
    assert_eq!(searcher.closed.load(Ordering::SeqCst), 0);
    assert_eq!(saved.execute_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn sequential_stdio_frames_match_the_owned_frozen_go_child_and_close_on_eof() {
    let fixture = support::fixture();
    let response = support::standard_response();
    let searcher = support::FixtureSearcher::new(move |_, request| {
        let mut response = response.clone();
        Box::pin(async move {
            response.input.sha256 = format!("synthetic-{}", request.provider.as_str());
            SearchOutcome {
                response,
                error: None,
            }
        })
    });
    let reverse = ReverseExecutor::new(searcher.clone(), Provider::Ascii2dBovw, true);
    let saved = support::EmptyExecution::new();
    let context = Context::new();
    let (peer, server) = tokio::io::duplex(32768);
    let (read, mut write) = tokio::io::split(server);
    let run = pixiv_mcp::stdio::serve_saved_with_reverse_and_downloads_context(
        &saved.execution,
        None,
        DownloadExecutors::default(),
        Some(&reverse),
        &context,
        read,
        &mut write,
    );
    let exchange = async {
        let mut peer = BufReader::new(peer);
        let requests = [
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"migration-reverse-search","version":"0"}}}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"reverse_search","arguments":{"source":"/synthetic/private-source-secret.png"}}}),
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"reverse_search","arguments":{"source":"/synthetic/private-source-secret.png","provider":"ascii2d-color"}}}),
            json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"reverse_search","arguments":{"source":"/synthetic/private-source-secret.png","pixiv_only":false}}}),
        ];
        for (index, request) in requests.iter().enumerate() {
            peer.get_mut()
                .write_all(support::frames(std::slice::from_ref(request)).as_bytes())
                .await
                .unwrap();
            let actual = receive(&mut peer).await;
            assert_eq!(actual, fixture["owned_test_child_stdio_responses"][index]);
            let wire = actual.to_string();
            for secret in [
                "private-source-secret",
                "input-key-secret",
                "cookie-secret",
                "request-header-secret",
            ] {
                assert!(!wire.contains(secret));
            }
            if index == 0 {
                peer.get_mut()
                    .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
                    .await
                    .unwrap();
            }
        }
        peer.get_mut().shutdown().await.unwrap();
    };
    let (result, ()) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(run, exchange)
    })
    .await
    .unwrap();
    result.unwrap();
    assert_eq!(searcher.requests.lock().unwrap().len(), 2);
    assert_eq!(searcher.closed.load(Ordering::SeqCst), 0);
    assert_eq!(saved.execute_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn local_source_copy_accepts_stdio_cancellation_before_provider_search_and_allows_reuse() {
    use pixiv_app::reverse_search::{
        Aggregator, AggregatorDependencies, Dependencies, Facade, Loader, SourceLoaderOptions,
    };
    use std::{future::Future, task::Poll};

    let owned = tempfile::tempdir().unwrap();
    let snapshots = owned.path().join("snapshots");
    std::fs::create_dir(&snapshots).unwrap();
    let source = owned.path().join("private-source-secret.png");
    let bytes = Arc::new(vec![0x5a; 1024 * 1024]);
    std::fs::write(&source, bytes.as_slice()).unwrap();
    let provider = support::LocalSnapshotProvider::new(bytes.clone());
    let facade = Arc::new(Facade::new(Dependencies {
        sources: Some(Arc::new(Loader::new(SourceLoaderOptions {
            temp_dir: snapshots.clone(),
            ..Default::default()
        }))),
        payloads: Some(Arc::new(Aggregator::new(AggregatorDependencies {
            sauce_nao: Some(provider.clone()),
            ..Default::default()
        }))),
    }));
    let reverse = ReverseExecutor::new(facade.clone(), Provider::SauceNao, true);
    let saved = support::EmptyExecution::new();
    let context = Context::new();
    let (peer, server) = tokio::io::duplex(32768);
    let (read, mut write) = tokio::io::split(server);
    let mut run = Box::pin(
        pixiv_mcp::stdio::serve_saved_with_reverse_and_downloads_context(
            &saved.execution,
            None,
            DownloadExecutors::default(),
            Some(&reverse),
            &context,
            read,
            &mut write,
        ),
    );
    let mut peer = BufReader::new(peer);
    let fixture = support::fixture();
    peer.get_mut()
        .write_all(
            support::frames(
                &fixture["cancellation_and_reuse"]["sent"]
                    .as_array()
                    .unwrap()[..2],
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut task = std::task::Context::from_waker(futures_util::task::noop_waker_ref());
    assert!(matches!(run.as_mut().poll(&mut task), Poll::Pending));
    assert_eq!(receive(&mut peer).await, fixture["initialization"]);
    let call = |id| json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"reverse_search","arguments":{"source":source.to_str().unwrap()}}});
    peer.get_mut()
        .write_all(support::frames(&[call(2)]).as_bytes())
        .await
        .unwrap();
    assert!(matches!(run.as_mut().poll(&mut task), Poll::Pending));
    assert_eq!(provider.preflights.load(Ordering::SeqCst), 1);
    assert_eq!(
        provider.searches.load(Ordering::SeqCst),
        0,
        "a local source copy must let stdio read cancellation before provider search"
    );
    let partial: Vec<_> = std::fs::read_dir(&snapshots).unwrap().collect();
    assert_eq!(
        partial.len(),
        1,
        "the real loader must have a partial snapshot"
    );
    let copied = partial[0].as_ref().unwrap().metadata().unwrap().len();
    assert!(copied > 0 && copied < bytes.len() as u64);
    peer.get_mut()
        .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{\"requestId\":2}}\n")
        .await
        .unwrap();
    let exchange = async {
        let canceled = receive(&mut peer).await;
        assert_eq!(
            canceled,
            json!({
                "jsonrpc":"2.0","id":2,"result":{
                    "content":[{"type":"text","text":"Error: reverse search failed"}],
                    "structuredContent":{"input":{"kind":"","sha256":""},"providers":[],"results":[],"records":[],"provider_errors":[],"partial":false},
                    "isError":true
                }
            })
        );
        assert_eq!(provider.searches.load(Ordering::SeqCst), 0);
        assert_eq!(std::fs::read_dir(&snapshots).unwrap().count(), 0);
        assert_eq!(std::fs::read(&source).unwrap(), *bytes);
        assert_eq!(context.error(), None);
        peer.get_mut()
            .write_all(support::frames(&[call(3)]).as_bytes())
            .await
            .unwrap();
        let reused = receive(&mut peer).await;
        assert_eq!(reused["id"], 3);
        assert!(reused["result"].get("isError").is_none());
        assert_eq!(
            reused["result"]["structuredContent"]["input"]["kind"],
            "file"
        );
        assert_eq!(provider.preflights.load(Ordering::SeqCst), 2);
        assert_eq!(provider.searches.load(Ordering::SeqCst), 1);
        assert_eq!(std::fs::read_dir(&snapshots).unwrap().count(), 0);
        assert_eq!(std::fs::read(&source).unwrap(), *bytes);
        assert!(!reused.to_string().contains("private-source-secret"));
        peer.get_mut().shutdown().await.unwrap();
    };
    let (result, ()) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(run, exchange)
    })
    .await
    .unwrap();
    result.unwrap();
    assert_eq!(saved.execute_calls.load(Ordering::SeqCst), 0);
    assert_eq!(provider.closes.load(Ordering::SeqCst), 0);
    facade.close().await.unwrap();
    assert_eq!(provider.closes.load(Ordering::SeqCst), 1);
}
