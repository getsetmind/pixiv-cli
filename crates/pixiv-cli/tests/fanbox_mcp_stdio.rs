use pixiv_cli_rs::fanbox_mcp::{Data, McpCommand};
use serde_json::{Value, json};
use std::sync::Arc;
#[path = "../../pixiv-mcp/tests/support/fanbox_read_tools.rs"]
mod support;
use support::{Session, assert_observation};
fn frozen() -> Value {
    serde_json::from_str(include_str!(
        "../../pixiv-mcp/tests/fixtures/fanbox-read-tools.json"
    ))
    .unwrap()
}

#[tokio::test]
async fn actual_fanbox_command_leaf_stdio_matches_all_three_frozen_go_schedules() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let fixture = frozen();
    for row in fixture["stdio"].as_array().unwrap() {
        let session = Arc::new(Session::new(row["report"]["observation"].clone()));
        let root = session.harness.context();
        let (client, server_io) = tokio::io::duplex(2 * 1024 * 1024);
        let (input, mut output) = tokio::io::split(server_io);
        let service_session = session.clone();
        let run_session = session.clone();
        let command = McpCommand::parse(&["mcp".into()]).unwrap();
        let server = tokio::spawn(async move {
            command
                .run(Data {
                    service_factory: move || {
                        service_session.harness.trace("command.service");
                        Ok(Some(service_session.facade.clone()))
                    },
                    run_server: move |_facade, proxy: Option<String>| async move {
                        run_session.harness.trace(format!(
                            "command.run_stdio/proxy={}",
                            proxy.as_deref().unwrap_or("nil")
                        ));
                        pixiv_mcp::fanbox::stdio::serve(
                            &run_session.server,
                            Some(&root),
                            input,
                            &mut output,
                        )
                        .await
                        .map_err(Into::into)
                    },
                })
                .await
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
                received.push(
                    serde_json::from_str::<Value>(&lines.next_line().await.unwrap().unwrap())
                        .unwrap(),
                );
            }
        }
        client_output.shutdown().await.unwrap();
        while let Some(line) = lines.next_line().await.unwrap() {
            received.push(serde_json::from_str::<Value>(&line).unwrap());
        }
        let command_error = server
            .await
            .unwrap()
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();
        let actual = json!({"name":row["name"],"sent":sent,"received":received,"exit_code":0,"stderr":"","report":{"command_error":command_error,"observation":session.finish()}});
        assert_observation(
            &actual,
            row,
            &format!("cli_stdio_{}", row["name"].as_str().unwrap()),
        );
    }
}

#[tokio::test]
async fn fanbox_mcp_proxy_validation_precedes_the_actual_service_factory() {
    use pixiv_cli_rs::CommandError;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let opened = Arc::new(AtomicUsize::new(0));
    let counter = opened.clone();
    let command =
        McpCommand::parse(&["mcp".into(), "--proxy=".into(), "--no-proxy=false".into()]).unwrap();
    let error = command
        .run(Data {
            service_factory: move || {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(None)
            },
            run_server: |_, _| async { Ok(()) },
        })
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "use either --proxy or --no-proxy, not both"
    );
    assert_eq!(opened.load(Ordering::SeqCst), 0);
    let error = McpCommand::default()
        .run(Data {
            service_factory: || Ok(None),
            run_server: |_, _| async { Ok(()) },
        })
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "fanbox is not available: cannot open the local account store"
    );
    let error = McpCommand::default()
        .run(Data {
            service_factory: || Err(CommandError::Message("owned service failure")),
            run_server: |_, _| async { Ok(()) },
        })
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "owned service failure");
}
