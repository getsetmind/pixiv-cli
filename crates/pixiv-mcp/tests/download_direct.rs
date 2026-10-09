use pixiv_mcp::download::{decode_download, download_tool};
use serde_json::Value;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/download_direct.json")).unwrap()
}

#[test]
fn download_schema_matches_registered_frozen_go_tool() {
    assert_eq!(download_tool(), fixture()["tool"]);
}

#[test]
fn download_schema_errors_preserve_frozen_go_rpc_messages() {
    for row in fixture()["cases"].as_array().unwrap() {
        if let Some(error) = row["response"].get("error") {
            let actual = decode_download(Some(&row["arguments"])).unwrap_err();
            assert_eq!(
                actual,
                error["message"].as_str().unwrap(),
                "{}",
                row["name"]
            );
        } else {
            decode_download(Some(&row["arguments"])).unwrap();
        }
    }
}

#[path = "support/download_direct.rs"]
mod support;
use pixiv_app::lifecycle::Context;
use pixiv_sdk::Client;
use std::sync::{Arc, Mutex};
use support::{FixtureSaveClient, FixtureTransport};

fn normalize(value: &mut Value, root: &str) {
    match value {
        Value::String(text) => *text = text.replace(root, "<ROOT>"),
        Value::Array(values) => values.iter_mut().for_each(|value| normalize(value, root)),
        Value::Object(values) => values.values_mut().for_each(|value| normalize(value, root)),
        _ => {}
    }
}
#[tokio::test]
async fn download_direct_reports_and_published_files_match_frozen_go() {
    for row in fixture()["cases"].as_array().unwrap() {
        let name = row["name"].as_str().unwrap();
        let input = match decode_download(Some(&row["arguments"])) {
            Ok(input) => input,
            Err(_) => continue,
        };
        let directory = tempfile::tempdir().unwrap();
        let context = Context::new();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let transport = FixtureTransport {
            context: context.clone(),
            requests: requests.clone(),
        };
        let client = FixtureSaveClient(Arc::new(Client::with_transport(
            "fixture-access",
            transport,
        )));
        let result = pixiv_mcp::download::download(
            &context,
            &client,
            directory.path().to_str().unwrap(),
            input,
        )
        .await;
        let mut actual = serde_json::to_value(result).unwrap();
        normalize(&mut actual, directory.path().to_str().unwrap());
        assert_eq!(actual, row["response"]["result"], "{name}");
        assert_eq!(
            serde_json::json!(*requests.lock().unwrap()),
            row["requests"],
            "{name}"
        );
        let mut entries = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        entries.sort();
        let files=entries.iter().map(|path|serde_json::json!({"name":path.file_name().unwrap().to_str().unwrap(),"hex":std::fs::read(path).unwrap().iter().map(|byte|format!("{byte:02x}")).collect::<String>()})).collect::<Vec<_>>();
        assert_eq!(serde_json::json!(files), row["files"], "{name}");
    }
}

#[tokio::test]
async fn saved_download_preserves_partial_reports_and_actual_commit_boundary() {
    use pixiv_app::{
        config::Store,
        database::{Database, PixivAccount},
        execution::Execution,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    for row in fixture()["cases"].as_array().unwrap() {
        let name = row["name"].as_str().unwrap();
        let input = match decode_download(Some(&row["arguments"])) {
            Ok(input) => input,
            Err(_) => continue,
        };
        let directory = tempfile::tempdir().unwrap();
        let destinations = directory.path().join("destinations");
        std::fs::create_dir(&destinations).unwrap();
        let config = directory.path().join("config.toml");
        std::fs::write(&config, "").unwrap();
        let mut database = Database::open(directory.path()).unwrap();
        database
            .save_pixiv_credential(&PixivAccount::new(42, "fixture", b"fixture-refresh"))
            .unwrap();
        let database = Arc::new(Mutex::new(database));
        let context = Context::new();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let transport = FixtureTransport {
            context: context.clone(),
            requests: requests.clone(),
        };
        let opens = Arc::new(AtomicUsize::new(0));
        let observed = opens.clone();
        let execution = Execution::new(Store::new(config), database.clone(), move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(transport.clone())
        });
        let factory: Arc<pixiv_mcp::download::SaveClientFactory<FixtureTransport>> =
            Arc::new(|client| Arc::new(FixtureSaveClient(client)));
        let result = pixiv_mcp::download::saved_download(
            &execution,
            &context,
            destinations.to_str().unwrap(),
            input,
            None,
            factory,
        )
        .await;
        let mut actual = serde_json::to_value(result).unwrap();
        normalize(&mut actual, destinations.to_str().unwrap());
        assert_eq!(actual, row["response"]["result"], "saved {name}");
        assert_eq!(
            opens.load(Ordering::SeqCst),
            row["opens"].as_u64().unwrap() as usize,
            "saved {name}"
        );
        assert_eq!(
            serde_json::json!(*requests.lock().unwrap()),
            row["requests"],
            "saved {name}"
        );
        if opens.load(Ordering::SeqCst) > 0 {
            let account = database.lock().unwrap().get_pixiv(42).unwrap();
            assert_eq!(account.credential_revision, 2, "saved {name}");
            assert_eq!(
                account.refresh_token_copy(),
                b"fixture-rotated",
                "saved {name}"
            );
        }
    }
}

#[tokio::test]
async fn direct_download_cancellation_waits_for_request_context_and_keeps_published_prefix() {
    let contract = fixture();
    let row = contract["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "partial-cancel")
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let context = Context::new();
    let started = Arc::new(tokio::sync::Notify::new());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let client = support::ManualFixtureSaveClient(Arc::new(Client::with_transport(
        "fixture-access",
        support::ManualCancelTransport {
            inner: FixtureTransport {
                context: context.clone(),
                requests,
            },
            started: started.clone(),
        },
    )));
    let input = decode_download(Some(&row["arguments"])).unwrap();
    let operation =
        pixiv_mcp::download::download(&context, &client, directory.path().to_str().unwrap(), input);
    let cancel = async {
        started.notified().await;
        context.cancel();
    };
    let (result, ()) = tokio::join!(operation, cancel);
    let mut actual = serde_json::to_value(result).unwrap();
    normalize(&mut actual, directory.path().to_str().unwrap());
    assert_eq!(actual, row["response"]["result"]);
}
