#![cfg(target_os = "linux")]

use pixiv_app::{
    diagnostics::{Event, Scope},
    download::{
        ArtworkFuture, DownloadRequest, DownloadSaveClient, SaveFuture, UgoiraMetadataFuture,
        download_artworks,
    },
    lifecycle::Context,
    scheduler::SchedulerError,
};
use pixiv_sdk::{
    Error, Reason,
    error::{Cause, RetryAdvice},
    models::{
        Artwork, ArtworkKind, ArtworkPage, ImageResource, UgoiraArchive, UgoiraFrame,
        UgoiraMetadata, User,
    },
    resource::{Resource, ResourceRef, SavedResource},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

// The frozen Go trace uses one worker; only this test thread is pinned so production worker selection stays unchanged.
#[cfg(target_os = "linux")]
struct CallingThreadAffinity(libc::cpu_set_t);
#[cfg(target_os = "linux")]
impl CallingThreadAffinity {
    fn single_cpu() -> Self {
        unsafe {
            let mut original: libc::cpu_set_t = std::mem::zeroed();
            assert_eq!(
                libc::sched_getaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &mut original),
                0
            );
            let cpu = (0..libc::CPU_SETSIZE as usize)
                .find(|cpu| libc::CPU_ISSET(*cpu, &original))
                .unwrap();
            let mut selected: libc::cpu_set_t = std::mem::zeroed();
            libc::CPU_ZERO(&mut selected);
            libc::CPU_SET(cpu, &mut selected);
            assert_eq!(
                libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &selected),
                0
            );
            assert_eq!(std::thread::available_parallelism().unwrap().get(), 1);
            Self(original)
        }
    }
}
#[cfg(target_os = "linux")]
impl Drop for CallingThreadAffinity {
    fn drop(&mut self) {
        assert_eq!(
            unsafe { libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &self.0) },
            0
        );
    }
}

fn hex_decode(raw: &str) -> Vec<u8> {
    raw.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}
fn hex_encode(raw: &[u8]) -> String {
    raw.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn empty_artwork(id: i64, kind: &str, title: &str, user: User) -> Artwork {
    Artwork {
        id,
        kind: match kind {
            "illustration" => ArtworkKind::Illust,
            "manga" => ArtworkKind::Manga,
            "ugoira" => ArtworkKind::Ugoira,
            _ => ArtworkKind::Unknown,
        },
        title: title.into(),
        user,
        caption: String::new(),
        raw_kind: kind.into(),
        tags: vec![],
        published_at: "0001-01-01T00:00:00Z".parse().unwrap(),
        updated_at: None,
        total_bookmarks: 0,
        total_views: 0,
        width: 0,
        height: 0,
        page_count: 0,
        x_restrict: 0,
        ai_type: 0,
        tools: vec![],
        cover: ImageResource::default(),
        pages: vec![],
    }
}
fn error_kind(error: &SchedulerError) -> &'static str {
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
struct SaveInput {
    operation: &'static str,
    source: String,
    payload: String,
    path: PathBuf,
    body: Vec<u8>,
    mime: String,
    outcome: String,
}
struct ArtworkBatchClient {
    root: String,
    case: Value,
    native: Value,
    calls: Mutex<Vec<Value>>,
    original_affinity: libc::cpu_set_t,
    causes: Mutex<Vec<Arc<SchedulerError>>>,
}
impl ArtworkBatchClient {
    fn normalize(&self, raw: &str) -> String {
        let normalized = raw.replace(&self.root, "${ROOT}").replace('\\', "/");
        let path = Path::new(&normalized);
        if path.file_name().is_some_and(|name| {
            name.to_string_lossy().starts_with("ugoira-")
                && name.to_string_lossy().ends_with(".zip")
        }) {
            path.with_file_name("ugoira-${TEMP}.zip")
                .to_string_lossy()
                .into_owned()
        } else {
            normalized
        }
    }
    fn record(&self, value: Value) {
        self.calls.lock().unwrap().push(value);
    }
    fn outcome(&self, operation: &str, outcome: &str) -> Result<(), SchedulerError> {
        let error = match outcome {
            "" => return Ok(()),
            "business" => SchedulerError::Message(format!("synthetic {operation} failure")),
            "rate" => Error::new(Reason::RateLimited, operation)
                .with_http_status(429)
                .with_retry(RetryAdvice {
                    safe: true,
                    after: Some("2026-10-11T01:02:03Z".parse().unwrap()),
                })
                .into(),
            "forbidden" => Error::new(Reason::ResourceForbidden, operation).into(),
            "context_error_only" => SchedulerError::Canceled,
            "wrapped_context_error_only" => Error::new(Reason::UpstreamUnavailable, operation)
                .with_cause(Cause::Canceled)
                .into(),
            "deadline_error_only" => SchedulerError::DeadlineExceeded,
            other => panic!("unknown fixture outcome {other}"),
        };
        let error = Arc::new(error);
        self.causes.lock().unwrap().push(Arc::clone(&error));
        Err(SchedulerError::Shared(error))
    }
    fn original(&self, error: &SchedulerError) -> bool {
        let causes = self.causes.lock().unwrap();
        match error {
            SchedulerError::Shared(inner) => causes.iter().any(|cause| Arc::ptr_eq(inner, cause)),
            SchedulerError::Canceled => causes
                .iter()
                .any(|cause| matches!(cause.as_ref(), SchedulerError::Canceled)),
            SchedulerError::DeadlineExceeded => causes
                .iter()
                .any(|cause| matches!(cause.as_ref(), SchedulerError::DeadlineExceeded)),
            _ => false,
        }
    }
    async fn detail(&self, id: i64) -> Result<Artwork, SchedulerError> {
        self.record(json!({"operation":"artwork","id":id}));
        let input = self.case["artworks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|input| input["id"].as_i64() == Some(id))
            .unwrap();
        self.outcome("Artwork", input["error"].as_str().unwrap_or(""))?;
        let mut art = empty_artwork(
            id,
            input["kind"].as_str().unwrap(),
            input["title"].as_str().unwrap(),
            User {
                id: input["author_id"].as_i64().unwrap(),
                name: input["author"].as_str().unwrap().into(),
                ..Default::default()
            },
        );
        art.raw_kind = input["raw_kind"].as_str().unwrap_or("").into();
        art.page_count = input["page_count"].as_i64().unwrap();
        art.pages = input["pages"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(index, page)| {
                let payload = page["payload"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("{{\"k\":\"artwork\",\"id\":{id},\"p\":{index}}}"));
                ArtworkPage {
                    page_index: page["page_index"].as_u64().unwrap() as usize,
                    image: ImageResource {
                        resource: Resource {
                            reference: ResourceRef::new("pixiv", payload.as_bytes()).unwrap(),
                            url: page["url"].as_str().unwrap().into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    width: 0,
                    height: 0,
                }
            })
            .collect();
        Ok(art)
    }
    async fn save(
        &self,
        context: Context,
        input: SaveInput,
    ) -> Result<SavedResource, SchedulerError> {
        let SaveInput {
            operation,
            source,
            payload,
            path,
            body,
            mime,
            outcome,
        } = input;
        let mut call = json!({"operation":operation,"source":source,"path":self.normalize(&path.to_string_lossy())});
        if !payload.is_empty() {
            call["payload"] = json!(payload);
        }
        if path.parent().unwrap().is_dir() {
            call["parent_exists"] = json!(true);
        }
        if let Some(error) = context.error() {
            call["context_error"] = json!(error.to_string());
        }
        self.record(call);
        self.outcome("SaveResource", &outcome)?;
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &body).unwrap();
        Ok(SavedResource {
            path: path.to_string_lossy().into_owned(),
            size: body.len() as i64,
            content_type: mime,
        })
    }
}
impl DownloadSaveClient for ArtworkBatchClient {
    fn artwork(&self, _context: Context, id: i64) -> Option<ArtworkFuture<'_>> {
        Some(Box::pin(self.detail(id)))
    }
    fn ugoira_metadata(&self, _context: Context, id: i64) -> Option<UgoiraMetadataFuture<'_>> {
        Some(Box::pin(async move {
            self.record(json!({"operation":"ugoira_metadata","id":id}));
            Ok(UgoiraMetadata {
                artwork_id: id,
                archives: vec![UgoiraArchive {
                    quality: "original".into(),
                    resource: Resource {
                        reference: ResourceRef::new(
                            "pixiv",
                            format!("{{\"k\":\"ugoira\",\"id\":{id},\"q\":\"original\"}}")
                                .as_bytes(),
                        )
                        .unwrap(),
                        url: format!("https://i.pximg.net/{id}.zip"),
                        ..Default::default()
                    },
                }],
                frames: self.native["frames"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|frame| UgoiraFrame {
                        filename: frame["file"].as_str().unwrap().into(),
                        delay_milliseconds: frame["delay"].as_i64().unwrap(),
                    })
                    .collect(),
            })
        }))
    }
    fn save_ref(&self, context: Context, reference: ResourceRef, path: PathBuf) -> SaveFuture<'_> {
        Box::pin(async move {
            let payload = String::from_utf8(reference.payload().unwrap()).unwrap();
            let identity: Value = serde_json::from_str(&payload).unwrap();
            let mut body = b"direct-ref".to_vec();
            let mut mime = "image/jpeg".to_owned();
            let mut outcome = "";
            if identity["k"] == "ugoira" {
                body = hex_decode(self.native["zip_hex"].as_str().unwrap());
                mime = "application/zip".into();
            } else if let Some(art) = self.case["artworks"]
                .as_array()
                .unwrap()
                .iter()
                .find(|input| input["id"] == identity["id"])
                && let Some(page) = art["pages"]
                    .as_array()
                    .unwrap()
                    .get(identity["p"].as_u64().unwrap_or(0) as usize)
            {
                body = hex_decode(page["body_hex"].as_str().unwrap());
                mime = page["mime"].as_str().unwrap().into();
                outcome = page["error"].as_str().unwrap_or("");
            }
            let saved = self
                .save(
                    context,
                    SaveInput {
                        operation: "save_ref",
                        source: reference.to_string(),
                        payload,
                        path,
                        body,
                        mime,
                        outcome: outcome.into(),
                    },
                )
                .await;
            if identity["k"] == "ugoira" {
                // Native encoding inherits the original scheduling context used by Go, while batch trace remains single-worker.
                assert_eq!(
                    unsafe {
                        libc::sched_setaffinity(
                            0,
                            std::mem::size_of::<libc::cpu_set_t>(),
                            &self.original_affinity,
                        )
                    },
                    0
                );
            }
            saved
        })
    }
    fn save_url(&self, context: Context, url: String, path: PathBuf) -> SaveFuture<'_> {
        Box::pin(self.save(
            context,
            SaveInput {
                operation: "save_url",
                source: url,
                payload: String::new(),
                path,
                body: b"direct-url".to_vec(),
                mime: "image/jpeg".into(),
                outcome: String::new(),
            },
        ))
    }
}
fn manifest(client: &ArtworkBatchClient, root: &Path) -> Vec<Value> {
    if !root.exists() {
        return vec![];
    }
    fn visit(client: &ArtworkBatchClient, path: &Path, out: &mut Vec<Value>) {
        let mut entry =
            json!({"path":client.normalize(&path.to_string_lossy()),"directory":path.is_dir()});
        if path.is_file() {
            entry["body_hex"] = json!(hex_encode(&std::fs::read(path).unwrap()));
        }
        out.push(entry);
        if path.is_dir() {
            let mut children = std::fs::read_dir(path)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect::<Vec<_>>();
            children.sort();
            for child in children {
                visit(client, &child, out);
            }
        }
    }
    let mut out = vec![];
    visit(client, root, &mut out);
    out
}
#[cfg(target_os = "linux")]
#[test]
fn artwork_batch_matches_frozen_go_service_and_real_manager() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .thread_keep_alive(std::time::Duration::from_secs(300))
        .build()
        .unwrap();
    runtime.block_on(compare_frozen_artwork_batch());
}

async fn compare_frozen_artwork_batch() {
    tokio::task::spawn_blocking(|| ()).await.unwrap();
    let fixture_bytes = include_bytes!("fixtures/download_artwork_batch.json");
    assert_eq!(
        format!("{:x}", Sha256::digest(fixture_bytes)),
        "17e318d103427713ac335dc0255c5a28fa205f7bacf1c5dcbb2cbb64ec4eabfe"
    );
    let fixture: Value = serde_json::from_slice(fixture_bytes).unwrap();
    assert_eq!(
        fixture["reference"],
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 27);
    let native_bytes = include_bytes!("fixtures/ugoira_encoder.json");
    assert_eq!(
        format!("{:x}", Sha256::digest(native_bytes)),
        "88df4a179e7f5790953ac7507407bb9faa277bd736e16f5757928c63a303d662"
    );
    let native: Value = serde_json::from_slice(native_bytes).unwrap();
    let mut differences = vec![];
    for case in fixture["cases"].as_array().unwrap() {
        let affinity = CallingThreadAffinity::single_cpu();
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("download");
        let client = ArtworkBatchClient {
            root: root.to_string_lossy().into_owned(),
            case: case.clone(),
            native: native["cases"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["name"] == "gif")
                .unwrap()
                .clone(),
            calls: Mutex::new(vec![]),
            original_affinity: affinity.0,
            causes: Mutex::new(vec![]),
        };
        let events = Arc::new(Mutex::new(vec![]));
        let captured = Arc::clone(&events);
        let context = Context::new().with_scope(Scope::new(Some(Arc::new(move |e:Event| {assert!(e.duration_ns>=0);captured.lock().unwrap().push(json!({"module":e.module,"kind":e.kind,"operation":e.operation,"count":e.count,"reason":e.reason}));})),"Pixiv CLI",0));
        if case["cancel_before"].as_bool().unwrap_or(false) {
            context.cancel();
        }
        let request = DownloadRequest {
            download_path: client.root.clone(),
            filename_template: case["filename_template"].as_str().unwrap().into(),
            directory_template: case["directory_template"]
                .as_str()
                .filter(|value| !value.trim().is_empty())
                .or_else(|| case["default_directory_template"].as_str())
                .unwrap_or("")
                .into(),
            quality: case["quality"].as_str().unwrap().into(),
            ugoira_format: case["format"].as_str().unwrap().into(),
            pages: case["pages"]
                .as_array()
                .map(|pages| pages.iter().map(|p| p.as_i64().unwrap()).collect())
                .unwrap_or_default(),
        };
        let ids = case["ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|id| id.as_i64().unwrap())
            .collect::<Vec<_>>();
        let attempt = download_artworks(&context, &client, &ids, &request).await;
        let items = attempt.report.items.iter().map(|i|json!({"id":i.illust_id,"title":i.title,"author":i.author,"kind":i.kind,"files":i.files.iter().map(|f|json!({"path":client.normalize(&f.path.to_string_lossy()),"page":f.page,"bytes":f.bytes})).collect::<Vec<_>>(),"quality":i.quality,"frames":if i.frames.is_empty(){Value::Null}else{json!(i.frames.iter().map(|f|json!({"file":f.filename,"delay":f.delay_milliseconds})).collect::<Vec<_>>())},"frame_report":i.frame_report.as_ref().map(|f|json!({"declared":f.declared,"actual":f.actual,"undeclared":f.undeclared,"missing":f.missing}))})).collect::<Vec<_>>();
        let failures = attempt.report.failures.iter().map(|f| {let classified=f.cause.classified();json!({"id":f.illust_id,"url":f.url,"kind":f.kind,"message":client.normalize(&f.message),"cause_kind":error_kind(&f.cause),"cause_reason":if error_kind(&f.cause)=="sdk" {classified.map(|e|e.code.as_str()).unwrap_or("")} else {""},"code":f.code,"path":client.normalize(&f.path.to_string_lossy()),"missing":if f.missing.is_empty(){Value::Null}else{json!(f.missing)},"cause_message":client.normalize(&f.cause.to_string()),"cause_product":classified.map(|e|e.product.as_str()).unwrap_or(""),"cause_operation":classified.map(|e|e.operation.as_str()).unwrap_or(""),"retry_safe":classified.is_some_and(|e|e.retry.safe),"retry_has_after":classified.is_some_and(|e|e.retry.after.is_some()),"retry_after":classified.and_then(|e|e.retry.after).map(|t|t.to_rfc3339_opts(chrono::SecondsFormat::Secs,true)).unwrap_or_default(),"original_cause":client.original(&f.cause)})}).collect::<Vec<_>>();
        let actual = json!({"calls":*client.calls.lock().unwrap(),"items":items,"failures":failures,"warnings":attempt.report.warnings.iter().map(|w|json!({"IllustID":w.illust_id,"Type":w.kind,"Message":w.message})).collect::<Vec<_>>(),"manifest":manifest(&client,&root),"diagnostics":*events.lock().unwrap(),"committed":attempt.report.committed,"error":attempt.error.as_ref().map(|e|client.normalize(&e.to_string())).unwrap_or_default(),"error_kind":attempt.error.as_ref().map(error_kind).unwrap_or(""),"error_reason":attempt.error.as_ref().filter(|e|error_kind(e)=="sdk").and_then(SchedulerError::classified).map(|e|e.code.as_str()).unwrap_or(""),"original_error":attempt.error.as_ref().is_some_and(|e|client.original(e))});
        let mut expected = case["expected"].clone();
        expected
            .as_object_mut()
            .unwrap()
            .remove("go_factory_observations");
        if actual != expected {
            differences.push(format!(
                "{}: actual={actual}, expected={expected}",
                case["name"]
            ));
        }
    }
    assert!(
        differences.is_empty(),
        "{} runtime differences:\n{}",
        differences.len(),
        differences.join("\n")
    );
}
