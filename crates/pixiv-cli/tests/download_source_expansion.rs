#![cfg(target_os = "linux")]

#[path = "support/download_source_expansion.rs"]
mod fixture;

use pixiv_app::{
    config::Store,
    database::{Database, PixivAccount},
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
    sync::{Arc, Mutex},
};

fn owned_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .thread_keep_alive(std::time::Duration::from_secs(300))
        .build()
        .unwrap()
}

const GO_ONLY_FIELDS: &[&str] = &[
    "go_port_events",
    "go_pooled_calls",
    "go_private_attempts",
    "go_gate_port_events",
    "go_client_closes",
    "go_body_closes",
    "go_active_after",
    "go_writer_under_lease",
];
fn frozen() -> Value {
    use sha2::{Digest, Sha256};
    let bytes = include_bytes!("fixtures/download_source_expansion.json");
    assert_eq!(
        format!("{:x}", Sha256::digest(bytes)),
        "cc5acf1f06e473810fa8d35656c27f21d98afa404b04a41ab7648d595b0d0aab"
    );
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        value["reference"],
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    assert_eq!(value["go_worker_count"], 1);
    assert_eq!(value["cases"].as_array().unwrap().len(), 41);
    value
}
struct Environment {
    root: tempfile::TempDir,
    _accounts: tempfile::TempDir,
    store: Store,
    fixture: fixture::Fixture,
    runtime: DownloadRuntime,
}
impl Environment {
    fn new(row: &Value) -> Self {
        let root = tempfile::tempdir().unwrap();
        let accounts = tempfile::tempdir().unwrap();
        let path = accounts.path().join("config.toml");
        std::fs::write(
            &path,
            "[account_pool]\nenabled=true\nstrategy='round_robin'\n",
        )
        .unwrap();
        let mut database = Database::open(accounts.path()).unwrap();
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
        let runtime = DownloadRuntime {
            download_path: root.path().join("runtime").display().to_string(),
            filename_template: row["runtime_filename"].as_str().unwrap().into(),
            directory_template: row["runtime_directory"].as_str().unwrap().into(),
            output_json: row["runtime_json"].as_bool().unwrap(),
        };
        Self {
            root,
            _accounts: accounts,
            store: Store::new(path),
            fixture: fixture::Fixture::new(row, database),
            runtime,
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
    fn normalize(&self, text: &str) -> String {
        text.replace(&self.root.path().display().to_string(), "$ROOT")
    }
    fn manifest(&self) -> (Value, Value) {
        let mut files = BTreeMap::new();
        let mut directories = vec![];
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
        json!([42,43].map(|id|{let a=db.get_pixiv(id).unwrap();json!({"id":id,"revision":a.credential_revision,"frozen":a.pool_frozen_until.is_some_and(|until|until>chrono::Utc::now().timestamp()),"selected":a.pool_last_selected,"rotated":a.refresh_token_copy()==format!("fixture-rotated-{id}").as_bytes()})}))
    }
    async fn command(
        &self,
        context: &Context,
        args: Vec<String>,
        input: &str,
    ) -> (Result<(), CommandError>, String, String) {
        *self.fixture.context.lock().unwrap() = context.clone();
        let mut input = std::io::Cursor::new(input.as_bytes());
        let mut actual_args = vec!["download".into()];
        actual_args.extend(
            args.into_iter()
                .map(|arg| arg.replace("$ROOT", &self.root.path().display().to_string())),
        );
        let out = Arc::new(Mutex::new(Vec::new()));
        let err = Arc::new(Mutex::new(Vec::new()));
        let result = match DownloadCommand::parse(&actual_args, &mut input, false) {
            Err(error) => Err(error),
            Ok(command) => {
                let transport = self.fixture.clone();
                let root = self.root.path().display().to_string();
                tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    command.execute_with_factory(
                        context,
                        || Ok(self.runtime.clone()),
                        &mut input,
                        DownloadSinks {
                            output: out.clone(),
                            error: err.clone(),
                        },
                        || Ok(self.execution()),
                        move |client| {
                            transport.observed.lock().unwrap().active_adapters += 1;
                            Arc::new(fixture::SaveClient {
                                client,
                                fixture: transport.clone(),
                                root: root.clone(),
                            })
                        },
                    ),
                )
                .await
                .expect("finite SDK source expansion did not complete/cancel")
            }
        };
        let stdout = self.normalize(&String::from_utf8(out.lock().unwrap().clone()).unwrap());
        let stderr = self.normalize(&String::from_utf8(err.lock().unwrap().clone()).unwrap());
        (result, stdout, stderr)
    }
}
fn cause(error: Option<&CommandError>) -> String {
    match error {
        Some(CommandError::App(error)) if error.is_canceled() => "cancel".into(),
        Some(CommandError::App(error)) => error
            .classified()
            .map(|error| error.code.to_string())
            .unwrap_or_default(),
        Some(CommandError::Sdk(error)) => {
            if pixiv_sdk::error::is_canceled(error) {
                "cancel".into()
            } else {
                error.code.to_string()
            }
        }
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

#[cfg(target_os = "linux")]
#[test]
fn saved_source_expansion_matches_41_frozen_go_cli_workflows() {
    owned_runtime().block_on(async {
        tokio::task::spawn_blocking(|| ()).await.unwrap();
        run_saved_source_expansion_matches_41_frozen_go_cli_workflows().await;
    });
}

async fn run_saved_source_expansion_matches_41_frozen_go_cli_workflows() {
    let _single_worker = fixture::OneWorker::acquire();
    let frozen = frozen();
    let mut failures = vec![];
    for row in frozen["cases"].as_array().unwrap() {
        let name = row["name"].as_str().unwrap();
        for field in GO_ONLY_FIELDS {
            assert!(
                !row[*field].is_null(),
                "missing explicit private Go observer {field}"
            );
        }
        let env = Environment::new(row);
        let context = Context::new();
        if row["failure"] == "before-cancel" {
            context.cancel();
        }
        let args = row["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|arg| arg.as_str().unwrap().into())
            .collect();
        let (result, stdout, stderr) = env
            .command(&context, args, row["input"].as_str().unwrap())
            .await;
        for (field, actual) in [
            ("stdout", json!(stdout)),
            ("stderr", json!(stderr)),
            (
                "error",
                json!(
                    result
                        .as_ref()
                        .err()
                        .map(|error| env.normalize(&error.to_string()))
                        .unwrap_or_default()
                ),
            ),
            ("cause", json!(cause(result.as_ref().err()))),
            (
                "pipeline",
                json!(matches!(result, Err(CommandError::Pipeline))),
            ),
            ("parent_canceled", json!(context.error().is_some())),
        ] {
            compare(&mut failures, name, field, actual, &row[field]);
        }
        let (files, directories) = env.manifest();
        compare(&mut failures, name, "files", files, &row["files"]);
        compare(
            &mut failures,
            name,
            "directories",
            directories,
            &row["directories"],
        );
        {
            let o = env.fixture.observed.lock().unwrap();
            compare(
                &mut failures,
                name,
                "main SDK requests",
                json!(o.requests),
                &json!(
                    row["requests"].as_array().unwrap()
                        [..row["main_requests"].as_u64().unwrap() as usize]
                ),
            );
            compare(
                &mut failures,
                name,
                "main manager SDK saves",
                json!(o.saves),
                &json!(
                    row["saves"].as_array().unwrap()
                        [..row["main_saves"].as_u64().unwrap() as usize]
                ),
            );
            assert_eq!(o.active_adapters, 0, "adapter leaked after {name}");
            assert_eq!(o.active_bodies, 0, "body leaked after {name}");
        }
        if row["followup"] == true {
            let (result, stdout, stderr) =
                env.command(&Context::new(), vec!["45".into()], "").await;
            let (files, _) = env.manifest();
            let actual = json!({"error":result.as_ref().err().map(|error|env.normalize(&error.to_string())).unwrap_or_default(),"cause":cause(result.as_ref().err()),"stdout":stdout,"stderr":stderr,"files":files});
            let mut after = row["after"].clone();
            after.as_object_mut().unwrap().remove("active");
            compare(
                &mut failures,
                name,
                "fresh CLI sharing saved SQLite/config",
                actual,
                &after,
            );
        }
        let o = env.fixture.observed.lock().unwrap();
        compare(
            &mut failures,
            name,
            "all SDK requests",
            json!(o.requests),
            &row["requests"],
        );
        compare(
            &mut failures,
            name,
            "all manager SDK saves",
            json!(o.saves),
            &row["saves"],
        );
        assert_eq!(o.active_adapters, 0);
        assert_eq!(o.active_bodies, 0);
        drop(o);
        compare(
            &mut failures,
            name,
            "persisted accounts",
            env.accounts(),
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

#[cfg(target_os = "linux")]
#[test]
fn canceled_lists_release_the_same_saved_execution_for_a_fresh_operation() {
    owned_runtime().block_on(async {
        tokio::task::spawn_blocking(|| ()).await.unwrap();
        run_canceled_lists_release_the_same_saved_execution_for_a_fresh_operation().await;
    });
}

async fn run_canceled_lists_release_the_same_saved_execution_for_a_fresh_operation() {
    let _single_worker = fixture::OneWorker::acquire();
    let frozen = frozen();
    for row in frozen["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["followup"] == true)
    {
        let env = Environment::new(row);
        let execution = env.execution();
        let context = Context::new();
        if row["failure"] == "before-cancel" {
            context.cancel();
        }
        *env.fixture.context.lock().unwrap() = context.clone();
        let sources = row["args"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|arg| {
                arg.as_str()
                    .filter(|arg| !arg.starts_with("--"))
                    .map(str::to_owned)
            })
            .collect::<Vec<_>>();
        let request = DownloadRequest {
            download_path: env.runtime.download_path.clone(),
            filename_template: env.runtime.filename_template.clone(),
            ugoira_format: "zip".into(),
            ..Default::default()
        };
        let transport = env.fixture.clone();
        let root = env.root.path().display().to_string();
        let error = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            execution.use_client(
                Some(&context),
                0,
                None,
                Some(Arc::new(move |context, client| {
                    let transport = transport.clone();
                    let root = root.clone();
                    let sources = sources.clone();
                    let request = request.clone();
                    Box::pin(async move {
                        transport.observed.lock().unwrap().active_adapters += 1;
                        let adapter = fixture::SaveClient {
                            client,
                            fixture: transport,
                            root,
                        };
                        let attempt =
                            download_sources(&context, &adapter, &sources, &request).await;
                        UseOutcome {
                            committed: attempt.report.committed,
                            error: attempt.error,
                        }
                    })
                })),
            ),
        )
        .await
        .expect("saved list cancellation did not terminate")
        .unwrap_err();
        assert!(
            error.is_canceled(),
            "{} lost cancellation: {error}",
            row["name"]
        );
        let context = Context::new();
        *env.fixture.context.lock().unwrap() = context.clone();
        let transport = env.fixture.clone();
        let root = env.root.path().display().to_string();
        let request = DownloadRequest {
            download_path: env.runtime.download_path.clone(),
            filename_template: env.runtime.filename_template.clone(),
            ..Default::default()
        };
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            execution.use_client(
                Some(&context),
                0,
                None,
                Some(Arc::new(move |context, client| {
                    let transport = transport.clone();
                    let root = root.clone();
                    let request = request.clone();
                    Box::pin(async move {
                        transport.observed.lock().unwrap().active_adapters += 1;
                        let adapter = fixture::SaveClient {
                            client,
                            fixture: transport,
                            root,
                        };
                        let attempt =
                            download_sources(&context, &adapter, &["45".into()], &request).await;
                        assert!(attempt.report.failures.is_empty());
                        UseOutcome {
                            committed: attempt.report.committed,
                            error: attempt.error,
                        }
                    })
                })),
            ),
        )
        .await
        .expect("same saved Execution lease/gate was not released")
        .unwrap();
        let (files, _) = env.manifest();
        assert_eq!(files, row["after"]["files"]);
        let o = env.fixture.observed.lock().unwrap();
        assert_eq!(
            json!(o.requests),
            row["requests"],
            "same-owner request sequence changed"
        );
        assert_eq!(
            json!(o.saves),
            row["saves"],
            "same-owner manager-save sequence changed"
        );
        assert_eq!(o.active_adapters, 0);
        assert_eq!(o.active_bodies, 0);
        drop(o);
        assert_eq!(env.accounts(), row["accounts"]);
    }
}
