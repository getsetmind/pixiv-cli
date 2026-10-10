use pixiv_app::{
    diagnostics::{Event, Scope},
    download::{DownloadRequest, DownloadSaveClient, download_sources},
    lifecycle::Context,
    scheduler::SchedulerError,
};
use pixiv_sdk::{
    Error, Reason,
    resource::{ResourceRef, SavedResource},
};
use serde_json::{Value, json};
use std::{
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

struct SaveClient {
    root: String,
    relative_root: String,
    outcomes: Vec<String>,
    unsupported: bool,
    calls: Mutex<Vec<Value>>,
}
impl SaveClient {
    fn normalize(&self, path: &str) -> String {
        path.replace(&self.root, "${ROOT}")
            .replace(&self.relative_root, "${REL}")
            .replace('\\', "/")
    }
    async fn save(
        &self,
        context: Context,
        kind: &str,
        source: String,
        path: PathBuf,
    ) -> Result<SavedResource, SchedulerError> {
        let index = {
            let mut calls = self.calls.lock().unwrap();
            calls.push(
                json!({"kind":kind,"source":source,"path":self.normalize(&path.to_string_lossy())}),
            );
            calls.len() - 1
        };
        match self.outcomes.get(index).map(String::as_str).unwrap_or("ok") {
            "typed" => return Err(Error::new(Reason::ResourceForbidden, "SaveResource").into()),
            "signed_error" => {
                return Err(SchedulerError::Message(format!(
                    "GET {source} failed: signature=secret token=hidden HTTP/1.1 attempt 1"
                )));
            }
            "cancel" => {
                context.cancel();
                return Err(SchedulerError::Canceled);
            }
            "deadline" => {
                return Err(context.cancelled().await.into());
            }
            "context_error_only" => return Err(SchedulerError::Canceled),
            _ => {}
        }
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"synthetic-image").unwrap();
        Ok(SavedResource {
            path: path.to_string_lossy().into_owned(),
            size: 15,
            content_type: "image/png".into(),
        })
    }
}
impl DownloadSaveClient for SaveClient {
    fn supports_direct_urls(&self) -> bool {
        !self.unsupported
    }
    fn save_ref(
        &self,
        context: Context,
        reference: ResourceRef,
        path: PathBuf,
    ) -> Pin<Box<dyn Future<Output = Result<SavedResource, SchedulerError>> + Send + '_>> {
        Box::pin(self.save(context, "ref", reference.to_string(), path))
    }
    fn save_url(
        &self,
        context: Context,
        url: String,
        path: PathBuf,
    ) -> Pin<Box<dyn Future<Output = Result<SavedResource, SchedulerError>> + Send + '_>> {
        Box::pin(self.save(context, "url", url, path))
    }
}
fn error_kind(error: &SchedulerError) -> &str {
    if error.is_canceled() {
        "canceled"
    } else if error.is_deadline_exceeded() {
        "deadline"
    } else if error.classified().is_some() {
        "sdk"
    } else {
        "plain"
    }
}

#[tokio::test]
async fn frozen_go_direct_source_contract() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/download_direct_sources.json")).unwrap();
    assert_eq!(
        fixture["reference"],
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    let mut differences = vec![];
    for case in fixture["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let cwd = std::env::current_dir().unwrap();
        let root = tempfile::tempdir_in(&cwd).unwrap();
        let root_string = root.path().to_string_lossy().into_owned();
        let relative_root = root
            .path()
            .strip_prefix(&cwd)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let client = SaveClient {
            root: root_string.clone(),
            relative_root: relative_root.clone(),
            outcomes: case["outcomes"]
                .as_array()
                .map(|a| a.iter().map(|v| v.as_str().unwrap().to_owned()).collect())
                .unwrap_or_default(),
            unsupported: case["unsupported_url_client"].as_bool().unwrap_or(false),
            calls: Mutex::new(vec![]),
        };
        let events = Arc::new(Mutex::new(vec![]));
        let context = if client.outcomes.iter().any(|v| v == "deadline") {
            Context::with_deadline(Instant::now() + Duration::from_millis(30))
        } else {
            Context::new()
        };
        let context = if case["capture_diagnostics"].as_bool().unwrap_or(false) {
            let events = Arc::clone(&events);
            context.with_scope(Scope::new(Some(Arc::new(move |event:Event| {
                assert!(event.duration_ns>=0);
                events.lock().unwrap().push(json!({"module":event.module,"kind":event.kind,"operation":event.operation,"count":event.count,"reason":event.reason}));
            })),"Pixiv CLI",0))
        } else {
            context
        };
        if case["cancel_before"].as_bool().unwrap_or(false) {
            context.cancel();
        }
        let mut request = DownloadRequest {
            download_path: case["download_path"]
                .as_str()
                .unwrap()
                .replace("${ROOT}", &root_string)
                .replace("${REL}", &relative_root),
            ..Default::default()
        };
        if case["ignore_artwork_options"].as_bool().unwrap_or(false) {
            request.filename_template = "{unsupported}".into();
            request.directory_template = "../unsafe".into();
            request.pages = vec![-1];
            request.quality = "invalid".into();
            request.ugoira_format = "invalid".into();
        }
        let sources = case["sources"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        let attempt = download_sources(&context, &client, &sources, &request).await;
        let items=attempt.report.items.iter().flat_map(|item|item.files.iter().map(|file|json!({
            "illust_id":item.illust_id,"title":item.title,"author":item.author,"type":item.kind,
            "path":client.normalize(&file.path.to_string_lossy()),"page":file.page,"bytes":file.bytes,
            "body":std::fs::read_to_string(&file.path).unwrap()
        }))).collect::<Vec<_>>();
        let failures=attempt.report.failures.iter().map(|f|json!({
            "url":f.url,"type":f.kind,"message":f.message,"cause_message":f.cause.to_string(),
            "cause_kind":error_kind(&f.cause),"cause_reason":f.cause.classified().map(|e|e.code.as_str()).unwrap_or(""),"code":f.code
        })).collect::<Vec<_>>();
        let mut got = json!({"calls":*client.calls.lock().unwrap(),"items":items,"failures":failures,"warning_count":attempt.report.warnings.len(),"committed":attempt.report.committed,
            "error":attempt.error.as_ref().map(ToString::to_string).unwrap_or_default(),
            "error_kind":attempt.error.as_ref().map(error_kind).unwrap_or("")});
        if case["capture_diagnostics"].as_bool().unwrap_or(false) {
            got["diagnostics"] = json!(*events.lock().unwrap());
        }
        if got != case["expected"] {
            differences.push(format!(
                "{name}: actual={got}, expected={}",
                case["expected"]
            ));
        }
    }
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

#[tokio::test]
async fn save_only_clients_keep_explicit_artwork_and_list_capability_failures() {
    for source in [
        "42",
        "https://www.pixiv.net/artworks/42",
        "https://www.pixiv.net/users/42",
        "https://www.pixiv.net/users/42/bookmarks/artworks",
    ] {
        let root = tempfile::tempdir().unwrap();
        let client = SaveClient {
            root: root.path().to_string_lossy().into_owned(),
            relative_root: String::from("unused-relative-root"),
            outcomes: vec![],
            unsupported: false,
            calls: Mutex::new(vec![]),
        };
        let request = DownloadRequest {
            download_path: client.root.clone(),
            ..Default::default()
        };
        let attempt = download_sources(&Context::new(), &client, &[source.into()], &request).await;
        let expected = if source.ends_with("/bookmarks/artworks") {
            "download client does not support user artwork bookmark listing"
        } else if source.contains("/users/") {
            "download client does not support user artwork listing"
        } else {
            "artwork download is not yet supported"
        };
        assert_eq!(attempt.error.unwrap().to_string(), expected, "{source}");
        assert!(attempt.report.failures.is_empty(), "{source}");
        assert!(attempt.report.items.is_empty(), "{source}");
        assert!(!attempt.report.committed, "{source}");
        assert!(client.calls.lock().unwrap().is_empty(), "{source}");
    }
}

#[tokio::test]
async fn native_adapter_preserves_sdk_policy_failure_without_network() {
    use pixiv_app::download::NativeDownloadSaveClient;
    use std::sync::Arc;
    let root = tempfile::tempdir().unwrap();
    let sdk = Arc::new(pixiv_sdk::Client::new("", None).unwrap());
    let client = NativeDownloadSaveClient::new(sdk);
    let source = "https://forbidden.invalid/photo.png?signature=secret";
    let request = DownloadRequest {
        download_path: root.path().to_string_lossy().into_owned(),
        ..Default::default()
    };
    let attempt = download_sources(&Context::new(), &client, &[source.into()], &request).await;
    assert!(attempt.error.is_none());
    assert_eq!(attempt.report.failures.len(), 1);
    let failure = &attempt.report.failures[0];
    assert_eq!(failure.url, "[redacted source]");
    assert_eq!(
        failure.cause.classified().unwrap().code,
        Reason::ResourceForbidden
    );
    assert!(!failure.message.contains("secret"));
    assert!(attempt.report.items.is_empty());
    assert!(!attempt.report.committed);
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn native_adapter_checks_cancellation_before_sdk_save() {
    use pixiv_app::download::NativeDownloadSaveClient;
    use std::sync::Arc;
    let root = tempfile::tempdir().unwrap();
    let client = NativeDownloadSaveClient::new(Arc::new(pixiv_sdk::Client::new("", None).unwrap()));
    let context = Context::new();
    context.cancel();
    let reference = ResourceRef::new("pixiv", br#"{"k":"artwork","id":42,"p":0}"#).unwrap();
    assert!(
        client
            .save_ref(context.clone(), reference, root.path().join("ref"))
            .await
            .unwrap_err()
            .is_canceled()
    );
    assert!(
        client
            .save_url(
                context,
                String::from("https://i.pximg.net/photo.png"),
                root.path().join("url")
            )
            .await
            .unwrap_err()
            .is_canceled()
    );
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}
