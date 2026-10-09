#[path = "support/download_static.rs"]
mod support;
use pixiv_app::{
    config::Store,
    database::{Database, PixivAccount},
    execution::Execution,
    lifecycle::Context,
};
use pixiv_mcp::download::{
    DownloadDefaults, DownloadExecutor, DownloadFuture, SaveClientFactory, decode_download,
    download_tool, saved_download_with_defaults,
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, atomic::Ordering};
use support::{FixtureSaveClient, FixtureTransport, Observed, files, normalize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/download_static.json")).unwrap()
}
fn initialize() -> String {
    format!(
        "{}\n{}\n",
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"fixture","version":"0"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"})
    )
}
struct Setup {
    directory: tempfile::TempDir,
    execution: Arc<Execution<FixtureTransport>>,
    defaults: DownloadDefaults,
    observed: Arc<Observed>,
    database: Arc<Mutex<Database>>,
}
fn setup(row: &Value, metadata: Value, pending: Option<Arc<tokio::sync::Notify>>) -> Setup {
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("downloads");
    std::fs::create_dir(&destination).unwrap();
    let config = directory.path().join("config.toml");
    std::fs::write(&config,format!("[download]\npath={}\nfilename_template={}\ndirectory_template={}\n[account_pool]\nenabled=true\nstrategy='round_robin'\n[pixiv.auth]\ndefault_user_id=43\n",serde_json::to_string(destination.to_str().unwrap()).unwrap(),row["filename_template"],row["directory_template"])).unwrap();
    let store = Store::new(config);
    let runtime = store.current().unwrap().runtime().unwrap();
    let defaults = DownloadDefaults::from(&runtime);
    assert_eq!(
        defaults.filename_template,
        row["filename_template"].as_str().unwrap()
    );
    assert_eq!(
        defaults.directory_template,
        row["directory_template"].as_str().unwrap()
    );
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
    let database = Arc::new(Mutex::new(database));
    let observed = Arc::new(Observed::default());
    let observer = observed.clone();
    let execution = Arc::new(Execution::new(store, database.clone(), move |_| {
        Ok(FixtureTransport {
            observed: observer.clone(),
            metadata: metadata.clone(),
            pending: pending.clone(),
        })
    }));
    Setup {
        directory,
        execution,
        defaults,
        observed,
        database,
    }
}
fn executor(
    execution: Arc<Execution<FixtureTransport>>,
    defaults: DownloadDefaults,
) -> impl Fn(Context, pixiv_mcp::download::DownloadInput) -> DownloadFuture + Send + Sync {
    move |context, input| {
        let execution = execution.clone();
        let defaults = defaults.clone();
        Box::pin(async move {
            let factory: Arc<SaveClientFactory<FixtureTransport>> =
                Arc::new(|client| Arc::new(FixtureSaveClient(client)));
            saved_download_with_defaults(&execution, &context, &defaults, input, None, factory)
                .await
        })
    }
}
#[test]
fn static_download_schema_and_wire_validation_match_frozen_registered_go() {
    let data = fixture();
    assert_eq!(download_tool(), data["tool"]);
    for row in data["cases"].as_array().unwrap() {
        if let Some(error) = row["response"].get("error") {
            assert_eq!(
                decode_download(Some(&row["arguments"])).unwrap_err(),
                error["message"].as_str().unwrap()
            );
        } else {
            decode_download(Some(&row["arguments"])).unwrap();
        }
    }
}
#[tokio::test]
async fn static_download_configured_saved_stdio_reports_requests_and_files_match_go() {
    let data = fixture();
    for row in data["cases"].as_array().unwrap() {
        let name = row["name"].as_str().unwrap();
        let Setup {
            directory: _directory,
            execution,
            defaults,
            observed,
            database,
        } = setup(row, data["metadata"].clone(), None);
        let root = defaults.download_path.clone();
        let execute = executor(execution.clone(), defaults);
        let input = format!(
            "{}{}\n",
            initialize(),
            json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"download","arguments":row["arguments"]}})
        );
        let mut output = Vec::new();
        pixiv_mcp::stdio::serve_saved_with_download(
            &execution,
            None,
            &execute as &DownloadExecutor,
            input.as_bytes(),
            &mut output,
        )
        .await
        .unwrap();
        let mut actual = std::str::from_utf8(&output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .find(|value| value["id"] == 7)
            .unwrap();
        normalize(&mut actual, &root);
        assert_eq!(actual, row["response"], "{name}");
        let mut requests = observed.requests.lock().unwrap().clone();
        if name == "one-artwork-business-failure" {
            requests.sort_by(|a, b| a["url"].as_str().cmp(&b["url"].as_str()));
        }
        assert_eq!(json!(requests), row["requests"], "request {name}");
        assert_eq!(
            json!(*observed.opens.lock().unwrap()),
            row["opens"],
            "opens {name}"
        );
        assert_eq!(
            observed.closes.load(Ordering::SeqCst),
            row["closes"].as_u64().unwrap() as usize,
            "closes {name}"
        );
        let (actual_files, actual_directories) = files(std::path::Path::new(&root));
        assert_eq!(actual_files, row["files"], "files {name}");
        assert_eq!(actual_directories, row["directories"], "directories {name}");
        assert_eq!(
            database
                .lock()
                .unwrap()
                .get_pixiv(42)
                .unwrap()
                .credential_revision,
            1,
            "pool account mutated {name}"
        );
        if !observed.opens.lock().unwrap().is_empty() {
            assert_eq!(
                database
                    .lock()
                    .unwrap()
                    .get_pixiv(43)
                    .unwrap()
                    .credential_revision,
                2,
                "default rotation {name}"
            );
        }
    }
}
#[tokio::test]
async fn static_stdio_notification_cancels_page_two_body_and_retains_published_prefix() {
    let data = fixture();
    let row = data["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "multipage-default")
        .unwrap();
    let ready = Arc::new(tokio::sync::Notify::new());
    let Setup {
        directory: _directory,
        execution,
        defaults,
        observed,
        database: _database,
    } = setup(row, data["metadata"].clone(), Some(ready.clone()));
    let root = defaults.download_path.clone();
    let execute = executor(execution.clone(), defaults);
    let (mut peer, server) = tokio::io::duplex(16384);
    let (read, mut write) = tokio::io::split(server);
    let run = pixiv_mcp::stdio::serve_saved_with_download(
        &execution,
        None,
        &execute as &DownloadExecutor,
        read,
        &mut write,
    );
    let exchange = async {
        peer.write_all(initialize().as_bytes()).await.unwrap();
        peer.write_all(format!("{}\n",json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"download","arguments":{"src":"44"}}})).as_bytes()).await.unwrap();
        ready.notified().await;
        let (pending_files, _) = files(std::path::Path::new(&root));
        assert_eq!(
            pending_files.as_array().unwrap().len(),
            2,
            "one published file plus one live atomic temp"
        );
        peer.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{\"requestId\":7,\"reason\":\"fixture\"}}\n").await.unwrap();
        let mut peer = BufReader::new(peer);
        let mut actual = loop {
            let mut line = String::new();
            assert_ne!(peer.read_line(&mut line).await.unwrap(), 0);
            let value: Value = serde_json::from_str(&line).unwrap();
            if value["id"] == 7 {
                break value;
            }
        };
        normalize(&mut actual, &root);
        assert_eq!(actual, data["stdio_cancellation"]["response"]);
    };
    let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        tokio::join!(run, exchange)
    })
    .await
    .unwrap();
    result.unwrap();
    let (actual_files, _) = files(std::path::Path::new(&root));
    assert_eq!(actual_files, data["stdio_cancellation"]["files"]);
    assert_eq!(observed.closes.load(Ordering::SeqCst), 1);
}
