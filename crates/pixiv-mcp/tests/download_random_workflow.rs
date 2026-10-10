#[path = "support/download_random_workflow.rs"]
mod support;

use futures_util::FutureExt;
use pixiv_app::{
    config::Store,
    database::{Database, PixivAccount},
    execution::Execution,
    lifecycle::Context,
};
use pixiv_mcp::download::{
    DownloadDefaults, DownloadFuture, DownloadRandomExecutor, DownloadRandomInput,
    SaveClientFactory, saved_download_random_with_account,
};
use pixiv_mcp::{runtime::Account, stdio::DownloadExecutors};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    sync::{Arc, Mutex, atomic::Ordering},
};
use support::{
    FixtureSaveClient, FixtureTransport, Observed, PendingBody, files, normalize, normalize_order,
    scripts,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn fixture() -> Arc<Value> {
    Arc::new(serde_json::from_str(include_str!("fixtures/download_random_workflow.json")).unwrap())
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
fn setup(row: &Value, data: Arc<Value>, pending: Option<PendingBody>) -> Setup {
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
    let db = database.clone();
    let observed = Arc::new(Observed::default());
    let observer = observed.clone();
    let responses = scripts(row);
    let row = row.clone();
    let execution = Arc::new(Execution::new(store, database.clone(), move |options| {
        observer.factories.fetch_add(1, Ordering::SeqCst);
        observer.active.fetch_add(1, Ordering::SeqCst);
        observer
            .proxies
            .lock()
            .unwrap()
            .push(if options.proxy().is_empty() {
                None
            } else {
                Some(options.proxy().into())
            });
        Ok(FixtureTransport {
            observed: observer.clone(),
            fixture: data.clone(),
            row: row.clone(),
            database: db.clone(),
            responses: Mutex::new(responses.clone()),
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
    setup: &Setup,
    row: &Value,
) -> impl Fn(Context, DownloadRandomInput) -> DownloadFuture + Send + Sync + 'static {
    let execution = setup.execution.clone();
    let defaults = setup.defaults.clone();
    let observed = setup.observed.clone();
    let account = Account {
        user_id: row["account_override"].as_i64().unwrap(),
        https_proxy_override: row["https_proxy_override"].as_str().map(Into::into),
    };
    let remove_requested = row["remove_published_before_output"].as_bool().unwrap();
    let remove_published = remove_requested && row.get("detail_overrides").is_some();
    let remove_on_drop = remove_requested && !remove_published;
    move |context, input| {
        let execution = execution.clone();
        let defaults = defaults.clone();
        let observed = observed.clone();
        let account = account.clone();
        Box::pin(async move {
            *observed.request_context.lock().unwrap() = Some(context.clone());
            let factory: Arc<SaveClientFactory<FixtureTransport>> = Arc::new(move |client| {
                Arc::new(FixtureSaveClient {
                    client,
                    remove_published,
                    remove_on_drop,
                    published: Mutex::new(vec![]),
                })
            });
            saved_download_random_with_account(
                &execution, &context, &defaults, input, &account, factory,
            )
            .await
        })
    }
}
fn account_states(database: &Mutex<Database>) -> Value {
    let database = database.lock().unwrap();
    json!([42,43].map(|id|{let account=database.get_pixiv(id).unwrap();json!({"id":id,"revision":account.credential_revision,"frozen":account.pool_frozen_until.is_some(),"selected":account.pool_last_selected})}))
}
fn compare_observations(setup: &Setup, row: &Value, name: &str) {
    let mut requests = setup.observed.requests.lock().unwrap().clone();
    normalize_order(&mut requests, row);
    assert_eq!(
        json!(requests),
        row["requests"],
        "wire order/method/query/auth/referer {name}"
    );
    let mut responses = setup.observed.responses.lock().unwrap().clone();
    normalize_order(&mut responses, row);
    assert_eq!(
        json!(responses),
        row["responses"],
        "response/retry/cause {name}"
    );
    assert_eq!(
        json!(*setup.observed.opens.lock().unwrap()),
        row["opens"],
        "single-account refresh/no replay {name}"
    );
    assert_eq!(
        account_states(&setup.database),
        row["account_states"],
        "saved revision/freeze/selection {name}"
    );
    let (actual_files, directories) = files(Path::new(&setup.defaults.download_path));
    assert_eq!(
        actual_files, row["files"],
        "published bytes/owned temp cleanup {name}"
    );
    assert_eq!(directories, row["directories"], "directories {name}");
    let config = std::fs::read_to_string(setup.directory.path().join("config.toml"))
        .unwrap()
        .replace(&setup.defaults.download_path, "<ROOT>");
    assert_eq!(
        config, row["config_after"],
        "preserve saved settings {name}"
    );
    assert_eq!(
        setup.observed.active.load(Ordering::SeqCst),
        0,
        "Rust transport owner released {name}"
    );
    assert_eq!(
        setup.observed.active_reads.load(Ordering::SeqCst),
        0,
        "owned body released {name}"
    );
    let proxies = setup.observed.proxies.lock().unwrap();
    if !row["https_proxy_override"].is_null() {
        for proxy in proxies.iter() {
            assert_eq!(
                json!(proxy),
                row["https_proxy_override"],
                "explicit account proxy assembly {name}"
            );
        }
    }
    assert!(
        proxies.len() <= row["acquired"].as_u64().unwrap() as usize,
        "single-lease transport construction/no replay {name}"
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
    let mut previous = 0;
    for item in output["items"].as_array().unwrap() {
        let id = item["illust_id"].as_i64().unwrap();
        assert!(
            id > previous,
            "public item output must stay sorted/deduplicated"
        );
        previous = id;
        assert_eq!(item["url"], format!("https://www.pixiv.net/artworks/{id}"));
        assert!(!item["title"].as_str().unwrap().contains("List title"));
        assert!(!item["author"].as_str().unwrap().contains("List author"));
    }
    for content in value["content"].as_array().unwrap() {
        assert_eq!(content["type"], "text");
    }
    let encoded = value.to_string();
    assert!(!encoded.contains("fixture-access"));
    assert!(!encoded.contains("fixture=private"));
}
#[test]
fn random_workflow_fixture_identity_preserves_original_and_appended_stat_inputs() {
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(include_bytes!("fixtures/download_random_workflow.json"))
        ),
        "14b222e8dc6d5e693187c38ddb59e71a3643707c176838b29358434b94dd8753"
    );
    let data = fixture();
    assert_eq!(data["cases"].as_array().unwrap().len(), 49);
    assert_eq!(data["stdio_cancellation"].as_array().unwrap().len(), 2);
    let original = &data["cases"][19];
    assert_eq!(
        original["name"],
        "published-file-removed-before-output-stat"
    );
    assert!(original.get("detail_overrides").is_none());
    assert!(
        original["requests"]
            .as_array()
            .unwrap()
            .iter()
            .any(|request| request["url"].as_str().unwrap().contains("42_p0.jpg"))
    );
    let appended = &data["cases"][48];
    assert_eq!(
        appended["name"],
        "png-identity-file-removed-before-output-stat"
    );
    assert!(
        appended["detail_overrides"]["42"]["illust"]["meta_single_page"]["original_image_url"]
            .as_str()
            .unwrap()
            .contains("42_p0.png")
    );
}
async fn compare_saved_case(row: &Value, data: Arc<Value>) {
    let name = row["name"].as_str().unwrap();
    let setup = setup(row, data, None);
    let execute = executor(&setup, row);
    let input = format!(
        "{}{}\n",
        initialize(),
        json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"download_random_from_recommendation","arguments":row["arguments"]}})
    );
    let mut output = vec![];
    pixiv_mcp::stdio::serve_saved_with_downloads(
        &setup.execution,
        None,
        DownloadExecutors {
            download: None,
            random_from_recommendation: Some(&execute as &DownloadRandomExecutor),
        },
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
    assert_eq!(
        actual, row["response"],
        "actual saved random stdio RPC {name}"
    );
    verify_output(&actual["result"]);
    compare_observations(&setup, row, name);
}
#[tokio::test]
async fn random_singleton_saved_stdio_matches_frozen_go_runtime_result() {
    let data = fixture();
    compare_saved_case(&data["cases"][0], data.clone()).await;
}
#[tokio::test]
async fn random_saved_stdio_matches_go_results_requests_media_settings_and_accounts() {
    let data = fixture();
    let mut failures = vec![];
    for row in data["cases"].as_array().unwrap().iter() {
        let row = row.clone();
        let data = data.clone();
        let name = row["name"].as_str().unwrap().to_owned();
        if std::panic::AssertUnwindSafe(compare_saved_case(&row, data))
            .catch_unwind()
            .await
            .is_err()
        {
            failures.push(name);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
#[tokio::test]
async fn random_subset_public_cardinality_keeps_default_null_clamp_subset_and_order() {
    let data = fixture();
    for (arguments, count) in [
        (json!({"count":2}), 2),
        (json!({}), 5),
        (json!({"count":null}), 5),
    ] {
        let mut row = data["cases"][1].clone();
        row["arguments"] = arguments;
        row["candidate_ids"] = json!([1, 2, 3, 4, 5, 6]);
        let mut recommendation = row["recommendation_responses"][0]["body"].clone();
        let mut sixth = recommendation["illusts"][0].clone();
        sixth["id"] = json!(6);
        recommendation["illusts"]
            .as_array_mut()
            .unwrap()
            .push(sixth);
        row["recommendation_responses"][0]["body"] = recommendation;
        let mut data = (*data).clone();
        data["artwork_metadata"]["6"] = data["artwork_metadata"]["1"].clone();
        data["artwork_metadata"]["6"]["illust"]["id"] = json!(6);
        data["artwork_metadata"]["6"]["illust"]["meta_single_page"]["original_image_url"] = json!(
            "https://i.pximg.net/img-original/img/2026/10/08/12/34/56/6_p0.jpg?fixture=private"
        );
        let destination_setup = setup_subset(&row, Arc::new(data));
        let setup = destination_setup;
        let execute = executor(&setup, &row);
        let input = format!(
            "{}{}\n",
            initialize(),
            json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"download_random_from_recommendation","arguments":row["arguments"]}})
        );
        let mut output = vec![];
        pixiv_mcp::stdio::serve_saved_with_downloads(
            &setup.execution,
            None,
            DownloadExecutors {
                download: None,
                random_from_recommendation: Some(&execute as &DownloadRandomExecutor),
            },
            input.as_bytes(),
            &mut output,
        )
        .await
        .unwrap();
        let actual = std::str::from_utf8(&output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .find(|value| value["id"] == 7)
            .unwrap();
        assert!(actual.get("error").is_none());
        assert!(actual["result"].get("isError").is_none());
        verify_output(&actual["result"]);
        let items = actual["result"]["structuredContent"]["items"]
            .as_array()
            .unwrap();
        assert_eq!(items.len(), count);
        assert_eq!(
            actual["result"]["structuredContent"]["files"]
                .as_array()
                .unwrap()
                .len(),
            count
        );
        for item in items {
            assert!((1..=6).contains(&item["illust_id"].as_i64().unwrap()));
        }
        let (disk, _) = files(Path::new(&setup.defaults.download_path));
        assert_eq!(
            disk.as_array().unwrap().len(),
            count,
            "actual published sample cardinality"
        );
        assert!(
            disk.as_array()
                .unwrap()
                .iter()
                .all(|file| !file["name"].as_str().unwrap().contains(".atomic-write-")),
            "sample atomic cleanup"
        );
        assert_eq!(setup.observed.opens.lock().unwrap().as_slice(), &[43]);
        assert_eq!(setup.observed.active.load(Ordering::SeqCst), 0);
        let requests = setup.observed.requests.lock().unwrap();
        assert_eq!(
            requests[0]["url"],
            "https://oauth.secure.pixiv.net/auth/token"
        );
        assert_eq!(
            requests[1]["url"],
            "https://app-api.pixiv.net/v1/illust/recommended"
        );
        assert_eq!(requests.len(), 2 + count * 2);
    }
}
fn setup_subset(row: &Value, data: Arc<Value>) -> Setup {
    let mut row = row.clone();
    let mut responses = vec![];
    responses.extend(
        row["responses"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|response| {
                response["url"] == "https://oauth.secure.pixiv.net/auth/token"
                    || response["url"] == "https://app-api.pixiv.net/v1/illust/recommended"
            })
            .cloned(),
    );
    for id in 1..=6 {
        responses.push(json!({"url":format!("https://app-api.pixiv.net/v1/illust/detail?illust_id={id}"),"status":200,"retry_after":"","transport_error":""}));
        responses.push(json!({"url":format!("https://i.pximg.net/img-original/img/2026/10/08/12/34/56/{id}_p0.jpg?fixture=private"),"status":200,"retry_after":"","transport_error":""}));
    }
    row["responses"] = json!(responses);
    setup(&row, data, None)
}
#[tokio::test]
async fn random_stdio_owned_body_cancellation_retains_original_cause_prefix_and_reuses_saved_owner()
{
    let data = fixture();
    for cancellation in data["stdio_cancellation"].as_array().unwrap() {
        let row = &cancellation["observed"];
        let kind = cancellation["kind"].as_str().unwrap();
        let ready = Arc::new(tokio::sync::Notify::new());
        let setup = setup(
            row,
            data.clone(),
            Some(PendingBody {
                ready: ready.clone(),
                kind: kind.into(),
            }),
        );
        let execute = executor(&setup, row);
        let (peer, server) = tokio::io::duplex(65536);
        let (read, mut write) = tokio::io::split(server);
        let run = pixiv_mcp::stdio::serve_saved_with_downloads(
            &setup.execution,
            None,
            DownloadExecutors {
                download: None,
                random_from_recommendation: Some(&execute as &DownloadRandomExecutor),
            },
            read,
            &mut write,
        );
        let exchange = async {
            let mut peer = BufReader::new(peer);
            peer.get_mut()
                .write_all(initialize().as_bytes())
                .await
                .unwrap();
            peer.get_mut().write_all(format!("{}\n",json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"download_random_from_recommendation","arguments":cancellation["arguments"]}})).as_bytes()).await.unwrap();
            ready.notified().await;
            assert_eq!(setup.observed.active_reads.load(Ordering::SeqCst), 1);
            let (disk, _) = files(Path::new(&setup.defaults.download_path));
            let mut published = 0;
            let mut temporary = 0;
            for file in disk.as_array().unwrap() {
                if file["name"]
                    .as_str()
                    .unwrap()
                    .rsplit('/')
                    .next()
                    .unwrap()
                    .starts_with(".atomic-write-")
                {
                    temporary += 1
                } else {
                    published += 1
                }
            }
            assert_eq!(json!(published), cancellation["files_before_cancel"]);
            assert_eq!(json!(temporary), cancellation["temporary_before_cancel"]);
            peer.get_mut().write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{\"requestId\":7,\"reason\":\"fixture\"}}\n").await.unwrap();
            for (id, expected) in [
                (7, &cancellation["response"]),
                (8, &cancellation["next_response"]),
            ] {
                if id == 8 {
                    peer.get_mut().write_all(b"{\"jsonrpc\":\"2.0\",\"id\":8,\"method\":\"tools/call\",\"params\":{\"name\":\"download_random_from_recommendation\",\"arguments\":{}}}\n").await.unwrap();
                }
                let mut actual = loop {
                    let mut line = String::new();
                    assert_ne!(peer.read_line(&mut line).await.unwrap(), 0);
                    let value: Value = serde_json::from_str(&line).unwrap();
                    if value["id"] == id {
                        break value;
                    }
                };
                normalize(&mut actual, &setup.defaults.download_path);
                assert_eq!(
                    &actual, expected,
                    "original cause/retained prefix/fresh owner {kind}/{id}"
                );
                assert_eq!(setup.observed.active.load(Ordering::SeqCst), 0);
                assert_eq!(setup.observed.active_reads.load(Ordering::SeqCst), 0);
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            let (served, ()) = tokio::join!(run, exchange);
            served.unwrap();
        })
        .await
        .unwrap();
        compare_observations(&setup, row, kind);
        assert_eq!(setup.observed.factories.load(Ordering::SeqCst), 2);
    }
}
