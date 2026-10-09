#[allow(dead_code)]
#[path = "support/download_fixture.rs"]
mod fixture;
use pixiv_app::{
    config::Store,
    database::{Database, PixivAccount},
    download::{DownloadSaveClient, SaveFuture},
    execution::Execution,
    lifecycle::Context,
};
use pixiv_cli_rs::download::{DownloadCommand, DownloadRuntime, DownloadSinks};
use pixiv_sdk::resource::ResourceRef;
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
struct ActiveClient {
    inner: fixture::SaveClient,
    active: Arc<AtomicBool>,
}
impl Drop for ActiveClient {
    fn drop(&mut self) {
        self.active.store(false, Ordering::SeqCst);
    }
}
impl DownloadSaveClient for ActiveClient {
    fn save_ref(&self, context: Context, reference: ResourceRef, path: PathBuf) -> SaveFuture<'_> {
        self.inner.save_ref(context, reference, path)
    }
    fn save_url(&self, context: Context, url: String, path: PathBuf) -> SaveFuture<'_> {
        self.inner.save_url(context, url, path)
    }
}
struct ObservingWriter {
    active: Arc<AtomicBool>,
    called: Arc<AtomicBool>,
    path: PathBuf,
    kind: std::io::ErrorKind,
}
impl std::io::Write for ObservingWriter {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        assert!(
            self.active.load(Ordering::SeqCst),
            "output must be written inside the owned client callback"
        );
        assert!(
            self.path.exists(),
            "file must be published before report output"
        );
        self.called.store(true, Ordering::SeqCst);
        Err(std::io::Error::new(self.kind, "fixture writer failure"))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
async fn run_output_failure(
    kind: std::io::ErrorKind,
    flag: &str,
) -> Result<(), pixiv_cli_rs::CommandError> {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    std::fs::write(&path, "").unwrap();
    let mut db = Database::open(directory.path()).unwrap();
    db.save_pixiv_credential(&PixivAccount::new(42, "fixture", b"fixture-refresh"))
        .unwrap();
    let context = Context::new();
    let transport = fixture::Fixture {
        failure: String::new(),
        context: context.clone(),
        requests: Arc::new(Mutex::new(vec![])),
    };
    let execution = Execution::new(Store::new(path), Arc::new(Mutex::new(db)), move |_| {
        Ok(transport.clone())
    });
    let active = Arc::new(AtomicBool::new(false));
    let called = Arc::new(AtomicBool::new(false));
    let marker = active.clone();
    let out = Arc::new(Mutex::new(ObservingWriter {
        active: active.clone(),
        called: called.clone(),
        path: directory.path().join("one-b011ca290fdf.png"),
        kind,
    }));
    let command = DownloadCommand::parse(
        &[
            "download".into(),
            "https://i.pximg.net/assets/one.png?signature=private".into(),
            flag.into(),
        ],
        &mut &b""[..],
        false,
    )
    .unwrap();
    let result = command
        .execute_with_factory(
            &context,
            || {
                Ok(DownloadRuntime {
                    download_path: directory.path().to_string_lossy().into_owned(),
                    ..Default::default()
                })
            },
            &mut &b""[..],
            DownloadSinks {
                output: out,
                error: Arc::new(Mutex::new(fixture::FailWriter)),
            },
            || Ok(execution),
            move |client| {
                marker.store(true, Ordering::SeqCst);
                Arc::new(ActiveClient {
                    inner: fixture::SaveClient(client),
                    active: marker.clone(),
                })
            },
        )
        .await;
    assert!(called.load(Ordering::SeqCst));
    assert!(!active.load(Ordering::SeqCst));
    result
}

#[tokio::test]
async fn owned_output_failure_occurs_before_the_resource_client_callback_closes() {
    let result = run_output_failure(std::io::ErrorKind::Other, "--json").await;
    assert_eq!(result.unwrap_err().to_string(), "fixture writer failure");
}

#[tokio::test]
async fn owned_ndjson_broken_pipe_keeps_the_output_variant_and_exits_quietly() {
    let result = run_output_failure(std::io::ErrorKind::BrokenPipe, "--ndjson").await;
    assert!(
        matches!(&result, Err(pixiv_cli_rs::CommandError::Output(error)) if error.kind() == std::io::ErrorKind::BrokenPipe)
    );
    let mut diagnostics = Vec::new();
    assert_eq!(
        pixiv_cli_rs::finish_command(result, true, false, &mut diagnostics),
        0
    );
    assert!(diagnostics.is_empty());
}
