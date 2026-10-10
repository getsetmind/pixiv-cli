#[path = "support/download_records.rs"]
mod fixture;

use pixiv_app::{
    config::Store,
    database::{Database, PixivAccount},
    diagnostics::{Event, Scope},
    download::{DownloadRequest, download_sources},
    execution::Execution,
    facade::UseOutcome,
    lifecycle::Context,
};
use pixiv_cli_rs::{
    CommandError,
    download::{DownloadCommand, DownloadRuntime, DownloadSinks},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicI64, Ordering},
    },
};

const GO_ONLY_FIELDS: &[&str] = &[
    "go_private_attempts",
    "go_gate_port_events",
    "events",
    "pooled",
    "closes",
    "body_closes",
    "held_gate",
    "main_pooled",
    "reader_returned",
    "active_after",
];
fn frozen() -> Value {
    use sha2::{Digest, Sha256};
    let bytes = include_bytes!("fixtures/download_records.json");
    assert_eq!(
        format!("{:x}", Sha256::digest(bytes)),
        "e153248c35b5f44da3576997eeba3cb06165383d42e22596cd463bc6746c0f3c"
    );
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        value["reference"],
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    assert_eq!(value["cases"].as_array().unwrap().len(), 59);
    value
}
struct Environment {
    root: tempfile::TempDir,
    _accounts: tempfile::TempDir,
    store: Store,
    fixture: fixture::Fixture,
}
impl Environment {
    fn new(row: &Value, assets: &Value) -> Self {
        let root = tempfile::tempdir().unwrap();
        let accounts = tempfile::tempdir().unwrap();
        let path = accounts.path().join("config.toml");
        std::fs::write(
            &path,
            "[account_pool]\nenabled=true\nstrategy='round_robin'\n",
        )
        .unwrap();
        let mut database = Database::open(accounts.path()).unwrap();
        if row["failure"] != "no-accounts" {
            for id in [42, 43] {
                database
                    .save_pixiv_credential(&PixivAccount::new(
                        id,
                        "fixture",
                        format!("fixture-refresh-{id}").as_bytes(),
                    ))
                    .unwrap();
            }
        }
        if row["failure"] != "no-schedulable" {
            database.set_all_pixiv_schedulable(true).unwrap();
        }
        if row["failure"] == "already-frozen" {
            for id in [42, 43] {
                database
                    .freeze_pixiv(id, chrono::Utc::now().timestamp() + 3600)
                    .unwrap();
            }
        }
        let fixture = fixture::Fixture {
            row: row.clone(),
            assets: assets.clone(),
            database: Arc::new(Mutex::new(database)),
            observed: Arc::new(Mutex::new(fixture::Observed::default())),
            context: Arc::new(Mutex::new(Context::new())),
            account: Arc::new(AtomicI64::new(0)),
            counts: Arc::new(Mutex::new(BTreeMap::new())),
            disable: Arc::new(AtomicBool::new(false)),
        };
        Self {
            root,
            _accounts: accounts,
            store: Store::new(path),
            fixture,
        }
    }
    fn execution(&self) -> Execution<fixture::Fixture> {
        let transport = self.fixture.clone();
        Execution::new(
            self.store.clone(),
            self.fixture.database.clone(),
            move |_| Ok(transport.clone()),
        )
    }
    fn runtime(&self) -> DownloadRuntime {
        DownloadRuntime {
            download_path: self.root.path().join("runtime").display().to_string(),
            filename_template: self.fixture.row["runtime_filename"]
                .as_str()
                .unwrap()
                .into(),
            directory_template: self.fixture.row["runtime_directory"]
                .as_str()
                .unwrap()
                .into(),
            output_json: self.fixture.row["runtime_json"].as_bool().unwrap(),
        }
    }
    fn normalize(&self, text: &str) -> String {
        text.replace(&self.root.path().display().to_string(), "$ROOT")
    }
    fn manifest(&self) -> (Value, Value) {
        let mut files = BTreeMap::new();
        let mut directories = Vec::new();
        fixture::manifest(
            self.root.path(),
            self.root.path(),
            &mut files,
            &mut directories,
        );
        (json!(files), json!(directories))
    }
    fn accounts(&self) -> Value {
        let db = self.fixture.database.lock().unwrap();
        if self.fixture.row["failure"] == "no-accounts" {
            return json!([]);
        };
        json!([42,43].map(|id|{let account=db.get_pixiv(id).unwrap();json!({"id":id,"revision":account.credential_revision,"frozen":account.pool_frozen_until.is_some_and(|until|until>chrono::Utc::now().timestamp()),"selected":account.pool_last_selected,"rotated":account.refresh_token_copy()==format!("fixture-rotated-{id}").as_bytes()})}))
    }
    fn adapter(
        &self,
        client: Arc<pixiv_sdk::Client<fixture::Fixture>>,
    ) -> Arc<fixture::SaveClient> {
        self.fixture.observed.lock().unwrap().active_adapters += 1;
        Arc::new(fixture::SaveClient {
            client,
            fixture: self.fixture.clone(),
            root: self.root.path().display().to_string(),
        })
    }
}
fn cause(error: Option<&CommandError>) -> String {
    match error {
        Some(CommandError::App(error)) if error.is_canceled() => "cancel".into(),
        Some(CommandError::App(error)) => error
            .classified()
            .map(|e| e.code.to_string())
            .unwrap_or_default(),
        Some(CommandError::Sdk(error)) => error.code.to_string(),
        _ => String::new(),
    }
}
fn compare(failures: &mut Vec<String>, name: &str, field: &str, actual: Value, expected: &Value) {
    if actual != *expected {
        failures.push(format!(
            "{name}: {field}\n actual: {actual}\n expected: {expected}"
        ));
    }
}
fn input(typ: &str, id: i64) -> String {
    format!(
        "{{\"type\":{typ:?},\"id\":{id},\"url\":\"https://misleading.invalid/users/999?private=ignored\"}}\n"
    )
}
fn selected_context(context: &Context, notify: Arc<tokio::sync::Notify>) -> Context {
    context.with_scope(Scope::new(
        Some(Arc::new(move |event: Event| {
            if event.operation == "selected" {
                notify.notify_one();
            }
        })),
        "record test",
        0,
    ))
}

#[tokio::test]
async fn saved_records_match_all_59_frozen_go_public_workflows() {
    let frozen = frozen();
    let mut failures = vec![];
    for row in frozen["cases"].as_array().unwrap() {
        let name = row["name"].as_str().unwrap();
        for field in GO_ONLY_FIELDS {
            assert!(
                !row[*field].is_null() || *field == "held_gate",
                "missing explicit Go-only observer field {field}"
            );
        }
        let environment = Environment::new(row, &frozen["assets_hex"]);
        let mut context = environment.fixture.context.lock().unwrap().clone();
        if row["failure"] == "before-cancel" {
            context.cancel();
        }
        let notify = Arc::new(tokio::sync::Notify::new());
        if row["failure"] == "gate-cancel" {
            context = selected_context(&context, notify.clone());
        }
        let stdout = Arc::new(Mutex::new(Vec::new()));
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let writer = Arc::new(Mutex::new(fixture::Writer {
            bytes: stderr.clone(),
            observed: environment.fixture.observed.clone(),
            failures: usize::from(row["writer_failure"] == "once"),
            persistent: row["writer_failure"] == "persistent",
            root: environment.root.path().display().to_string(),
        }));
        let returned = Arc::new(Mutex::new(Vec::new()));
        let mut reader = fixture::Reader {
            bytes: row["input"].as_str().unwrap().as_bytes().to_vec(),
            position: 0,
            failure: row["reader_failure"].as_str().unwrap().into(),
            context: context.clone(),
            returned: returned.clone(),
        };
        let mut args = vec!["download".to_owned()];
        args.extend(
            row["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap().to_owned()),
        );
        let command = DownloadCommand::parse(&args, &mut reader, false);
        let execution = environment.execution();
        let lease = if row["failure"] == "gate-cancel" {
            Some(
                execution
                    .open_client(&Context::new(), 42, None)
                    .await
                    .unwrap(),
            )
        } else {
            None
        };
        let transport = environment.fixture.clone();
        let root = environment.root.path().display().to_string();
        let result = match command {
            Err(error) => Err(error),
            Ok(command) => {
                let action = command.execute_with_factory(
                    &context,
                    || Ok(environment.runtime()),
                    &mut reader,
                    DownloadSinks {
                        output: stdout.clone(),
                        error: writer,
                    },
                    || Ok(execution),
                    move |client| {
                        transport.observed.lock().unwrap().active_adapters += 1;
                        Arc::new(fixture::SaveClient {
                            client,
                            fixture: transport.clone(),
                            root: root.clone(),
                        })
                    },
                );
                if lease.is_some() {
                    tokio::pin!(action);
                    tokio::time::timeout(std::time::Duration::from_secs(5),async{tokio::select! {result=&mut action=>panic!("held real lease failed to block record command: {result:?}"),_=notify.notified()=>{}}}).await.expect("record scheduler did not reach the held gate");
                    context.cancel();
                    tokio::time::timeout(std::time::Duration::from_secs(5), action)
                        .await
                        .expect("record gate wait did not cancel")
                } else {
                    tokio::time::timeout(std::time::Duration::from_secs(5), action)
                        .await
                        .expect("record command failed to terminate")
                }
            }
        };
        if let Some(lease) = lease {
            lease.close().unwrap();
        }
        if name.starts_with("explicit-source-") {
            assert!(
                returned.lock().unwrap().is_empty(),
                "explicit source consumed stdin"
            );
        }
        let result_error = result
            .as_ref()
            .err()
            .map(|error| environment.normalize(&error.to_string()))
            .unwrap_or_default();
        compare(
            &mut failures,
            name,
            "stdout",
            json!(
                environment.normalize(&String::from_utf8(stdout.lock().unwrap().clone()).unwrap())
            ),
            &row["stdout"],
        );
        compare(
            &mut failures,
            name,
            "stderr",
            json!(
                environment.normalize(&String::from_utf8(stderr.lock().unwrap().clone()).unwrap())
            ),
            &row["stderr"],
        );
        compare(
            &mut failures,
            name,
            "error",
            json!(result_error),
            &row["error"],
        );
        compare(
            &mut failures,
            name,
            "cause",
            json!(cause(result.as_ref().err())),
            &row["cause"],
        );
        compare(
            &mut failures,
            name,
            "pipeline",
            json!(matches!(result, Err(CommandError::Pipeline))),
            &row["pipeline"],
        );
        let (files, directories) = environment.manifest();
        compare(&mut failures, name, "files", files, &row["files"]);
        compare(
            &mut failures,
            name,
            "directories",
            directories,
            &row["directories"],
        );
        {
            let observed = environment.fixture.observed.lock().unwrap();
            compare(
                &mut failures,
                name,
                "main SDK requests",
                json!(observed.requests),
                &json!(
                    row["requests"].as_array().unwrap()
                        [..row["main_requests"].as_u64().unwrap() as usize]
                ),
            );
            compare(
                &mut failures,
                name,
                "main SDK saves",
                json!(observed.saves),
                &json!(
                    row["saves"].as_array().unwrap()
                        [..row["main_saves"].as_u64().unwrap() as usize]
                ),
            );
            let expected_writes = row["writer_attempts"]
                .as_array()
                .unwrap()
                .iter()
                .map(|write| json!({"text":write["text"],"failed":write["failed"]}))
                .collect::<Vec<_>>();
            compare(
                &mut failures,
                name,
                "framed writer text/failure (Go lease marker unconsumed)",
                json!(observed.writes),
                &json!(expected_writes),
            );
            assert_eq!(
                observed.active_adapters, 0,
                "media adapter retained after {name}"
            );
            assert_eq!(
                observed.active_bodies, 0,
                "owned resource body retained after {name}"
            );
        }
        if row["followup"] == true {
            environment.fixture.disable.store(true, Ordering::SeqCst);
            let context = Context::new();
            *environment.fixture.context.lock().unwrap() = context.clone();
            let mut reader = std::io::Cursor::new(input("artwork", 45).into_bytes());
            let command = DownloadCommand::parse(&["download".into()], &mut reader, false).unwrap();
            let out = Arc::new(Mutex::new(Vec::new()));
            let err = Arc::new(Mutex::new(Vec::new()));
            let transport = environment.fixture.clone();
            let root = environment.root.path().display().to_string();
            let result = command
                .execute_with_factory(
                    &context,
                    || Ok(environment.runtime()),
                    &mut reader,
                    DownloadSinks {
                        output: out.clone(),
                        error: err.clone(),
                    },
                    || Ok(environment.execution()),
                    move |client| {
                        transport.observed.lock().unwrap().active_adapters += 1;
                        Arc::new(fixture::SaveClient {
                            client,
                            fixture: transport.clone(),
                            root: root.clone(),
                        })
                    },
                )
                .await;
            let (files, _) = environment.manifest();
            // Go counts SDK leases here; Rust adapter cleanup is asserted independently.
            let actual = json!({"error":result.as_ref().err().map(|error|environment.normalize(&error.to_string())).unwrap_or_default(),"cause":cause(result.as_ref().err()),"stdout":environment.normalize(&String::from_utf8(out.lock().unwrap().clone()).unwrap()),"stderr":environment.normalize(&String::from_utf8(err.lock().unwrap().clone()).unwrap()),"files":files});
            compare(
                &mut failures,
                name,
                "fresh CLI composition sharing saved config/DB",
                actual,
                &{
                    let mut after = row["after"].clone();
                    after.as_object_mut().unwrap().remove("active");
                    after
                },
            );
        }
        let observed = environment.fixture.observed.lock().unwrap();
        compare(
            &mut failures,
            name,
            "all SDK requests",
            json!(observed.requests),
            &row["requests"],
        );
        compare(
            &mut failures,
            name,
            "all SDK saves",
            json!(observed.saves),
            &row["saves"],
        );
        assert_eq!(observed.active_bodies, 0);
        assert_eq!(observed.active_adapters, 0);
        drop(observed);
        compare(
            &mut failures,
            name,
            "persisted accounts",
            environment.accounts(),
            &row["accounts"],
        );
    }
    assert!(
        failures.is_empty(),
        "{} frozen public mismatches:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[tokio::test]
async fn canceled_gate_wait_reuses_the_same_saved_execution_after_holder_release() {
    let frozen = frozen();
    let row = frozen["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["failure"] == "gate-cancel")
        .unwrap();
    let environment = Environment::new(row, &frozen["assets_hex"]);
    let execution = environment.execution();
    let holder = execution
        .open_client(&Context::new(), 42, None)
        .await
        .unwrap();
    let notify = Arc::new(tokio::sync::Notify::new());
    let context = selected_context(&Context::new(), notify.clone());
    let ran = Arc::new(AtomicBool::new(false));
    let flag = ran.clone();
    let action = execution.read(&context, 0, None, move |_, _| {
        flag.store(true, Ordering::SeqCst);
        async { Ok(()) }
    });
    tokio::pin!(action);
    tokio::time::timeout(std::time::Duration::from_secs(5),async{tokio::select! {result=&mut action=>panic!("live lease did not block: {result:?}"),_=notify.notified()=>{}}}).await.expect("same Execution scheduler did not reach held gate");
    context.cancel();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(5), action)
            .await
            .expect("same Execution gate wait did not cancel")
            .unwrap_err()
            .is_canceled()
    );
    assert!(!ran.load(Ordering::SeqCst));
    assert_eq!(
        environment.fixture.observed.lock().unwrap().requests.len(),
        1
    );
    holder.close().unwrap();
    environment.fixture.disable.store(true, Ordering::SeqCst);
    let fresh = Context::new();
    *environment.fixture.context.lock().unwrap() = fresh.clone();
    let transport = environment.fixture.clone();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        execution.read(&fresh, 0, None, move |_, client| {
            let transport = transport.clone();
            async move {
                client.artwork(45).await?;
                assert_eq!(
                    transport
                        .database
                        .lock()
                        .unwrap()
                        .get_pixiv(43)
                        .unwrap()
                        .credential_revision,
                    2
                );
                Ok(())
            }
        }),
    )
    .await
    .expect("same Execution was not reusable after gate cancellation")
    .unwrap();
    assert_eq!(
        environment.fixture.observed.lock().unwrap().requests.len(),
        3
    );
}

#[tokio::test]
async fn owned_second_page_cancel_retains_prefix_and_reuses_the_same_execution() {
    let frozen = frozen();
    let row = frozen["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["failure"] == "page-cancel")
        .unwrap();
    let environment = Arc::new(Environment::new(row, &frozen["assets_hex"]));
    let execution = environment.execution();
    let context = environment.fixture.context.lock().unwrap().clone();
    let run = |environment: Arc<Environment>, id: i64| {
        Arc::new(
            move |context: Context,
                  client: Arc<pixiv_sdk::Client<fixture::Fixture>>|
                  -> pixiv_app::facade::UseFuture {
                let environment = environment.clone();
                Box::pin(async move {
                    let adapter = environment.adapter(client);
                    let request = DownloadRequest {
                        download_path: environment
                            .root
                            .path()
                            .join("runtime")
                            .display()
                            .to_string(),
                        filename_template: "{id}_{num}".into(),
                        quality: "original".into(),
                        ugoira_format: "gif".into(),
                        ..DownloadRequest::default()
                    };
                    let result =
                        download_sources(&context, adapter.as_ref(), &[id.to_string()], &request)
                            .await;
                    UseOutcome {
                        committed: result.report.committed
                            || result
                                .report
                                .items
                                .iter()
                                .any(|item| !item.files.is_empty()),
                        error: result.error,
                    }
                })
            },
        )
    };
    let result = execution
        .use_client(Some(&context), 0, None, Some(run(environment.clone(), 43)))
        .await;
    assert!(result.unwrap_err().is_canceled());
    assert_eq!(environment.manifest().0, row["files"]);
    assert_eq!(
        environment.fixture.observed.lock().unwrap().active_bodies,
        0
    );
    environment.fixture.disable.store(true, Ordering::SeqCst);
    let fresh = Context::new();
    *environment.fixture.context.lock().unwrap() = fresh.clone();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        execution.use_client(Some(&fresh), 0, None, Some(run(environment.clone(), 45))),
    )
    .await
    .expect("same Execution was not reusable after owned body cancellation")
    .unwrap();
    assert_eq!(environment.manifest().0, row["after"]["files"]);
    assert_eq!(environment.accounts(), row["accounts"]);
    let observed = environment.fixture.observed.lock().unwrap();
    assert_eq!(json!(observed.requests), row["requests"]);
    assert_eq!(json!(observed.saves), row["saves"]);
    assert_eq!(observed.active_bodies, 0);
    assert_eq!(observed.active_adapters, 0);
}
