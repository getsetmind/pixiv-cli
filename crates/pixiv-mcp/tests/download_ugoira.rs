#[path = "support/download_ugoira.rs"]
mod support;

use pixiv_app::{
    config::Store,
    database::{Database, PixivAccount},
    execution::Execution,
    lifecycle::Context,
};
use pixiv_mcp::download::{
    DownloadDefaults, DownloadExecutor, DownloadFuture, DownloadInput, SaveClientFactory,
    decode_download, download_tool, download_with_defaults, saved_download_with_defaults,
};
use pixiv_sdk::Client;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    sync::{Arc, Mutex, atomic::Ordering},
};
use support::{
    FixtureSaveClient, FixtureTransport, Observed, PendingArchive, decode_hex, files, normalize,
    temporary_counts,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn fixture() -> Arc<Value> {
    Arc::new(serde_json::from_str(include_str!("fixtures/download_ugoira.json")).unwrap())
}
fn initialize() -> String {
    format!(
        "{}\n{}\n",
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"fixture","version":"0"}}}),
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
fn setup(
    row: &Value,
    data: Arc<Value>,
    pending: Option<Arc<tokio::sync::Notify>>,
    rate_metadata: bool,
    rate_archive: bool,
) -> Setup {
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("downloads");
    std::fs::create_dir(&destination).unwrap();
    for file in row["initial_files"].as_array().unwrap() {
        let path = destination.join(file["name"].as_str().unwrap());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, decode_hex(file["hex"].as_str().unwrap())).unwrap();
    }
    let config = directory.path().join("config.toml");
    std::fs::write(&config,format!("[download]\npath={}\nfilename_template={}\ndirectory_template={}\n[account_pool]\nenabled=true\nstrategy='round_robin'\n[pixiv.auth]\ndefault_user_id=43\n",
        serde_json::to_string(destination.to_str().unwrap()).unwrap(),
        row["filename_template"],row["directory_template"])).unwrap();
    let store = Store::new(config);
    let defaults = DownloadDefaults::from(&store.current().unwrap().runtime().unwrap());
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
    let pending = pending.map(|ready| PendingArchive {
        ready,
        root: destination,
    });
    let execution = Arc::new(Execution::new(store, database.clone(), move |_| {
        Ok(FixtureTransport {
            observed: observer.clone(),
            fixture: data.clone(),
            pending: pending.clone(),
            rate_metadata,
            rate_archive,
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
) -> impl Fn(Context, DownloadInput) -> DownloadFuture + Send + Sync {
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
fn account_states(database: &Mutex<Database>) -> Value {
    let database = database.lock().unwrap();
    json!([42, 43].map(|id| {
        let account = database.get_pixiv(id).unwrap();
        json!({"id":id,"revision":account.credential_revision,
            "frozen":account.pool_frozen_until.is_some(),"selected":account.pool_last_selected})
    }))
}
fn compare_disk(root: &str, row: &Value, name: &str) {
    let (actual_files, actual_directories) = files(Path::new(root));
    assert_eq!(actual_files, row["files"], "files {name}");
    assert_eq!(actual_directories, row["directories"], "directories {name}");
}
fn verify_output(value: &Value) {
    let output = &value["structuredContent"];
    let grouped = output["items"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|item| {
            assert!(item.get("quality").is_none());
            assert!(item.get("frames").is_none());
            assert!(item.get("frame_report").is_none());
            item["files"].as_array().unwrap().clone()
        })
        .collect::<Vec<_>>();
    assert_eq!(json!(grouped), output["files"]);
    for failure in output["failures"].as_array().unwrap() {
        for key in ["code", "path", "missing"] {
            assert!(failure.get(key).is_none());
        }
    }
    for content in value["content"].as_array().unwrap() {
        assert_eq!(content["type"], "text");
    }
    let encoded = value.to_string();
    assert!(!encoded.contains("signature=private"));
    assert!(!encoded.contains("fixture-access"));
}

#[test]
fn ugoira_schema_and_validation_preserve_frozen_registered_go_contract() {
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(include_bytes!("fixtures/download_ugoira.json"))
        ),
        "7719be28349638875db9d29cb09eca4e3ae31d67bb8fa880b5bef515bd81a490"
    );
    let data = fixture();
    assert_eq!(data["cases"].as_array().unwrap().len(), 41);
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
async fn ugoira_direct_native_outputs_and_archive_file_metadata_match_frozen_go() {
    let data = fixture();
    for row in data["cases"].as_array().unwrap() {
        let name = row["name"].as_str().unwrap();
        let Ok(input) = decode_download(Some(&row["arguments"])) else {
            continue;
        };
        let Setup {
            directory: _directory,
            execution: _execution,
            defaults,
            observed,
            database: _database,
        } = setup(row, data.clone(), None, false, false);
        let client = FixtureSaveClient(Arc::new(Client::with_transport(
            "fixture-access-43",
            FixtureTransport {
                observed: observed.clone(),
                fixture: data.clone(),
                pending: None,
                rate_metadata: false,
                rate_archive: false,
            },
        )));
        let result = download_with_defaults(&Context::new(), &client, &defaults, input).await;
        let mut actual = serde_json::to_value(result).unwrap();
        normalize(&mut actual, &defaults.download_path);
        assert_eq!(actual, row["response"]["result"], "direct {name}");
        verify_output(&actual);
        let mut requests = observed.requests.lock().unwrap().clone();
        let mut expected = row["requests"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["url"] != "https://oauth.secure.pixiv.net/auth/token")
            .cloned()
            .collect::<Vec<_>>();
        if name == "mixed-static-direct-ugoira-prefix-failure" {
            requests.sort_by(|a, b| a["url"].as_str().cmp(&b["url"].as_str()));
            expected.sort_by(|a, b| a["url"].as_str().cmp(&b["url"].as_str()));
        }
        assert_eq!(json!(requests), json!(expected), "direct requests {name}");
        compare_disk(&defaults.download_path, row, name);
    }
}

#[tokio::test]
async fn ugoira_saved_stdio_native_results_requests_disk_and_account_state_match_go() {
    let data = fixture();
    for row in data["cases"].as_array().unwrap() {
        let name = row["name"].as_str().unwrap();
        let Setup {
            directory: _directory,
            execution,
            defaults,
            observed,
            database,
        } = setup(row, data.clone(), None, false, false);
        let root = defaults.download_path.clone();
        let execute = executor(execution.clone(), defaults);
        let input = format!(
            "{}{}\n",
            initialize(),
            json!({"jsonrpc":"2.0","id":7,
            "method":"tools/call","params":{"name":"download","arguments":row["arguments"]}})
        );
        let mut output = vec![];
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
        assert_eq!(actual, row["response"], "saved stdio {name}");
        if actual.get("result").is_some() {
            verify_output(&actual["result"]);
        }
        let mut requests = observed.requests.lock().unwrap().clone();
        if name == "mixed-static-direct-ugoira-prefix-failure" {
            requests.sort_by(|a, b| a["url"].as_str().cmp(&b["url"].as_str()));
        }
        assert_eq!(json!(requests), row["requests"], "requests {name}");
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
        assert_eq!(
            account_states(&database),
            row["account_states"],
            "account states {name}"
        );
        compare_disk(&root, row, name);
    }
}

#[tokio::test]
async fn ugoira_stdio_cancellation_cleans_owned_archive_and_atomic_temp_keeps_static_prefix() {
    let data = fixture();
    let row = data["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "default-genuine-native-gif")
        .unwrap();
    let ready = Arc::new(tokio::sync::Notify::new());
    let Setup {
        directory: _directory,
        execution,
        defaults,
        observed,
        database: _database,
    } = setup(row, data.clone(), Some(ready.clone()), false, false);
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
        peer.write_all(
            format!(
                "{}\n",
                json!({"jsonrpc":"2.0","id":7,"method":"tools/call",
            "params":{"name":"download","arguments":{"srcs":["42","71"],"ugoira_mode":"apng"}}})
            )
            .as_bytes(),
        )
        .await
        .unwrap();
        ready.notified().await;
        let cancellation = &data["stdio_cancellation"];
        assert_eq!(
            temporary_counts(Path::new(&root)),
            (
                cancellation["published_before_cancel"].as_u64().unwrap() as usize,
                cancellation["archive_temporary_before_cancel"]
                    .as_u64()
                    .unwrap() as usize,
                cancellation["atomic_temporary_before_cancel"]
                    .as_u64()
                    .unwrap() as usize
            )
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
        assert_eq!(actual, cancellation["response"]);
    };
    let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        tokio::join!(run, exchange)
    })
    .await
    .unwrap();
    result.unwrap();
    compare_disk(&root, &data["stdio_cancellation"], "stdio cancellation");
    assert_eq!(
        observed.closes.load(Ordering::SeqCst),
        data["stdio_cancellation"]["closes"].as_u64().unwrap() as usize
    );
}

#[tokio::test]
async fn ugoira_saved_metadata_and_archive_rate_failures_never_replay_default_lease() {
    let data = fixture();
    let row = data["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "archive-original-preferred")
        .unwrap();
    for (metadata, archive) in [(true, false), (false, true)] {
        for prefix in [false, true] {
            let Setup {
                directory: _directory,
                execution,
                defaults,
                observed,
                database,
            } = setup(row, data.clone(), None, metadata, archive);
            let execute = executor(execution, defaults.clone());
            let result = execute(
                Context::new(),
                DownloadInput {
                    srcs: if prefix {
                        vec!["42".into(), "71".into()]
                    } else {
                        vec!["71".into()]
                    },
                    ugoira_mode: "zip".into(),
                    ..Default::default()
                },
            )
            .await;
            assert!(result.is_error);
            assert_eq!(result.structured_content.items.len(), usize::from(prefix));
            assert_eq!(result.structured_content.failures.len(), 1);
            assert_eq!(
                result.structured_content.failures[0].message,
                if metadata {
                    "pixiv:UgoiraMetadata: rate_limited"
                } else {
                    "pixiv:SaveResource: rate_limited"
                }
            );
            assert!(!result.structured_content.text.contains("Download failed:"));
            assert_eq!(*observed.opens.lock().unwrap(), vec![43]);
            assert_eq!(observed.closes.load(Ordering::SeqCst), 1);
            assert_eq!(
                account_states(&database),
                json!([
                {"id":42,"revision":1,"frozen":false,"selected":false},
                {"id":43,"revision":2,"frozen":false,"selected":false}])
            );
            let requests = observed.requests.lock().unwrap();
            assert_eq!(
                requests
                    .iter()
                    .filter(|r| r["url"].as_str().unwrap().contains("/v1/ugoira/metadata"))
                    .count(),
                1
            );
            assert_eq!(
                requests
                    .iter()
                    .filter(|r| r["url"].as_str().unwrap().contains("/original/71.zip"))
                    .count(),
                usize::from(archive)
            );
            assert_eq!(
                temporary_counts(Path::new(&defaults.download_path)),
                (usize::from(prefix), 0, 0)
            );
        }
    }
}
