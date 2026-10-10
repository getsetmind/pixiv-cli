use serde_json::{Value, json};

fn frozen() -> Value {
    serde_json::from_str(include_str!("fixtures/fanbox-read-tools.json")).unwrap()
}

#[test]
fn all_eleven_fanbox_schemas_and_initialize_match_frozen_go() {
    let fixture = frozen();
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 126);
    assert_eq!(
        fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["calls"].as_array().unwrap().len())
            .sum::<usize>(),
        159
    );
    assert_eq!(fixture["stdio"].as_array().unwrap().len(), 3);
    assert_eq!(pixiv_mcp::fanbox::tools(), fixture["tools_list"]);
    assert_eq!(
        pixiv_mcp::fanbox::initialize("2025-06-18"),
        fixture["initialize"]
    );
}

#[tokio::test]
async fn fanbox_schema_validation_preserves_float64_rounding_and_handler_boundary() {
    let fixture = frozen();
    let server = pixiv_mcp::fanbox::Server::new(Default::default(), None);
    for row in fixture["cases"].as_array().unwrap() {
        for call in row["calls"].as_array().unwrap() {
            let expected = call["error"].as_str().unwrap();
            if expected.starts_with("calling \"tools/call\":") {
                let got = server
                    .call(
                        &pixiv_app::lifecycle::Context::background(),
                        call["tool"].as_str().unwrap(),
                        &call["arguments"],
                    )
                    .await;
                assert_eq!(
                    got.unwrap_err(),
                    expected.trim_start_matches("calling \"tools/call\": "),
                    "{}",
                    row["name"]
                );
            }
        }
    }
    assert_eq!(
        server
            .call(
                &pixiv_app::lifecycle::Context::background(),
                "fanbox_post",
                &json!({"post_id":""})
            )
            .await
            .unwrap(),
        json!({"content":[{"type":"text","text":"Error: post_id is required"}],"structuredContent":{"id":"","title":"","published_at":"","creator_id":"","is_restricted":false,"assets":[]},"isError":true})
    );
}

#[path = "support/fanbox_read_tools.rs"]
mod support;
use std::sync::Arc;
use support::{Session, assert_observation};

#[tokio::test]
async fn all_saved_session_fanbox_reads_match_the_sealed_tool_results_requests_and_ownership() {
    let fixture = frozen();
    for row in fixture["cases"].as_array().unwrap() {
        if matches!(
            row["name"].as_str().unwrap(),
            "cancel_reuse" | "independent_accounts"
        ) {
            continue;
        }
        let session = Session::new(row.clone());
        let context = session.harness.context();
        for (index, call) in row["calls"].as_array().unwrap().iter().enumerate() {
            if row["name"] == "selected_account_changed_between_tools" && index == 1 {
                session.select_nine();
            }
            let actual = session.call(&context, call).await;
            assert_observation(
                &actual,
                call,
                &format!("{}_call_{index}", row["name"].as_str().unwrap()),
            );
            session
                .harness
                .observation
                .lock()
                .unwrap()
                .calls
                .push(actual);
        }
        assert_observation(&session.finish(), row, row["name"].as_str().unwrap());
    }
}

#[tokio::test]
async fn selected_fanbox_snapshots_overlap_and_caller_cancellation_drains_before_same_session_reuse()
 {
    let fixture = frozen();
    for name in ["independent_accounts", "cancel_reuse"] {
        let row = fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["name"] == name)
            .unwrap();
        let session = Arc::new(Session::new(row.clone()));
        let root = session.harness.context();
        let first_context = root.child();
        let s = session.clone();
        let c = first_context.clone();
        let call = row["calls"][0].clone();
        let first = tokio::spawn(async move { s.call_client(&c, &call).await });
        session.harness.await_started().await;
        if name == "independent_accounts" {
            session.select_nine();
            let s = session.clone();
            let c = root.clone();
            let call = row["calls"][1].clone();
            let second = tokio::spawn(async move { s.call(&c, &call).await });
            session.harness.await_started().await;
            session.harness.release_first.add_permits(1);
            session.harness.await_completed().await;
            session.harness.release_second.add_permits(1);
            session.harness.await_completed().await;
            let mut calls = vec![first.await.unwrap(), second.await.unwrap()];
            calls.sort_by_key(|call| call["result"].to_string());
            session.harness.observation.lock().unwrap().calls = calls;
        } else {
            first_context.cancel();
            let caller = first.await.unwrap();
            session.harness.await_completed().await;
            let second = session.call(&root, &row["calls"][1]).await;
            session.harness.observation.lock().unwrap().calls = vec![caller, second];
        }
        assert_observation(&session.finish(), row, name);
    }
}

#[tokio::test]
async fn production_fanbox_stdio_preserves_explicit_cancellation_reuse_and_eof_drainage() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let fixture = frozen();
    for row in fixture["stdio"].as_array().unwrap() {
        let session = Arc::new(Session::new(row["report"]["observation"].clone()));
        let root = session.harness.context();
        let (client, server_io) = tokio::io::duplex(2 * 1024 * 1024);
        let (input, mut output) = tokio::io::split(server_io);
        let s = session.clone();
        let c = root.clone();
        let server = tokio::spawn(async move {
            pixiv_mcp::fanbox::stdio::serve(&s.server, Some(&c), input, &mut output).await
        });
        let (client_input, mut client_output) = tokio::io::split(client);
        let mut lines = BufReader::new(client_input).lines();
        let mut received = Vec::new();
        let mut sent = Vec::new();
        for frame in row["sent"].as_array().unwrap() {
            client_output
                .write_all(format!("{frame}\n").as_bytes())
                .await
                .unwrap();
            sent.push(frame.clone());
            if frame["method"] == "notifications/initialized" {
                continue;
            }
            if frame["method"] == "tools/call" && frame["id"] == 2 {
                session.harness.await_started().await;
                continue;
            }
            if frame.get("id").is_some() || frame["method"] == "notifications/cancelled" {
                let line = lines.next_line().await.unwrap().unwrap();
                received.push(serde_json::from_str::<Value>(&line).unwrap());
            }
        }
        client_output.shutdown().await.unwrap();
        while let Some(line) = lines.next_line().await.unwrap() {
            received.push(serde_json::from_str::<Value>(&line).unwrap());
        }
        server.await.unwrap().unwrap();
        let actual = json!({"name":row["name"],"sent":sent,"received":received,"exit_code":0,"stderr":"","report":{"command_error":"","observation":session.finish()}});
        let mut boundary_expected = row.clone();
        let trace = boundary_expected["report"]["observation"]["trace"]
            .as_array_mut()
            .unwrap();
        assert_eq!(
            &trace[..2],
            &[
                json!("command.service"),
                json!("command.run_stdio/proxy=nil")
            ]
        );
        trace.drain(..2);
        assert_observation(
            &actual,
            &boundary_expected,
            &format!("stdio_{}", row["name"].as_str().unwrap()),
        );
    }
}

#[tokio::test]
async fn fatal_fanbox_stdio_io_drains_owned_requests_and_preserves_go_read_write_cancellation_distinction()
 {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/fanbox-stdio-failures.json")).unwrap();
    for row in fixture["cases"].as_array().unwrap() {
        let session = Arc::new(Session::new(row["observation"].clone()));
        let root = session.harness.context();
        let read_failure = Arc::new(AtomicBool::new(false));
        let write_failure = Arc::new(AtomicBool::new(false));
        let drops = Arc::new(AtomicUsize::new(0));
        let (client, server_io) = tokio::io::duplex(64 * 1024);
        let (input, output) = tokio::io::split(server_io);
        let input = support::FailingInput {
            inner: input,
            fail: read_failure.clone(),
            drops: drops.clone(),
        };
        let mut output = support::FailingOutput {
            inner: output,
            fail: write_failure.clone(),
            harness: session.harness.clone(),
        };
        let s = session.clone();
        let mut task = tokio::spawn(async move {
            pixiv_mcp::fanbox::stdio::serve(&s.server, Some(&root), input, &mut output).await
        });
        let (client_input, mut client_output) = tokio::io::split(client);
        let mut lines = BufReader::new(client_input).lines();
        client_output.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{},\"clientInfo\":{\"name\":\"owned-failure\",\"version\":\"1\"}}}\n").await.unwrap();
        let frame: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        client_output.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"fanbox_home\",\"arguments\":{}}}\n").await.unwrap();
        session.harness.await_started().await;
        match row["name"].as_str().unwrap() {
            "parse_failure" => client_output.write_all(b"{\n").await.unwrap(),
            "read_failure" => {
                read_failure.store(true, Ordering::SeqCst);
                client_output.write_all(b"\n").await.unwrap();
            }
            "write_failure" => {
                write_failure.store(true, Ordering::SeqCst);
                client_output
                    .write_all(
                        b"{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"ping\",\"params\":{}}\n",
                    )
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let result = match tokio::time::timeout(std::time::Duration::from_secs(5), &mut task).await
        {
            Ok(result) => result.unwrap(),
            Err(_) => {
                task.abort();
                let _ = task.await;
                panic!("owned fatal stdio schedule must drain");
            }
        };
        let error = result.unwrap_err();
        if row["name"] == "parse_failure" {
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
            assert_eq!(
                row["server_error"],
                "unmarshaling jsonrpc message: unexpected end of JSON input"
            );
        } else {
            assert_eq!(error.to_string(), row["server_error"].as_str().unwrap());
        }
        assert_eq!(
            drops.load(Ordering::SeqCst),
            row["connection_closes"].as_u64().unwrap() as usize
        );
        let mut frames = vec![frame];
        while let Some(line) = lines.next_line().await.unwrap() {
            frames.push(serde_json::from_str::<Value>(&line).unwrap());
        }
        assert_eq!(json!(frames), row["frames"]);
        assert_observation(
            &session.finish(),
            &row["observation"],
            &format!("fatal_{}", row["name"].as_str().unwrap()),
        );
    }
}

#[tokio::test]
async fn dropping_an_owned_fanbox_tool_future_releases_its_opened_lease_once() {
    let fixture = frozen();
    let expected = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "cancel_reuse")
        .unwrap();
    let session = Arc::new(Session::new(expected.clone()));
    let root = session.harness.context().child();
    let s = session.clone();
    let call = expected["calls"][0].clone();
    let task = tokio::spawn(async move { s.call(&root, &call).await });
    session.harness.await_started().await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let observed = session.harness.observation.lock().unwrap();
    assert_eq!(observed.lease_opens, 1);
    assert_eq!(observed.lease_closes, 1);
    assert_eq!(observed.live, 0);
}
