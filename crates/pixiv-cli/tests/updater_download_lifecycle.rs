#[path = "support/download_static.rs"]
mod download_fixture;

use pixiv_app::{
    config::Store,
    database::{Database, PixivAccount},
    execution::Execution,
    lifecycle::Context,
};
use pixiv_cli_rs::{
    download::{DownloadCommand, DownloadLifecycle, DownloadRuntime, DownloadSinks},
    update::{PostSuccessFuture, PostSuccessPolicy},
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicI64, AtomicUsize, Ordering},
    },
};

#[tokio::test]
async fn direct_and_record_downloads_keep_the_actual_database_live_until_the_success_hook_finishes()
{
    let frozen: Value =
        serde_json::from_str(include_str!("fixtures/download_static.json")).unwrap();
    let row = frozen["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["failure"] == "" && row["metadata_pages"] == 1)
        .unwrap()
        .clone();
    for records in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(
            &path,
            "request_interval='0s'\n[pixiv.auth]\ndefault_user_id=42\n",
        )
        .unwrap();
        let mut database = Database::open(directory.path()).unwrap();
        database
            .save_pixiv_credential(&PixivAccount::new(42, "fixture", b"fixture-refresh-42"))
            .unwrap();
        let database = Arc::new(Mutex::new(database));
        let weak = Arc::downgrade(&database);
        let observed = Arc::new(Mutex::new(download_fixture::Observed::default()));
        let context = Context::new();
        let transport = download_fixture::Fixture {
            row: row.clone(),
            database: database.clone(),
            observed: observed.clone(),
            context: context.clone(),
            account: Arc::new(AtomicI64::new(0)),
            counts: Arc::new(Mutex::new(BTreeMap::new())),
        };
        let raw = if records {
            vec!["download", "--ndjson"]
        } else {
            vec!["download", "42", "--json"]
        };
        let mut input = std::io::Cursor::new(if records {
            b"{\"type\":\"artwork\",\"id\":\"42\",\"url\":\"https://www.pixiv.net/artworks/42\"}\n"
                .to_vec()
        } else {
            Vec::new()
        });
        let args = raw.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
        let command = DownloadCommand::parse(&args, &mut input, false).unwrap();
        let called = AtomicUsize::new(0);
        let hook = |policy: PostSuccessPolicy| -> PostSuccessFuture<'_> {
            assert!(!policy.skip_automatic_update);
            Box::pin(async {
                {
                    let observed = observed.lock().unwrap();
                    assert_eq!(observed.active, 0);
                    assert_eq!(observed.closes, 1);
                }
                assert_eq!(
                    weak.upgrade()
                        .unwrap()
                        .lock()
                        .unwrap()
                        .get_pixiv(42)
                        .unwrap()
                        .credential_revision,
                    2
                );
                tokio::task::yield_now().await;
                assert!(weak.upgrade().is_some());
                called.fetch_add(1, Ordering::SeqCst);
            })
        };
        let factory_observed = observed.clone();
        let factory_root = directory.path().display().to_string();
        let output_bytes = Arc::new(Mutex::new(Vec::new()));
        let output = Arc::new(Mutex::new(download_fixture::Writer {
            bytes: output_bytes.clone(),
            observed: observed.clone(),
            fail: false,
        }));
        command
            .execute_with_factory_and_post_success(
                DownloadLifecycle {
                    context: &context,
                    post_success: Some(&hook),
                },
                || {
                    Ok(DownloadRuntime {
                        download_path: directory.path().join("downloads").display().to_string(),
                        filename_template: "{id}".into(),
                        ..DownloadRuntime::default()
                    })
                },
                &mut input,
                DownloadSinks {
                    output,
                    error: Arc::new(Mutex::new(Vec::new())),
                },
                move || {
                    Ok(Execution::new(Store::new(path), database, move |_| {
                        Ok(transport.clone())
                    }))
                },
                move |client| {
                    let mut observed = factory_observed.lock().unwrap();
                    observed.active += 1;
                    observed.adapters += 1;
                    drop(observed);
                    Arc::new(download_fixture::SaveClient {
                        client,
                        observed: factory_observed.clone(),
                        root: factory_root.clone(),
                        committed: std::sync::atomic::AtomicBool::new(false),
                    })
                },
            )
            .await
            .unwrap();
        assert_eq!(called.load(Ordering::SeqCst), 1);
        if !records {
            assert!(observed.lock().unwrap().writer_under_lease);
            assert!(!output_bytes.lock().unwrap().is_empty());
        }
        let mut files = BTreeMap::new();
        let mut directories = Vec::new();
        download_fixture::manifest(
            directory.path(),
            &directory.path().join("downloads"),
            &mut files,
            &mut directories,
        );
        assert_eq!(files.len(), 1);
        assert!(
            weak.upgrade().is_none(),
            "actual download DB outlived the command owner"
        );
    }
}

#[tokio::test]
async fn help_and_failed_download_preparation_do_not_invoke_the_success_hook_or_open_execution() {
    use pixiv_cli_rs::CommandError;
    let context = Context::new();
    for raw in [
        vec!["download", "--help"],
        vec!["download", "42", "--quality=invalid"],
    ] {
        let mut input = std::io::Cursor::new(Vec::<u8>::new());
        let args = raw.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
        let command = DownloadCommand::parse(&args, &mut input, false).unwrap();
        let called = AtomicUsize::new(0);
        let hook = |_: PostSuccessPolicy| -> PostSuccessFuture<'_> {
            Box::pin(async {
                called.fetch_add(1, Ordering::SeqCst);
            })
        };
        let result = command
            .execute_with_factory_and_post_success(
                DownloadLifecycle {
                    context: &context,
                    post_success: Some(&hook),
                },
                || Ok(DownloadRuntime::default()),
                &mut input,
                DownloadSinks {
                    output: Arc::new(Mutex::new(Vec::new())),
                    error: Arc::new(Mutex::new(Vec::new())),
                },
                || -> Result<Execution<download_fixture::Fixture>, CommandError> {
                    panic!("help/failure must not open the actual execution")
                },
                |_| -> Arc<dyn pixiv_app::download::DownloadSaveClient> {
                    panic!("help/failure must not construct the download adapter")
                },
            )
            .await;
        assert_eq!(result.is_ok(), raw[1] == "--help");
        assert_eq!(called.load(Ordering::SeqCst), 0);
    }
}
