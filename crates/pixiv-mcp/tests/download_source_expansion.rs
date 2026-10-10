#[path = "support/download_source_expansion.rs"]
mod support;

use pixiv_app::{
    config::Store,
    database::{Database, PixivAccount},
    execution::Execution,
    lifecycle::Context,
};
use pixiv_mcp::download::{
    DownloadDefaults, DownloadExecutor, DownloadFuture, DownloadInput, SaveClientFactory,
    download_tool, saved_download_with_account,
};
use pixiv_mcp::runtime::Account;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    sync::{Arc, Mutex, atomic::Ordering},
};
use support::{
    FixtureSaveClient, FixtureTransport, Observed, PendingList, files, normalize, normalize_order,
    scripts,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn fixture() -> Arc<Value> {
    Arc::new(serde_json::from_str(include_str!("fixtures/download_source_expansion.json")).unwrap())
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
fn setup(row: &Value, data: Arc<Value>, pending: Option<Arc<tokio::sync::Notify>>) -> Setup {
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("downloads");
    std::fs::create_dir(&destination).unwrap();
    let config = directory.path().join("config.toml");
    std::fs::write(
        &config,
        row["config_before"]
            .as_str()
            .unwrap()
            .replace("<ROOT>", destination.to_str().unwrap()),
    )
    .unwrap();
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
    let responses = scripts(row);
    let execution = Arc::new(Execution::new(store, database.clone(), move |_| {
        observer.factories.fetch_add(1, Ordering::SeqCst);
        observer.active.fetch_add(1, Ordering::SeqCst);
        Ok(FixtureTransport {
            observed: observer.clone(),
            fixture: data.clone(),
            responses: Mutex::new(responses.clone()),
            pending: pending.clone().map(|ready| PendingList { ready }),
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
    observed: Arc<Observed>,
    account: Account,
) -> impl Fn(Context, DownloadInput) -> DownloadFuture + Send + Sync {
    move |context, input| {
        let execution = execution.clone();
        let defaults = defaults.clone();
        let observed = observed.clone();
        let account = account.clone();
        Box::pin(async move {
            let factory: Arc<SaveClientFactory<FixtureTransport>> = Arc::new(move |client| {
                Arc::new(FixtureSaveClient {
                    client,
                    observed: observed.clone(),
                })
            });
            saved_download_with_account(&execution, &context, &defaults, input, &account, factory)
                .await
        })
    }
}
fn account_states(database: &Mutex<Database>) -> Value {
    let database = database.lock().unwrap();
    json!([42,43].map(|id| {let account=database.get_pixiv(id).unwrap();json!({"id":id,"revision":account.credential_revision,"frozen":account.pool_frozen_until.is_some(),"selected":account.pool_last_selected})}))
}
fn compare_observations(setup: &Setup, row: &Value, name: &str) {
    let mut requests = setup.observed.requests.lock().unwrap().clone();
    normalize_order(&mut requests);
    assert_eq!(json!(requests), row["requests"], "requests {name}");
    let mut responses = setup.observed.responses.lock().unwrap().clone();
    normalize_order(&mut responses);
    assert_eq!(
        json!(responses),
        row["responses"],
        "HTTP statuses/retry/cause {name}"
    );
    assert_eq!(
        json!(*setup.observed.opens.lock().unwrap()),
        row["opens"],
        "saved single-account OAuth {name}"
    );
    assert_eq!(
        account_states(&setup.database),
        row["account_states"],
        "saved state/no pool selection or freeze {name}"
    );
    let (actual_files, actual_directories) = files(Path::new(&setup.defaults.download_path));
    assert_eq!(
        actual_files, row["files"],
        "published bytes/owned cleanup {name}"
    );
    assert_eq!(actual_directories, row["directories"], "directories {name}");
    let config = std::fs::read_to_string(setup.directory.path().join("config.toml"))
        .unwrap()
        .replace(&setup.defaults.download_path, "<ROOT>");
    assert_eq!(config, row["config_after"], "settings preservation {name}");
    assert_eq!(
        setup.observed.active.load(Ordering::SeqCst),
        0,
        "Rust transport ownership released {name}"
    );
    assert_eq!(
        setup.observed.active_reads.load(Ordering::SeqCst),
        0,
        "Rust body ownership released {name}"
    );
}
fn verify_output(value: &Value) {
    let output = &value["structuredContent"];
    let grouped = output["items"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|item| item["files"].as_array().unwrap().clone())
        .collect::<Vec<_>>();
    assert_eq!(json!(grouped), output["files"]);
    for item in output["items"].as_array().unwrap() {
        if item["illust_id"].as_i64().unwrap() > 0 {
            assert_eq!(
                item["url"],
                format!("https://www.pixiv.net/artworks/{}", item["illust_id"])
            )
        }
        assert!(!item["title"].as_str().unwrap().contains("List title"));
        assert!(!item["author"].as_str().unwrap().contains("List author"));
    }
    for content in value["content"].as_array().unwrap() {
        assert_eq!(content["type"], "text")
    }
    let encoded = value.to_string();
    assert!(!encoded.contains("fixture-access"));
    assert!(!encoded.contains("signature=private"));
}
#[test]
fn source_expansion_schema_and_frozen_fixture_identity_match_go() {
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(include_bytes!("fixtures/download_source_expansion.json"))
        ),
        "f78f257872f61de00c2c2b1ec717343d461c5917363bedd943ffd8561fb0ad9a"
    );
    let data = fixture();
    assert_eq!(data["cases"].as_array().unwrap().len(), 21);
    assert_eq!(data["stdio_cancellation"].as_array().unwrap().len(), 2);
    assert_eq!(download_tool(), data["tool"]);
}
async fn compare_saved_case(row: &Value, data: Arc<Value>) {
    let name = row["name"].as_str().unwrap();
    let setup = setup(row, data, None);
    let execute = executor(
        setup.execution.clone(),
        setup.defaults.clone(),
        setup.observed.clone(),
        Account {
            user_id: row["account_override"].as_i64().unwrap(),
            ..Default::default()
        },
    );
    let input = format!(
        "{}{}\n",
        initialize(),
        json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"download","arguments":row["arguments"]}})
    );
    let mut output = vec![];
    pixiv_mcp::stdio::serve_saved_with_download(
        &setup.execution,
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
    normalize(&mut actual, &setup.defaults.download_path);
    assert_eq!(actual, row["response"], "saved stdio {name}");
    verify_output(&actual["result"]);
    compare_observations(&setup, row, name);
}
#[tokio::test]
async fn source_expansion_saved_stdio_matches_go_results_requests_media_settings_and_accounts() {
    let data = fixture();
    for row in data["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["account_override"] == 0)
    {
        compare_saved_case(row, data.clone()).await;
    }
}
#[tokio::test]
async fn source_expansion_explicit_saved_account_leaves_persisted_default_unchanged() {
    let data = fixture();
    let row = data["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "explicit-account-leaves-default-unchanged")
        .unwrap();
    compare_saved_case(row, data.clone()).await;
}
#[tokio::test]
async fn source_expansion_stdio_list_read_cancellation_keeps_original_cause_and_reuses_saved_owner()
{
    let data = fixture();
    for cancellation in data["stdio_cancellation"].as_array().unwrap() {
        let row = &cancellation["observed"];
        let kind = cancellation["kind"].as_str().unwrap();
        let ready = Arc::new(tokio::sync::Notify::new());
        let setup = setup(row, data.clone(), Some(ready.clone()));
        let execute = executor(
            setup.execution.clone(),
            setup.defaults.clone(),
            setup.observed.clone(),
            Account {
                user_id: row["account_override"].as_i64().unwrap(),
                ..Default::default()
            },
        );
        let (peer, server) = tokio::io::duplex(65536);
        let (read, mut write) = tokio::io::split(server);
        let run = pixiv_mcp::stdio::serve_saved_with_download(
            &setup.execution,
            None,
            &execute as &DownloadExecutor,
            read,
            &mut write,
        );
        let exchange = async {
            let mut peer = BufReader::new(peer);
            peer.get_mut()
                .write_all(initialize().as_bytes())
                .await
                .unwrap();
            peer.get_mut().write_all(format!("{}\n",json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"download","arguments":cancellation["arguments"]}})).as_bytes()).await.unwrap();
            ready.notified().await;
            assert_eq!(
                setup.observed.active_reads.load(Ordering::SeqCst),
                1,
                "list body entered {kind}"
            );
            let (disk, _) = files(Path::new(&setup.defaults.download_path));
            assert_eq!(
                disk,
                json!([]),
                "no media acquisition before expansion completes {kind}"
            );
            peer.get_mut().write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{\"requestId\":7,\"reason\":\"fixture\"}}\n").await.unwrap();
            let mut actual = loop {
                let mut line = String::new();
                assert_ne!(peer.read_line(&mut line).await.unwrap(), 0);
                let value: Value = serde_json::from_str(&line).unwrap();
                if value["id"] == 7 {
                    break value;
                }
            };
            normalize(&mut actual, &setup.defaults.download_path);
            assert_eq!(
                actual, cancellation["response"],
                "original SDK/context cause {kind}"
            );
            assert_eq!(
                setup.observed.active.load(Ordering::SeqCst),
                0,
                "canceled saved transport released {kind}"
            );
            assert_eq!(
                setup.observed.active_reads.load(Ordering::SeqCst),
                0,
                "canceled body released {kind}"
            );
            peer.get_mut().write_all(b"{\"jsonrpc\":\"2.0\",\"id\":8,\"method\":\"tools/call\",\"params\":{\"name\":\"download\",\"arguments\":{\"src\":\"42\"}}}\n").await.unwrap();
            let mut actual = loop {
                let mut line = String::new();
                assert_ne!(peer.read_line(&mut line).await.unwrap(), 0);
                let value: Value = serde_json::from_str(&line).unwrap();
                if value["id"] == 8 {
                    break value;
                }
            };
            normalize(&mut actual, &setup.defaults.download_path);
            assert_eq!(
                actual, cancellation["next_response"],
                "fresh operation on released saved owner {kind}"
            );
        };
        let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            tokio::join!(run, exchange)
        })
        .await
        .expect("bounded list-body entered/cancelled/reused exchange");
        result.unwrap();
        compare_observations(&setup, row, kind);
    }
}
