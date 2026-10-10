use pixiv_app::{config::Store, database::Database, execution::Execution, lifecycle::Context};
use pixiv_mcp::download::{
    DownloadDefaults, DownloadFuture, DownloadRandomInput, SaveClientFactory,
    saved_download_random_with_account,
};
use pixiv_mcp::runtime::Account;
use pixiv_sdk::{
    Result,
    transport::{Request, Response, Transport},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

const FIXTURE: &str = include_str!("fixtures/download_random_options.json");

#[derive(Clone)]
struct NoIo {
    requests: Arc<AtomicUsize>,
}
impl Transport for NoIo {
    async fn send(&self, _: Request) -> Result<Response> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        panic!("preflight/no-account request attempted transport IO")
    }
}
struct Setup {
    directory: tempfile::TempDir,
    execution: Arc<Execution<NoIo>>,
    database: Arc<Mutex<Database>>,
    transport_factories: Arc<AtomicUsize>,
    requests: Arc<AtomicUsize>,
    save_factories: Arc<AtomicUsize>,
}
fn prepare(configuration: &str) -> Setup {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    std::fs::write(&path, configuration).unwrap();
    let database = Arc::new(Mutex::new(Database::open(directory.path()).unwrap()));
    let transport_factories = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(AtomicUsize::new(0));
    let observed_factories = transport_factories.clone();
    let observed_requests = requests.clone();
    let execution = Arc::new(Execution::new(
        Store::new(path),
        database.clone(),
        move |_| {
            observed_factories.fetch_add(1, Ordering::SeqCst);
            Ok(NoIo {
                requests: observed_requests.clone(),
            })
        },
    ));
    Setup {
        directory,
        execution,
        database,
        transport_factories,
        requests,
        save_factories: Arc::new(AtomicUsize::new(0)),
    }
}
fn executor(
    setup: &Setup,
) -> impl Fn(Context, DownloadRandomInput) -> DownloadFuture + Send + Sync + use<> {
    let execution = setup.execution.clone();
    let observed = setup.save_factories.clone();
    move |context, input| {
        let execution = execution.clone();
        let observed = observed.clone();
        Box::pin(async move {
            let factory: Arc<SaveClientFactory<NoIo>> = Arc::new(move |_| {
                observed.fetch_add(1, Ordering::SeqCst);
                panic!("preflight/no-account request constructed media client")
            });
            saved_download_random_with_account(
                &execution,
                &context,
                &DownloadDefaults::default(),
                input,
                &Account::default(),
                factory,
            )
            .await
        })
    }
}
fn initialize() -> String {
    format!(
        "{}\n{}\n",
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"fixture","version":"0"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"})
    )
}
async fn response(setup: &Setup, request: &str) -> Value {
    let execute = executor(setup);
    let input = format!("{}{request}\n", initialize());
    let mut output = Vec::new();
    pixiv_mcp::stdio::serve_saved_with_downloads(
        &setup.execution,
        None,
        pixiv_mcp::stdio::DownloadExecutors {
            download: None,
            random_from_recommendation: Some(&execute),
        },
        input.as_bytes(),
        &mut output,
    )
    .await
    .unwrap();
    String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|value| value["id"] == 7)
        .expect("owned stdio returned requested response")
}
fn assert_no_effects(setup: &Setup, configuration: &str, name: &str, preflight: bool) {
    assert_eq!(setup.requests.load(Ordering::SeqCst), 0, "requests {name}");
    if preflight {
        assert_eq!(
            setup.transport_factories.load(Ordering::SeqCst),
            0,
            "transport construction {name}"
        );
    }
    assert_eq!(
        setup.save_factories.load(Ordering::SeqCst),
        0,
        "media construction {name}"
    );
    assert!(
        setup
            .database
            .lock()
            .unwrap()
            .list_pixiv()
            .unwrap()
            .is_empty(),
        "account mutation {name}"
    );
    assert_eq!(
        std::fs::read_to_string(setup.directory.path().join("config.toml")).unwrap(),
        configuration,
        "configuration mutation {name}"
    );
}

#[test]
fn random_options_fixture_identity_remains_frozen_go() {
    let data: Value = serde_json::from_str(FIXTURE).unwrap();
    assert_eq!(
        format!("{:x}", Sha256::digest(FIXTURE.as_bytes())),
        "65967daedaf09c7d48b40a983a0e6bd0f2f584a03b4def3273fb5ab125be10a9"
    );
    assert_eq!(data["source"], "4b4426487ef18bed276706daec385e0d0a6979f9");
    assert_eq!(data["cases"].as_array().unwrap().len(), 77);
    assert_eq!(data["go_private_selection"].as_array().unwrap().len(), 18);
    assert_eq!(
        data["tool"]["outputSchema"],
        data["main_download_tool"]["outputSchema"]
    );
    assert!(data["tool"]["inputSchema"]["properties"]["count"]["minimum"].is_null());
    assert!(data["tool"]["inputSchema"]["properties"]["count"]["maximum"].is_null());
}

#[tokio::test]
async fn random_registered_tools_list_matches_go() {
    let data: Value = serde_json::from_str(FIXTURE).unwrap();
    let configuration = data["configuration"].as_str().unwrap();
    let setup = prepare(configuration);
    let listed = response(
        &setup,
        r#"{"jsonrpc":"2.0","id":7,"method":"tools/list","params":{}}"#,
    )
    .await;
    let tools = listed["result"]["tools"].as_array().unwrap();
    let random = tools
        .iter()
        .find(|tool| tool["name"] == "download_random_from_recommendation");
    assert_eq!(
        random,
        Some(&data["tool"]),
        "actual registered random schema"
    );
    assert_no_effects(&setup, configuration, "tools/list", true);
}

#[tokio::test]
async fn random_json_rpc_binding_and_saved_handler_preflight_match_go() {
    let data: Value = serde_json::from_str(FIXTURE).unwrap();
    let configuration = data["configuration"].as_str().unwrap();
    let mut differences = Vec::new();
    for row in data["cases"].as_array().unwrap() {
        let name = row["name"].as_str().unwrap();
        let request = row["request"].as_str().unwrap();
        let setup = prepare(configuration);
        let actual = response(&setup, request).await;
        assert_no_effects(&setup, configuration, name, row["open_calls"] == 0);
        if actual != row["response"] {
            differences.push(format!("{name}\nGo: {}\nRust: {actual}", row["response"]));
        }
        if row["open_calls"] == 0 {
            let unread_configuration = "[broken syntax";
            let setup = prepare(unread_configuration);
            let actual = response(&setup, request).await;
            assert_no_effects(&setup, unread_configuration, name, true);
            if actual != row["response"] {
                differences.push(format!(
                    "unread config {name}\nGo: {}\nRust: {actual}",
                    row["response"]
                ));
            }
        }
    }
    assert!(
        differences.is_empty(),
        "{} runtime differences:\n{}",
        differences.len(),
        differences.join("\n")
    );
}

const ENCODING_FIXTURE: &str = include_str!("fixtures/download_random_encoding.json");

#[test]
fn random_encoding_fixture_identity_remains_frozen_go() {
    let data: Value = serde_json::from_str(ENCODING_FIXTURE).unwrap();
    assert_eq!(
        format!("{:x}", Sha256::digest(ENCODING_FIXTURE.as_bytes())),
        "9072264b01ac0a186dfb348e63305fede39c8b20aa4c448a05c60d95a21bbaaf"
    );
    assert_eq!(data["source"], "4b4426487ef18bed276706daec385e0d0a6979f9");
    assert_eq!(data["cases"].as_array().unwrap().len(), 15);
    assert_eq!(
        data["original_options_sha256"],
        format!("{:x}", Sha256::digest(FIXTURE.as_bytes()))
    );
    let original: Value = serde_json::from_str(FIXTURE).unwrap();
    assert_eq!(data["tool"], original["tool"]);
}

#[tokio::test]
async fn random_number_and_collection_diagnostics_match_frozen_go() {
    let data: Value = serde_json::from_str(ENCODING_FIXTURE).unwrap();
    let configuration = data["configuration"].as_str().unwrap();
    let mut differences = Vec::new();
    for row in data["cases"].as_array().unwrap() {
        let name = row["name"].as_str().unwrap();
        let request = row["request"].as_str().unwrap();
        assert_eq!(row["open_calls"], 0, "Go validation boundary {name}");
        for configuration in [configuration, "[broken syntax"] {
            let setup = prepare(configuration);
            let actual = response(&setup, request).await;
            assert_no_effects(&setup, configuration, name, true);
            if actual != row["response"] {
                differences.push(format!("{name}\nGo: {}\nRust: {actual}", row["response"]));
            }
        }
    }
    assert!(
        differences.is_empty(),
        "{} encoding runtime differences:\n{}",
        differences.len(),
        differences.join("\n")
    );
}
