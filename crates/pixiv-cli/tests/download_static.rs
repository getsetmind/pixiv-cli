#[path = "support/download_static.rs"]
mod fixture;

use pixiv_app::{
    config::Store,
    database::{Database, PixivAccount},
    execution::Execution,
    lifecycle::Context,
};
use pixiv_cli_rs::{
    CommandError,
    download::{DownloadCommand, DownloadRuntime, DownloadSinks},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, atomic::AtomicI64},
};

fn cause(error: Option<&CommandError>) -> String {
    match error {
        Some(CommandError::App(error)) if error.is_canceled() => "cancel".into(),
        Some(CommandError::App(error)) => error
            .classified()
            .map(|error| error.code.to_string())
            .unwrap_or_default(),
        Some(CommandError::Sdk(error)) => error.code.to_string(),
        _ => String::new(),
    }
}

#[tokio::test]
async fn static_artwork_cli_matches_all_frozen_go_saved_sdk_files_outputs_and_errors() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/download_static.json")).unwrap();
    assert_eq!(
        fixture["reference"],
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    let rows = fixture["cases"].as_array().unwrap();
    assert_eq!(rows.len(), 40);
    for row in rows {
        let name = row["name"].as_str().unwrap();
        let root = tempfile::tempdir().unwrap();
        let root_text = root.path().display().to_string();
        let normalize = |s: &str| s.replace(&root_text, "$ROOT");
        let replace = |s: &str| s.replace("$ROOT", &root_text);
        let account_dir = tempfile::tempdir().unwrap();
        let path = account_dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[account_pool]\nenabled=true\nstrategy='round_robin'\n",
        )
        .unwrap();
        let mut db = Database::open(account_dir.path()).unwrap();
        for id in [42, 43] {
            db.save_pixiv_credential(&PixivAccount::new(
                id,
                "fixture",
                format!("fixture-refresh-{id}").as_bytes(),
            ))
            .unwrap();
        }
        db.set_all_pixiv_schedulable(true).unwrap();
        let db = Arc::new(Mutex::new(db));
        let observed = Arc::new(Mutex::new(fixture::Observed::default()));
        let context = Context::new();
        if row["failure"] == "before-cancel" {
            context.cancel();
        }
        let transport = fixture::Fixture {
            row: row.clone(),
            database: db.clone(),
            observed: observed.clone(),
            context: context.clone(),
            account: Arc::new(AtomicI64::new(0)),
            counts: Arc::new(Mutex::new(BTreeMap::new())),
        };
        let stdout = Arc::new(Mutex::new(Vec::new()));
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let out = Arc::new(Mutex::new(fixture::Writer {
            bytes: stdout.clone(),
            observed: observed.clone(),
            fail: row["failure"] == "writer",
        }));
        let err_out = Arc::new(Mutex::new(fixture::Writer {
            bytes: stderr.clone(),
            observed: observed.clone(),
            fail: false,
        }));
        let mut input = std::io::Cursor::new(row["input"].as_str().unwrap().as_bytes());
        let mut args = vec!["download".to_owned()];
        args.extend(
            row["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| replace(value.as_str().unwrap())),
        );
        let runtime_called = std::cell::Cell::new(false);
        let execution_called = std::cell::Cell::new(false);
        let factory_observed = observed.clone();
        let factory_root = root_text.clone();
        let parsed = DownloadCommand::parse(&args, &mut input, false);
        let result = match parsed {
            Err(error) => Err(error),
            Ok(command) => {
                command
                    .execute_with_factory(
                        &context,
                        || {
                            runtime_called.set(true);
                            Ok(DownloadRuntime {
                                download_path: root.path().join("runtime").display().to_string(),
                                filename_template: row["runtime_filename"].as_str().unwrap().into(),
                                directory_template: row["runtime_directory"]
                                    .as_str()
                                    .unwrap()
                                    .into(),
                                output_json: row["runtime_json"].as_bool().unwrap(),
                            })
                        },
                        &mut input,
                        DownloadSinks {
                            output: out,
                            error: err_out,
                        },
                        || {
                            execution_called.set(true);
                            Ok(Execution::new(Store::new(path), db.clone(), move |_| {
                                Ok(transport.clone())
                            }))
                        },
                        move |client| {
                            let mut observed = factory_observed.lock().unwrap();
                            observed.active += 1;
                            observed.adapters += 1;
                            drop(observed);
                            Arc::new(fixture::SaveClient {
                                client,
                                observed: factory_observed.clone(),
                                root: factory_root.clone(),
                                committed: std::sync::atomic::AtomicBool::new(false),
                            })
                        },
                    )
                    .await
            }
        };
        assert_eq!(
            normalize(&String::from_utf8(stdout.lock().unwrap().clone()).unwrap()),
            row["stdout"].as_str().unwrap(),
            "stdout {name}"
        );
        assert_eq!(
            normalize(&String::from_utf8(stderr.lock().unwrap().clone()).unwrap()),
            row["stderr"].as_str().unwrap(),
            "stderr {name}"
        );
        assert_eq!(
            result
                .as_ref()
                .err()
                .map(|error| normalize(&error.to_string()))
                .unwrap_or_default(),
            row["error"].as_str().unwrap(),
            "error {name}"
        );
        assert_eq!(
            cause(result.as_ref().err()),
            row["cause"].as_str().unwrap(),
            "typed cause {name}"
        );
        let events = row["events"].as_array().unwrap();
        assert_eq!(
            runtime_called.get(),
            events.iter().any(|event| event == "runtime"),
            "runtime boundary {name}"
        );
        assert_eq!(
            execution_called.get(),
            events.iter().any(|event| event == "pool"),
            "execution boundary {name}"
        );
        let observed = observed.lock().unwrap();
        assert_eq!(
            serde_json::to_value(&observed.requests).unwrap(),
            row["requests"],
            "SDK request trace {name}"
        );
        assert_eq!(
            serde_json::to_value(&observed.saves).unwrap(),
            row["saves"],
            "opaque ref/destination trace {name}"
        );
        let committed = row["attempts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|attempt| attempt["committed"].as_bool().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            observed.commits, committed,
            "publication/replay boundary {name}"
        );
        assert_eq!(
            observed.closes,
            row["closes"].as_u64().unwrap() as usize,
            "completed adapter lifetimes {name}"
        );
        assert_eq!(observed.active, 0, "adapter leaked {name}");
        assert_eq!(
            observed.writer_under_lease,
            row["writer_under_lease"].as_bool().unwrap(),
            "writer ownership {name}"
        );
        let db = db.lock().unwrap();
        let accounts=[42,43].map(|id|{let account=db.get_pixiv(id).unwrap();json!({"id":id,"revision":account.credential_revision,"frozen":account.pool_frozen_until.is_some_and(|until|until>chrono::Utc::now().timestamp()),"selected":account.pool_last_selected,"rotated":account.refresh_token_copy()==format!("fixture-rotated-{id}").as_bytes()})});
        assert_eq!(
            serde_json::to_value(accounts).unwrap(),
            row["accounts"],
            "persisted account state {name}"
        );
        let mut files = BTreeMap::new();
        let mut directories = Vec::new();
        fixture::manifest(root.path(), root.path(), &mut files, &mut directories);
        assert_eq!(
            serde_json::to_value(files).unwrap(),
            row["files"],
            "published disk bytes {name}"
        );
        assert_eq!(
            serde_json::to_value(directories).unwrap(),
            row["directories"],
            "directory effect ordering {name}"
        );
        assert_eq!(
            observed.adapters,
            row["attempts"].as_array().unwrap().len(),
            "account attempt count {name}"
        );
    }
}
