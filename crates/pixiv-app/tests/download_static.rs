use pixiv_app::{
    download::{ArtworkFuture, DownloadRequest, DownloadSaveClient, SaveFuture, download_sources},
    lifecycle::Context,
    scheduler::SchedulerError,
};
use pixiv_sdk::{
    Error, Reason,
    models::{Artwork, ArtworkKind, ArtworkPage, ImageResource, Tag, User},
    resource::{Resource, ResourceRef, SavedResource},
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

struct StaticClient {
    root: String,
    artworks: Vec<Value>,
    calls: Mutex<Vec<Value>>,
}
impl StaticClient {
    fn normalize(&self, path: &str) -> String {
        path.replace(&self.root, "${ROOT}").replace('\\', "/")
    }
    async fn metadata(&self, id: i64) -> Result<Artwork, SchedulerError> {
        self.calls
            .lock()
            .unwrap()
            .push(json!({"kind":"artwork","id":id}));
        let input = self
            .artworks
            .iter()
            .find(|a| a["id"].as_i64() == Some(id))
            .unwrap();
        match input["error"].as_str().unwrap_or("") {
            "business" => return Err(SchedulerError::Message("synthetic metadata failure".into())),
            "typed" => return Err(Error::new(Reason::RateLimited, "Artwork").into()),
            _ => {}
        }
        let pages = input["pages"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(index, p)| {
                let payload = p["payload"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("{{\"k\":\"artwork\",\"id\":{id},\"p\":{index}}}"));
                let reference =
                    ResourceRef::new(p["product"].as_str().unwrap_or("pixiv"), payload.as_bytes())
                        .unwrap();
                ArtworkPage {
                    page_index: p["page_index"].as_i64().unwrap() as usize,
                    image: ImageResource {
                        resource: Resource {
                            reference,
                            url: p["url"].as_str().unwrap().into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    width: 0,
                    height: 0,
                }
            })
            .collect();
        let published = input["published_at"].as_str().unwrap();
        Ok(Artwork {
            id,
            title: input["title"].as_str().unwrap().into(),
            caption: String::new(),
            kind: match input["kind"].as_str().unwrap() {
                "illustration" => ArtworkKind::Illust,
                "manga" => ArtworkKind::Manga,
                "ugoira" => ArtworkKind::Ugoira,
                _ => ArtworkKind::Unknown,
            },
            raw_kind: input["raw_kind"].as_str().unwrap_or("").into(),
            tags: input["tags"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| Tag {
                    name: v.as_str().unwrap().into(),
                    translated_name: String::new(),
                })
                .collect(),
            user: User {
                id: input["author_id"].as_i64().unwrap(),
                name: input["author"].as_str().unwrap().into(),
                ..Default::default()
            },
            published_at: if published.is_empty() {
                "0001-01-01T00:00:00Z"
            } else {
                published
            }
            .parse()
            .unwrap(),
            updated_at: None,
            total_bookmarks: 0,
            total_views: 0,
            width: 0,
            height: 0,
            page_count: input["page_count"].as_i64().unwrap(),
            x_restrict: 0,
            ai_type: 0,
            tools: vec![],
            cover: ImageResource::default(),
            pages,
        })
    }
    async fn save(
        &self,
        context: Context,
        kind: &str,
        source: String,
        payload: String,
        path: PathBuf,
        page: Value,
    ) -> Result<SavedResource, SchedulerError> {
        let mut call =
            json!({"kind":kind,"source":source,"path":self.normalize(&path.to_string_lossy())});
        if !payload.is_empty() {
            call["payload"] = json!(payload);
        }
        if path.parent().unwrap().is_dir() {
            call["parent_exists"] = json!(true);
        }
        self.calls.lock().unwrap().push(call);
        match page["error"].as_str().unwrap_or("") {
            "business" => return Err(SchedulerError::Message("synthetic page failure".into())),
            "typed" => return Err(Error::new(Reason::ResourceForbidden, "SaveResource").into()),
            "cancel" => {
                context.cancel();
                return Err(SchedulerError::Canceled);
            }
            "context_error_only" => return Err(SchedulerError::Canceled),
            "deadline" => return Err(SchedulerError::DeadlineExceeded),
            _ => {}
        }
        let body = decode_hex(page["body_hex"].as_str().unwrap());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &body).unwrap();
        Ok(SavedResource {
            path: path.to_string_lossy().into_owned(),
            size: body.len() as i64,
            content_type: page["mime"].as_str().unwrap().into(),
        })
    }
}
impl DownloadSaveClient for StaticClient {
    fn artwork(&self, _context: Context, id: i64) -> Option<ArtworkFuture<'_>> {
        Some(Box::pin(self.metadata(id)))
    }
    fn save_ref(&self, context: Context, reference: ResourceRef, path: PathBuf) -> SaveFuture<'_> {
        Box::pin(async move {
            let payload = String::from_utf8(reference.payload().unwrap()).unwrap();
            let identity: Value = serde_json::from_str(&payload).unwrap();
            let page = self
                .artworks
                .iter()
                .find(|a| a["id"] == identity["id"])
                .and_then(|a| {
                    a["pages"]
                        .as_array()
                        .unwrap()
                        .get(identity["p"].as_u64().unwrap_or(0) as usize)
                })
                .cloned()
                .unwrap_or_else(
                    || json!({"body_hex":encode_hex(b"direct-ref"),"mime":"image/jpeg"}),
                );
            self.save(context, "ref", reference.to_string(), payload, path, page)
                .await
        })
    }
    fn save_url(&self, context: Context, url: String, path: PathBuf) -> SaveFuture<'_> {
        Box::pin(self.save(
            context,
            "url",
            url,
            String::new(),
            path,
            json!({"body_hex":encode_hex(b"direct-url"),"mime":"image/jpeg"}),
        ))
    }
}
fn decode_hex(text: &str) -> Vec<u8> {
    text.as_bytes()
        .chunks_exact(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect()
}
fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
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
fn manifest(client: &StaticClient, root: &Path) -> Vec<Value> {
    fn visit(client: &StaticClient, path: &Path, out: &mut Vec<Value>) {
        if !path.exists() {
            return;
        }
        let mut entry =
            json!({"path":client.normalize(&path.to_string_lossy()),"directory":path.is_dir()});
        if path.is_file() {
            entry["body_hex"] = json!(encode_hex(&std::fs::read(path).unwrap()));
        }
        out.push(entry);
        if path.is_dir() {
            let mut paths = std::fs::read_dir(path)
                .unwrap()
                .map(|e| e.unwrap().path())
                .collect::<Vec<_>>();
            paths.sort();
            for p in paths {
                visit(client, &p, out);
            }
        }
    }
    let mut out = vec![];
    visit(client, root, &mut out);
    out
}

#[tokio::test]
async fn frozen_go_static_artwork_manager_contract() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/download_static.json")).unwrap();
    let mut differences = vec![];
    for case in fixture["cases"].as_array().unwrap() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("download");
        let client = StaticClient {
            root: root.to_string_lossy().into_owned(),
            artworks: case["artworks"].as_array().unwrap().clone(),
            calls: Mutex::new(vec![]),
        };
        if let Some(existing) = case["existing"].as_object() {
            for (name, body) in existing {
                let path = root.join(name);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, body.as_str().unwrap()).unwrap();
            }
        }
        let context = Context::new();
        if case["cancel_before"].as_bool().unwrap_or(false) {
            context.cancel();
        }
        let request = DownloadRequest {
            download_path: client.root.clone(),
            filename_template: case["filename_template"].as_str().unwrap().into(),
            directory_template: case["directory_template"]
                .as_str()
                .filter(|s| !s.is_empty())
                .or_else(|| case["default_directory_template"].as_str())
                .unwrap_or("")
                .into(),
            quality: case["quality"].as_str().unwrap().into(),
            pages: case["pages"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_i64().unwrap())
                .collect(),
            ..Default::default()
        };
        let sources = case["sources"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        let attempt = download_sources(&context, &client, &sources, &request).await;
        let items=attempt.report.items.iter().map(|i|json!({"id":i.illust_id,"title":i.title,"author":i.author,"kind":i.kind,"files":i.files.iter().map(|f|json!({"path":client.normalize(&f.path.to_string_lossy()),"page":f.page,"bytes":f.bytes})).collect::<Vec<_>>()})).collect::<Vec<_>>();
        let failures=attempt.report.failures.iter().map(|f|json!({"id":f.illust_id,"url":f.url,"kind":f.kind,"message":if f.message.starts_with("publish detected image extension: link ") && f.message.ends_with(": file exists") {"publish detected image extension: destination exists".to_owned()}else{client.normalize(&f.message)},"cause_kind":error_kind(&f.cause),"cause_reason":f.cause.classified().map(|e|e.code.as_str()).unwrap_or(""),"code":f.code})).collect::<Vec<_>>();
        let actual = json!({"calls":*client.calls.lock().unwrap(),"items":items,"failures":failures,"manifest":manifest(&client,&root),"committed":attempt.report.committed,"warning_count":attempt.report.warnings.len(),"error":attempt.error.as_ref().map(|e|client.normalize(&e.to_string())).unwrap_or_default(),"error_kind":attempt.error.as_ref().map(error_kind).unwrap_or("")});
        if actual != case["expected"] {
            differences.push(format!(
                "{}: actual={actual}, expected={}",
                case["name"], case["expected"]
            ));
        }
    }
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

struct ReversedClient {
    inner: StaticClient,
    release: tokio::sync::Semaphore,
    completions: Mutex<Vec<i64>>,
    concurrent: bool,
}
impl DownloadSaveClient for ReversedClient {
    fn artwork(&self, _context: Context, id: i64) -> Option<ArtworkFuture<'_>> {
        Some(Box::pin(async move {
            if id == 42 && self.concurrent {
                self.release.acquire().await.unwrap().forget();
            }
            self.inner.metadata(id).await
        }))
    }
    fn save_ref(&self, context: Context, reference: ResourceRef, path: PathBuf) -> SaveFuture<'_> {
        Box::pin(async move {
            let payload: Value = serde_json::from_slice(&reference.payload().unwrap()).unwrap();
            let id = payload["id"].as_i64().unwrap();
            let saved = self.inner.save_ref(context, reference, path).await?;
            self.completions.lock().unwrap().push(id);
            if id == 99 {
                self.release.add_permits(1);
            }
            Ok(saved)
        })
    }
    fn save_url(&self, context: Context, url: String, path: PathBuf) -> SaveFuture<'_> {
        self.inner.save_url(context, url, path)
    }
}
#[tokio::test]
async fn sorted_report_survives_reversed_worker_completion() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/download_static.json")).unwrap();
    let mut input = fixture["cases"][0]["artworks"][0].clone();
    input["id"] = json!(99);
    let root = tempfile::tempdir().unwrap();
    let client = ReversedClient {
        inner: StaticClient {
            root: root.path().to_string_lossy().into_owned(),
            artworks: vec![fixture["cases"][0]["artworks"][0].clone(), input],
            calls: Mutex::new(vec![]),
        },
        release: tokio::sync::Semaphore::new(0),
        completions: Mutex::new(vec![]),
        concurrent: std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1)
            >= 2,
    };
    let request = DownloadRequest {
        download_path: client.inner.root.clone(),
        filename_template: "{id}".into(),
        ..Default::default()
    };
    let attempt = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        download_sources(
            &Context::new(),
            &client,
            &["99".into(), "42".into(), "99".into()],
            &request,
        ),
    )
    .await
    .unwrap();
    assert!(attempt.error.is_none());
    assert!(attempt.report.failures.is_empty());
    assert_eq!(
        attempt
            .report
            .items
            .iter()
            .map(|i| i.illust_id)
            .collect::<Vec<_>>(),
        vec![42, 99]
    );
    assert_eq!(
        *client.completions.lock().unwrap(),
        if client.concurrent {
            vec![99, 42]
        } else {
            vec![42, 99]
        }
    );
}

struct BoundedClient {
    inner: StaticClient,
    gate: tokio::sync::Semaphore,
    active: std::sync::atomic::AtomicUsize,
    peak: std::sync::atomic::AtomicUsize,
    initial: std::sync::atomic::AtomicUsize,
    limit: usize,
}
impl DownloadSaveClient for BoundedClient {
    fn artwork(&self, _context: Context, id: i64) -> Option<ArtworkFuture<'_>> {
        use std::sync::atomic::Ordering;
        Some(Box::pin(async move {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(active, Ordering::SeqCst);
            if self.initial.fetch_add(1, Ordering::SeqCst) < self.limit {
                if active == self.limit {
                    self.gate.add_permits(self.limit);
                }
                self.gate.acquire().await.unwrap().forget();
            }
            let metadata = self.inner.metadata(id).await;
            self.active.fetch_sub(1, Ordering::SeqCst);
            metadata
        }))
    }
    fn save_ref(&self, context: Context, reference: ResourceRef, path: PathBuf) -> SaveFuture<'_> {
        self.inner.save_ref(context, reference, path)
    }
    fn save_url(&self, context: Context, url: String, path: PathBuf) -> SaveFuture<'_> {
        self.inner.save_url(context, url, path)
    }
}
#[tokio::test]
async fn artwork_workers_are_bounded_by_available_parallelism() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/download_static.json")).unwrap();
    let limit = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1);
    let count = limit + 1;
    let artworks = (1..=count)
        .map(|id| {
            let mut input = fixture["cases"][0]["artworks"][0].clone();
            input["id"] = json!(id);
            input
        })
        .collect();
    let root = tempfile::tempdir().unwrap();
    let client = BoundedClient {
        inner: StaticClient {
            root: root.path().to_string_lossy().into_owned(),
            artworks,
            calls: Mutex::new(vec![]),
        },
        gate: tokio::sync::Semaphore::new(0),
        active: std::sync::atomic::AtomicUsize::new(0),
        peak: std::sync::atomic::AtomicUsize::new(0),
        initial: std::sync::atomic::AtomicUsize::new(0),
        limit,
    };
    let request = DownloadRequest {
        download_path: client.inner.root.clone(),
        filename_template: "{id}".into(),
        ..Default::default()
    };
    let sources = (1..=count)
        .rev()
        .map(|id| id.to_string())
        .collect::<Vec<_>>();
    let attempt = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        download_sources(&Context::new(), &client, &sources, &request),
    )
    .await
    .unwrap();
    assert!(attempt.error.is_none());
    assert!(attempt.report.failures.is_empty());
    assert_eq!(attempt.report.items.len(), count);
    assert_eq!(client.peak.load(std::sync::atomic::Ordering::SeqCst), limit);
    assert_eq!(client.active.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn native_metadata_adapter_honors_context_without_network() {
    use pixiv_app::download::NativeDownloadSaveClient;
    let client = NativeDownloadSaveClient::new(std::sync::Arc::new(
        pixiv_sdk::Client::new("", None).unwrap(),
    ));
    let context = Context::new();
    context.cancel();
    let error = client
        .artwork(context, 42)
        .expect("native metadata capability")
        .await
        .unwrap_err();
    assert!(error.is_canceled());
}

#[tokio::test]
async fn metadata_only_client_keeps_visible_ugoira_capability_gap() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/download_static.json")).unwrap();
    let mut ugoira = fixture["cases"][0]["artworks"][0].clone();
    ugoira["kind"] = json!("ugoira");
    let mut later = fixture["cases"][0]["artworks"][0].clone();
    later["id"] = json!(99);
    let root = tempfile::tempdir().unwrap();
    let client = StaticClient {
        root: root.path().to_string_lossy().into_owned(),
        artworks: vec![ugoira, later],
        calls: Mutex::new(vec![]),
    };
    let request = DownloadRequest {
        download_path: client.root.clone(),
        filename_template: "{id}".into(),
        ..Default::default()
    };
    let attempt = download_sources(
        &Context::new(),
        &client,
        &["42".into(), "99".into()],
        &request,
    )
    .await;
    assert!(attempt.error.is_none());
    assert_eq!(attempt.report.failures.len(), 1);
    assert_eq!(attempt.report.failures[0].illust_id, 42);
    assert_eq!(
        attempt.report.failures[0].message,
        "ugoira download is not yet supported"
    );
    assert_eq!(attempt.report.items.len(), 1);
    assert_eq!(attempt.report.items[0].illust_id, 99);
    assert!(
        client
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call["kind"] == "artwork" && call["id"] == 42)
    );
    assert!(!client.calls.lock().unwrap().iter().any(|call| {
        call["kind"] == "ref"
            && call["payload"]
                .as_str()
                .is_some_and(|payload| payload.contains("\"id\":42"))
    }));
}
