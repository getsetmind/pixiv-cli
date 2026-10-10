use pixiv_app::{
    diagnostics::{Event, Scope},
    download::{
        AnimationEncoder, ArtworkFuture, ArtworkPageFuture, DownloadRequest, DownloadSaveClient,
        EncoderFuture, EncoderInput, NativeAnimationEncoder, SaveFuture, UgoiraMetadataFuture,
        download_sources_with_encoder,
    },
    lifecycle::Context,
    scheduler::SchedulerError,
};
use pixiv_sdk::{
    Error, Reason,
    cursor::{Cursor, Page},
    error::{Cause, RetryAdvice},
    models::{
        Artwork, ArtworkKind, ArtworkPage, ImageResource, UgoiraArchive, UgoiraFrame,
        UgoiraMetadata, User,
    },
    pixiv::{UserArtworkBookmarksRequest, UserArtworksRequest},
    resource::{Resource, ResourceRef, SavedResource},
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

// The frozen Go trace uses one worker; only this test thread is pinned so production worker selection stays unchanged.
#[cfg(target_os = "linux")]
struct CallingThreadAffinity(libc::cpu_set_t);
#[cfg(target_os = "linux")]
impl CallingThreadAffinity {
    fn using(mask: &libc::cpu_set_t) -> Self {
        unsafe {
            let mut previous: libc::cpu_set_t = std::mem::zeroed();
            assert_eq!(
                libc::sched_getaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &mut previous),
                0
            );
            assert_eq!(
                libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), mask),
                0
            );
            Self(previous)
        }
    }
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

#[cfg(target_os = "linux")]
struct GoNativeScheduling<'a>(&'a libc::cpu_set_t);
#[cfg(target_os = "linux")]
impl AnimationEncoder for GoNativeScheduling<'_> {
    fn encode(&self, context: Context, input: EncoderInput) -> EncoderFuture<'_> {
        Box::pin(async move {
            let _native_affinity = CallingThreadAffinity::using(self.0);
            NativeAnimationEncoder.encode(context, input).await
        })
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
struct ListInput {
    operation: &'static str,
    id: i64,
    kind: String,
    restrict: String,
    tag: String,
    cursor: Cursor,
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
struct ExpansionClient {
    root: String,
    case: Value,
    native: Value,
    calls: Mutex<Vec<Value>>,
    next_list: Mutex<usize>,
    causes: Mutex<Vec<Arc<SchedulerError>>>,
}
impl ExpansionClient {
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
    async fn list(
        &self,
        context: Context,
        input: ListInput,
    ) -> Result<Page<Artwork>, SchedulerError> {
        let ListInput {
            operation,
            id,
            kind,
            restrict,
            tag,
            cursor,
        } = input;
        let mut call = json!({"operation":operation,"id":id});
        for (key, value) in [
            ("kind", kind.as_str()),
            ("restrict", restrict.as_str()),
            ("tag", tag.as_str()),
            ("cursor", cursor.as_str()),
        ] {
            if !value.is_empty() {
                call[key] = json!(value);
            }
        }
        if let Some(error) = context.error() {
            call["context_error"] = json!(error.to_string());
        }
        self.record(call.clone());
        let mut next = self.next_list.lock().unwrap();
        let page = self.case["lists"]
            .as_array()
            .unwrap()
            .get(*next)
            .unwrap_or_else(|| panic!("unexpected list call {call}"));
        *next += 1;
        assert_eq!(page["operation"], operation);
        assert_eq!(page["user_id"], id);
        assert_eq!(page["kind"], kind);
        assert_eq!(page["cursor"], cursor.as_str());
        if operation == "user_bookmarks" {
            assert_eq!(restrict, "public");
            assert_eq!(tag, "");
        }
        if page["cancel"].as_bool().unwrap_or(false) {
            context.cancel();
        }
        self.outcome(operation, page["error"].as_str().unwrap_or(""))?;
        let next = page["next"].as_str().unwrap();
        Ok(Page {
            next: if next.is_empty() {
                Cursor::default()
            } else {
                Cursor::parse(next).unwrap()
            },
            items: page["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|input| {
                    empty_artwork(
                        input["id"].as_i64().unwrap(),
                        input["kind"].as_str().unwrap(),
                        input["title"].as_str().unwrap(),
                        User {
                            name: "list author must be discarded".into(),
                            ..Default::default()
                        },
                    )
                })
                .collect(),
        })
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
impl DownloadSaveClient for ExpansionClient {
    fn user_artworks(
        &self,
        context: Context,
        request: UserArtworksRequest,
    ) -> Option<ArtworkPageFuture<'_>> {
        Some(Box::pin(self.list(
            context,
            ListInput {
                operation: "user_artworks",
                id: request.user_id,
                kind: request.kind,
                restrict: String::new(),
                tag: String::new(),
                cursor: request.cursor,
            },
        )))
    }
    fn user_artwork_bookmarks(
        &self,
        context: Context,
        request: UserArtworkBookmarksRequest,
    ) -> Option<ArtworkPageFuture<'_>> {
        Some(Box::pin(self.list(
            context,
            ListInput {
                operation: "user_bookmarks",
                id: request.user_id,
                kind: String::new(),
                restrict: request.restrict,
                tag: request.tag,
                cursor: request.cursor,
            },
        )))
    }
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
            self.save(
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
            .await
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
fn manifest(client: &ExpansionClient, root: &Path) -> Vec<Value> {
    if !root.exists() {
        return vec![];
    }
    fn visit(client: &ExpansionClient, path: &Path, out: &mut Vec<Value>) {
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
#[tokio::test(flavor = "current_thread")]
async fn source_expansion_matches_frozen_go_service_and_real_manager() {
    #[cfg(target_os = "linux")]
    let _affinity = CallingThreadAffinity::single_cpu();
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/download_source_expansion.json")).unwrap();
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 52);
    let native: Value = serde_json::from_str(include_str!("fixtures/ugoira_encoder.json")).unwrap();
    let mut differences = vec![];
    for case in fixture["cases"].as_array().unwrap() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("download");
        let client = ExpansionClient {
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
            next_list: Mutex::new(0),
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
            directory_template: case["directory_template"].as_str().unwrap().into(),
            quality: case["quality"].as_str().unwrap().into(),
            ugoira_format: case["format"].as_str().unwrap().into(),
            pages: case["pages"]
                .as_array()
                .map(|pages| pages.iter().map(|p| p.as_i64().unwrap()).collect())
                .unwrap_or_default(),
        };
        let sources = case["sources"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        let attempt = download_sources_with_encoder(
            &context,
            &client,
            &sources,
            &request,
            &GoNativeScheduling(&_affinity.0),
        )
        .await;
        let items = attempt.report.items.iter().map(|i|json!({"id":i.illust_id,"title":i.title,"author":i.author,"kind":i.kind,"files":i.files.iter().map(|f|json!({"path":client.normalize(&f.path.to_string_lossy()),"page":f.page,"bytes":f.bytes})).collect::<Vec<_>>(),"quality":i.quality,"frames":if i.frames.is_empty(){Value::Null}else{json!(i.frames.iter().map(|f|json!({"file":f.filename,"delay":f.delay_milliseconds})).collect::<Vec<_>>())},"frame_report":i.frame_report.as_ref().map(|f|json!({"declared":f.declared,"actual":f.actual,"undeclared":f.undeclared,"missing":f.missing}))})).collect::<Vec<_>>();
        let failures = attempt.report.failures.iter().map(|f| {let classified=f.cause.classified();json!({"id":f.illust_id,"url":f.url,"kind":f.kind,"message":client.normalize(&f.message),"cause_kind":error_kind(&f.cause),"cause_reason":if error_kind(&f.cause)=="sdk" {classified.map(|e|e.code.as_str()).unwrap_or("")} else {""},"code":f.code,"path":client.normalize(&f.path.to_string_lossy()),"missing":if f.missing.is_empty(){Value::Null}else{json!(f.missing)},"cause_message":client.normalize(&f.cause.to_string()),"cause_product":classified.map(|e|e.product.as_str()).unwrap_or(""),"cause_operation":classified.map(|e|e.operation.as_str()).unwrap_or(""),"retry_safe":classified.is_some_and(|e|e.retry.safe),"retry_has_after":classified.is_some_and(|e|e.retry.after.is_some()),"retry_after":classified.and_then(|e|e.retry.after).map(|t|t.to_rfc3339_opts(chrono::SecondsFormat::Secs,true)).unwrap_or_default(),"original_cause":client.original(&f.cause)})}).collect::<Vec<_>>();
        let actual = json!({"calls":*client.calls.lock().unwrap(),"items":items,"failures":failures,"warnings":attempt.report.warnings.iter().map(|w|json!({"IllustID":w.illust_id,"Type":w.kind,"Message":w.message})).collect::<Vec<_>>(),"manifest":manifest(&client,&root),"diagnostics":*events.lock().unwrap(),"committed":attempt.report.committed,"error":attempt.error.as_ref().map(|e|client.normalize(&e.to_string())).unwrap_or_default(),"error_kind":attempt.error.as_ref().map(error_kind).unwrap_or(""),"error_reason":attempt.error.as_ref().filter(|e|error_kind(e)=="sdk").and_then(SchedulerError::classified).map(|e|e.code.as_str()).unwrap_or(""),"original_error":attempt.error.as_ref().is_some_and(|e|client.original(e))});
        let mut expected = case["expected"].clone();
        expected.as_object_mut().unwrap().remove("manager_requests");
        expected["calls"]
            .as_array_mut()
            .unwrap()
            .retain(|call| call["operation"] != "manager_factory");
        if actual != expected
            || *client.next_list.lock().unwrap() != case["lists"].as_array().unwrap().len()
        {
            differences.push(format!(
                "{}: actual={actual}, expected={expected}",
                case["name"]
            ));
        }
    }
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

#[tokio::test(flavor = "current_thread")]
async fn native_pending_list_cancellation_matches_frozen_sdk_transport_causes() {
    use pixiv_app::download::NativeDownloadSaveClient;
    use pixiv_sdk::transport::HttpTransport;
    use std::{
        error::Error as _,
        time::{Duration, Instant},
    };
    use tokio::{io::AsyncReadExt, net::TcpListener};
    let fixture: Value = serde_json::from_str(include_str!(
        "fixtures/download_source_native_cancellation.json"
    ))
    .unwrap();
    let mut differences = vec![];
    for case in fixture["cases"].as_array().unwrap() {
        let actual = tokio::time::timeout(Duration::from_secs(3), async {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let proxy = format!("http://{}",listener.local_addr().unwrap());
            let client = NativeDownloadSaveClient::new(Arc::new(pixiv_sdk::Client::with_transport("synthetic-token",HttpTransport::new(Some(&proxy)).unwrap())));
            let context = if case["mode"] == "deadline" {Context::with_deadline(Instant::now()+Duration::from_millis(500))} else {Context::new()};
            let mut future = match case["operation"].as_str().unwrap() {
                "UserArtworks" => client.user_artworks(context.clone(),UserArtworksRequest {user_id:7,kind:"illustration".into(),..Default::default()}).unwrap(),
                "UserArtworkBookmarks" => client.user_artwork_bookmarks(context.clone(),UserArtworkBookmarksRequest {user_id:7,restrict:"public".into(),..Default::default()}).unwrap(),
                other => panic!("unknown native fixture operation {other}"),
            };
            let (mut socket,_) = tokio::select! { result=&mut future=>panic!("native list completed before owned proxy CONNECT: {result:?}"), connection=listener.accept()=>connection.unwrap() };
            let mut bytes=vec![];
            loop {
                let mut buffer=[0u8;1024];
                let count = tokio::select! {result=&mut future=>panic!("native list completed while CONNECT was pending: {result:?}"), read=socket.read(&mut buffer)=>read.unwrap()};
                assert!(count>0,"proxy closed before CONNECT");bytes.extend_from_slice(&buffer[..count]);
                assert!(bytes.len()<8192,"unexpected proxy request size");
                if bytes.ends_with(b"\r\n\r\n") {break;}
            }
            let connect=String::from_utf8(bytes).unwrap();
            assert!(connect.starts_with("CONNECT app-api.pixiv.net:443 HTTP/1.1\r\n"),"{connect}");
            if case["mode"]=="cancel" {context.cancel();}
            let error=future.await.unwrap_err();
            drop(socket);drop(listener);
            let classified=error.classified();
            json!({"message":error.to_string(),"product":classified.map(|e|e.product.as_str()).unwrap_or(""),"operation":classified.map(|e|e.operation.as_str()).unwrap_or(""),"reason":classified.map(|e|e.code.as_str()).unwrap_or(""),"transport":classified.and_then(|e|e.transport).map(|t|serde_json::to_value(t).unwrap()).unwrap_or(json!("")),"http_status":classified.and_then(|e|e.http_status).unwrap_or(0),"retry_safe":classified.is_some_and(|e|e.retry.safe),"retry_has_after":classified.is_some_and(|e|e.retry.after.is_some()),"cause_message":classified.and_then(|e|e.source()).map(ToString::to_string).unwrap_or_default(),"canceled":error.is_canceled(),"deadline":error.is_deadline_exceeded(),"parent_error":context.error().map(|e|e.to_string()).unwrap_or_default()})
        }).await.expect("owned native CONNECT cancellation timed out");
        let mut expected = case["expected"].clone();
        expected.as_object_mut().unwrap().remove("method");
        expected.as_object_mut().unwrap().remove("url");
        if actual != expected {
            differences.push(format!(
                "{} {}: actual={actual}, expected={expected}",
                case["operation"], case["mode"]
            ));
        }
    }
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}
