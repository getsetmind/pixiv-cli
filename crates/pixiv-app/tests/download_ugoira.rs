use pixiv_app::{
    download::{
        AnimationEncoder, ArtworkFuture, DownloadRequest, DownloadSaveClient, EncoderFuture,
        EncoderInput, SaveFuture, UgoiraMetadataFuture, download_sources_with_encoder,
    },
    lifecycle::Context,
    scheduler::SchedulerError,
};
use pixiv_sdk::{
    Error, Reason,
    models::{
        Artwork, ArtworkKind, ArtworkPage, ImageResource, UgoiraArchive, UgoiraFrame,
        UgoiraMetadata, User,
    },
    resource::{Resource, ResourceRef, SavedResource},
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

struct WorkflowClient {
    root: String,
    case: Value,
    calls: Mutex<Vec<Value>>,
    temp_paths: Mutex<Vec<(PathBuf, String)>>,
}
impl WorkflowClient {
    fn normalize(&self, path: &str) -> String {
        path.replace(&self.root, "${ROOT}").replace('\\', "/")
    }
    fn record(&self, call: Value) {
        self.calls.lock().unwrap().push(call);
    }
    fn zip_bytes(&self) -> Vec<u8> {
        if let Some(hex) = self.case["zip_hex"].as_str() {
            return hex
                .as_bytes()
                .chunks_exact(2)
                .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                .collect();
        }
        if self.case["corrupt"].as_bool().unwrap() {
            b"synthetic corrupt zip".to_vec()
        } else {
            synthetic_zip(self.case["entries"].as_array().unwrap())
        }
    }
}
impl DownloadSaveClient for WorkflowClient {
    fn artwork(&self, _context: Context, id: i64) -> Option<ArtworkFuture<'_>> {
        Some(Box::pin(async move {
            self.record(json!({"kind":"artwork","id":id}));
            let mut artwork = Artwork {
                id,
                title: if id == 7 {
                    "static".into()
                } else {
                    self.case["title"].as_str().unwrap().into()
                },
                kind: if id == 7 {
                    ArtworkKind::Illust
                } else {
                    ArtworkKind::Ugoira
                },
                user: User {
                    id: if id == 7 { 0 } else { 3 },
                    name: "author".into(),
                    ..Default::default()
                },
                page_count: 1,
                caption: String::new(),
                raw_kind: String::new(),
                tags: vec![],
                published_at: "0001-01-01T00:00:00Z".parse().unwrap(),
                updated_at: None,
                total_bookmarks: 0,
                total_views: 0,
                width: 0,
                height: 0,
                x_restrict: 0,
                ai_type: 0,
                tools: vec![],
                cover: ImageResource::default(),
                pages: vec![],
            };
            if id == 7 {
                artwork.pages = vec![ArtworkPage {
                    image: ImageResource {
                        resource: Resource {
                            reference: ResourceRef::new(
                                "pixiv",
                                br#"{"k":"artwork","id":7,"p":0}"#,
                            )
                            .unwrap(),
                            url: "https://i.pximg.net/7.jpg".into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    page_index: 0,
                    width: 0,
                    height: 0,
                }]
            };
            Ok(artwork)
        }))
    }
    fn ugoira_metadata(&self, _context: Context, id: i64) -> Option<UgoiraMetadataFuture<'_>> {
        Some(Box::pin(async move {
            self.record(json!({"kind":"metadata","id":id}));
            let error = self.case["metadata_error"].as_str().unwrap();
            if !error.is_empty() {
                return Err(SchedulerError::Message(error.into()));
            }
            Ok(UgoiraMetadata {
                artwork_id: id,
                archives: self.case["archives"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|a| UgoiraArchive {
                        quality: a["quality"].as_str().unwrap().into(),
                        resource: Resource {
                            reference: ResourceRef::new(
                                "pixiv",
                                a["payload"].as_str().unwrap().as_bytes(),
                            )
                            .unwrap(),
                            url: a["url"].as_str().unwrap().into(),
                            ..Default::default()
                        },
                    })
                    .collect(),
                frames: self.case["frames"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|f| UgoiraFrame {
                        filename: f["file"].as_str().unwrap().into(),
                        delay_milliseconds: f["delay"].as_i64().unwrap(),
                    })
                    .collect(),
            })
        }))
    }
    fn save_ref(&self, context: Context, reference: ResourceRef, path: PathBuf) -> SaveFuture<'_> {
        Box::pin(async move {
            let name = path.file_name().unwrap().to_string_lossy();
            let archive = name.starts_with("ugoira-") && name.ends_with(".zip");
            let normalized = if archive {
                let n =
                    self.normalize(&path.with_file_name("ugoira-${TEMP}.zip").to_string_lossy());
                self.temp_paths
                    .lock()
                    .unwrap()
                    .push((path.clone(), n.clone()));
                n
            } else {
                self.normalize(&path.to_string_lossy())
            };
            let mut call = json!({"kind":"ref","source":reference.to_string(),"payload":String::from_utf8(reference.payload().unwrap()).unwrap(),"path":normalized});
            if path.parent().unwrap().is_dir() {
                call["parent_exists"] = json!(true)
            };
            self.record(call);
            let body = if archive {
                self.zip_bytes()
            } else {
                b"static-or-direct".to_vec()
            };
            if archive {
                let error = self.case["save_error"].as_str().unwrap();
                match error {
                    "" => {}
                    "cancel" => {
                        context.cancel();
                        return Err(SchedulerError::Canceled);
                    }
                    "cancel_after_write" => {
                        std::fs::write(&path, &body).unwrap();
                        context.cancel();
                        return Err(SchedulerError::Canceled);
                    }
                    "context_error_only" => return Err(SchedulerError::Canceled),
                    "deadline" => return Err(SchedulerError::DeadlineExceeded),
                    "typed" => {
                        return Err(Error::new(Reason::ResourceForbidden, "SaveResource").into());
                    }
                    _ => return Err(SchedulerError::Message(error.into())),
                }
            }
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &body).unwrap();
            Ok(SavedResource {
                path: path.to_string_lossy().into_owned(),
                size: body.len() as i64,
                content_type: if archive {
                    "application/zip"
                } else {
                    "image/jpeg"
                }
                .into(),
            })
        })
    }
    fn save_url(&self, _context: Context, url: String, path: PathBuf) -> SaveFuture<'_> {
        Box::pin(async move {
            self.record(
                json!({"kind":"url","source":url,"path":self.normalize(&path.to_string_lossy())}),
            );
            let body = b"direct-url";
            std::fs::write(&path, body).unwrap();
            Ok(SavedResource {
                path: path.to_string_lossy().into_owned(),
                size: body.len() as i64,
                content_type: "image/jpeg".into(),
            })
        })
    }
}
struct EncoderPort<'a>(&'a WorkflowClient);
impl AnimationEncoder for EncoderPort<'_> {
    fn encode(&self, context: Context, input: EncoderInput) -> EncoderFuture<'_> {
        Box::pin(async move {
            let c = self.0;
            let zip_path = c
                .temp_paths
                .lock()
                .unwrap()
                .iter()
                .find(|(p, _)| p == &input.zip_path)
                .unwrap()
                .1
                .clone();
            let mut call = json!({"kind":"encoder_port","path":c.normalize(&input.output_path.to_string_lossy()),"zip_path":zip_path,"work_dir":c.normalize(&input.work_dir.to_string_lossy()),"format":input.format,"zip_hex":encode_hex(&std::fs::read(&input.zip_path).unwrap())});
            if let Some(frames) = input.frames.filter(|f| !f.is_empty()) {
                call["frames"] = json!(
                    frames
                        .iter()
                        .map(|f| json!({"file":f.filename,"delay":f.delay_milliseconds}))
                        .collect::<Vec<_>>()
                )
            };
            c.record(call);
            match c.case["encoder_error"].as_str().unwrap() {
                "" => {}
                "cancel" => {
                    context.cancel();
                    return Err(SchedulerError::Canceled);
                }
                "context_error_only" => return Err(SchedulerError::Canceled),
                "deadline" => return Err(SchedulerError::DeadlineExceeded),
                e => return Err(SchedulerError::Message(e.into())),
            };
            std::fs::write(
                input.output_path,
                format!("injected-encoder-port:{}", input.format),
            )
            .unwrap();
            Ok(())
        })
    }
}
fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn crc32(bytes: &[u8]) -> u32 {
    let mut c = !0_u32;
    for b in bytes {
        c ^= *b as u32;
        for _ in 0..8 {
            c = (c >> 1) ^ if c & 1 != 0 { 0xedb88320 } else { 0 }
        }
    }
    !c
}
fn synthetic_zip(entries: &[Value]) -> Vec<u8> {
    fn u16(out: &mut Vec<u8>, n: u16) {
        out.extend(n.to_le_bytes())
    }
    fn u32(out: &mut Vec<u8>, n: u32) {
        out.extend(n.to_le_bytes())
    }
    let mut out = vec![];
    let mut central = vec![];
    for entry in entries {
        let name = entry.as_str().unwrap();
        let dir = name.ends_with('/');
        let body = if dir {
            vec![]
        } else {
            format!("synthetic-frame:{name}").into_bytes()
        };
        let offset = out.len() as u32;
        let utf8 = name
            .chars()
            .any(|c| !('\u{20}'..='}').contains(&c) || c == '\\');
        let flags = (if dir { 0 } else { 8 }) | (if utf8 { 0x800 } else { 0 });
        let crc = crc32(&body);
        let size = body.len() as u32;
        u32(&mut out, 0x04034b50);
        for n in [20, flags, 0, 0, 0] {
            u16(&mut out, n)
        }
        for _ in 0..3 {
            u32(&mut out, 0)
        }
        u16(&mut out, name.len() as u16);
        u16(&mut out, 0);
        out.extend(name.as_bytes());
        out.extend(&body);
        if !dir {
            u32(&mut out, 0x08074b50);
            u32(&mut out, crc);
            u32(&mut out, size);
            u32(&mut out, size)
        }
        u32(&mut central, 0x02014b50);
        for n in [20, 20, flags, 0, 0, 0] {
            u16(&mut central, n)
        }
        u32(&mut central, crc);
        u32(&mut central, size);
        u32(&mut central, size);
        u16(&mut central, name.len() as u16);
        for _ in 0..4 {
            u16(&mut central, 0)
        }
        u32(&mut central, 0);
        u32(&mut central, offset);
        central.extend(name.as_bytes());
    }
    let offset = out.len() as u32;
    let size = central.len() as u32;
    out.extend(central);
    u32(&mut out, 0x06054b50);
    u16(&mut out, 0);
    u16(&mut out, 0);
    u16(&mut out, entries.len() as u16);
    u16(&mut out, entries.len() as u16);
    u32(&mut out, size);
    u32(&mut out, offset);
    u16(&mut out, 0);
    out
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
fn manifest(client: &WorkflowClient, root: &Path) -> Vec<Value> {
    fn walk(c: &WorkflowClient, p: &Path, out: &mut Vec<Value>) {
        if !p.exists() {
            return;
        }
        let mut e = json!({"path":c.normalize(&p.to_string_lossy()),"directory":p.is_dir()});
        if p.is_file() {
            e["body_hex"] = json!(encode_hex(&std::fs::read(p).unwrap()))
        };
        out.push(e);
        if p.is_dir() {
            let mut paths = std::fs::read_dir(p)
                .unwrap()
                .map(|e| e.unwrap().path())
                .collect::<Vec<_>>();
            paths.sort();
            for p in paths {
                walk(c, &p, out)
            }
        }
    }
    let mut out = vec![];
    walk(client, root, &mut out);
    out
}
#[tokio::test]
async fn frozen_go_ugoira_manager_workflow_contract() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/download_ugoira.json")).unwrap();
    assert_eq!(
        fixture["reference"],
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 70);
    let mut differences = vec![];
    for case in fixture["cases"].as_array().unwrap() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("download");
        let client = WorkflowClient {
            root: root.to_string_lossy().into_owned(),
            case: case.clone(),
            calls: Mutex::new(vec![]),
            temp_paths: Mutex::new(vec![]),
        };
        for (name, body) in case["existing"].as_object().unwrap() {
            let p = root.join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body.as_str().unwrap()).unwrap()
        }
        let context = Context::new();
        if case["cancel_before"].as_bool().unwrap() {
            context.cancel()
        };
        let directory = case["directory_template"].as_str().unwrap();
        let request = DownloadRequest {
            download_path: client.root.clone(),
            filename_template: case["filename_template"].as_str().unwrap().into(),
            directory_template: if directory.is_empty() {
                case["default_directory_template"].as_str().unwrap()
            } else {
                directory
            }
            .into(),
            quality: case["quality"].as_str().unwrap().into(),
            ugoira_format: case["format"].as_str().unwrap().into(),
            pages: case["pages"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p.as_i64().unwrap())
                .collect(),
        };
        let sources = case["sources"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap().into())
            .collect::<Vec<_>>();
        let attempt = download_sources_with_encoder(
            &context,
            &client,
            &sources,
            &request,
            &EncoderPort(&client),
        )
        .await;
        let items=attempt.report.items.iter().map(|i|json!({"id":i.illust_id,"title":i.title,"author":i.author,"kind":i.kind,"files":i.files.iter().map(|f|json!({"path":client.normalize(&f.path.to_string_lossy()),"page":f.page,"bytes":f.bytes})).collect::<Vec<_>>(),"quality":i.quality,"frames":if i.frames.is_empty(){Value::Null}else{json!(i.frames.iter().map(|f|json!({"file":f.filename,"delay":f.delay_milliseconds})).collect::<Vec<_>>())},"frame_report":i.frame_report.as_ref().map(|r|json!({"declared":r.declared,"actual":r.actual,"undeclared":json!(r.undeclared),"missing":json!(r.missing)}))})).collect::<Vec<_>>();
        let failures=attempt.report.failures.iter().map(|f|json!({"id":f.illust_id,"url":f.url,"kind":f.kind,"message":client.normalize(&f.message),"cause_kind":error_kind(&f.cause),"cause_reason":f.cause.classified().map(|e|e.code.as_str()).unwrap_or(""),"code":f.code,"path":client.normalize(&f.path.to_string_lossy()),"missing":if f.missing.is_empty(){Value::Null}else{json!(f.missing)}})).collect::<Vec<_>>();
        let warnings = attempt
            .report
            .warnings
            .iter()
            .map(|w| json!({"IllustID":w.illust_id,"Type":w.kind,"Message":w.message}))
            .collect::<Vec<_>>();
        let actual = json!({"calls":*client.calls.lock().unwrap(),"items":items,"failures":failures,"warnings":warnings,"manifest":manifest(&client,&root),"committed":attempt.report.committed,"error":attempt.error.as_ref().map(|e|client.normalize(&e.to_string())).unwrap_or_default(),"error_kind":attempt.error.as_ref().map(error_kind).unwrap_or("")});
        if actual != case["expected"] {
            differences.push(format!(
                "{}: actual={actual}, expected={}",
                case["name"], case["expected"]
            ))
        }
    }
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

#[test]
fn synthetic_transport_zip_bytes_match_frozen_go_writer() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/download_ugoira.json")).unwrap();
    let mut compared = 0;
    for case in fixture["cases"].as_array().unwrap() {
        if case["corrupt"].as_bool().unwrap() {
            continue;
        }
        let produced = encode_hex(&synthetic_zip(case["entries"].as_array().unwrap()));
        for call in case["expected"]["calls"].as_array().unwrap() {
            if let Some(body) = call["zip_hex"].as_str() {
                assert_eq!(produced, body, "{}", case["name"]);
                compared += 1;
            }
        }
        for entry in case["expected"]["manifest"].as_array().unwrap() {
            if let Some(body) = entry["body_hex"].as_str().filter(|b| b.starts_with("504b")) {
                assert_eq!(produced, body, "{}", case["name"]);
                compared += 1;
            }
        }
    }
    assert!(compared > 40);
}

#[tokio::test]
async fn native_ugoira_metadata_adapter_honors_context_without_network() {
    let client = pixiv_app::download::NativeDownloadSaveClient::new(std::sync::Arc::new(
        pixiv_sdk::Client::new("", None).unwrap(),
    ));
    let context = Context::new();
    context.cancel();
    let error = client
        .ugoira_metadata(context, 42)
        .unwrap()
        .await
        .unwrap_err();
    assert!(error.is_canceled());
}

#[tokio::test]
async fn frozen_go_raw_zip_inspection_runs_through_public_manager() {
    let structure: Value =
        serde_json::from_str(include_str!("fixtures/ugoira_zip_directory.json")).unwrap();
    let workflows: Value =
        serde_json::from_str(include_str!("fixtures/download_ugoira.json")).unwrap();
    for zip_case in structure["cases"].as_array().unwrap() {
        let mut case = workflows["cases"][0].clone();
        case["format"] = json!("zip");
        case["zip_hex"] = zip_case["zip_hex"].clone();
        case["frames"] = json!(
            zip_case["declared"]
                .as_array()
                .unwrap()
                .iter()
                .map(|name| json!({"file":name,"delay":10}))
                .collect::<Vec<_>>()
        );
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("download");
        let client = WorkflowClient {
            root: root.to_string_lossy().into_owned(),
            case,
            calls: Mutex::new(vec![]),
            temp_paths: Mutex::new(vec![]),
        };
        let attempt = download_sources_with_encoder(
            &Context::new(),
            &client,
            &["42".into()],
            &DownloadRequest {
                download_path: client.root.clone(),
                filename_template: "{id}".into(),
                ugoira_format: "zip".into(),
                ..Default::default()
            },
            &EncoderPort(&client),
        )
        .await;
        let expected = &zip_case["expected"];
        let name = zip_case["name"].as_str().unwrap();
        if expected["error"].as_str().unwrap().is_empty() {
            assert!(attempt.error.is_none(), "{name}: {:?}", attempt.error);
            assert!(attempt.report.failures.is_empty(), "{name}");
            assert_eq!(attempt.report.items.len(), 1, "{name}");
            assert!(attempt.report.committed, "{name}");
            let report = attempt.report.items[0].frame_report.as_ref().unwrap();
            assert_eq!(
                json!({"declared":report.declared,"actual":report.actual,"undeclared":report.undeclared,"missing":report.missing}),
                expected["report"],
                "{name}"
            );
            assert_eq!(
                encode_hex(&std::fs::read(&attempt.report.items[0].files[0].path).unwrap()),
                zip_case["zip_hex"],
                "{name}"
            );
        } else {
            assert!(attempt.report.items.is_empty(), "{name}");
            assert!(!attempt.report.committed, "{name}");
            assert_eq!(attempt.report.failures.len(), 1, "{name}");
            let failure = &attempt.report.failures[0];
            assert_eq!(failure.code, expected["code"].as_str().unwrap(), "{name}");
            assert_eq!(
                failure.message,
                expected["error"].as_str().unwrap(),
                "{name}"
            );
            let missing = expected["report"]["missing"]
                .as_array()
                .map(|names| {
                    names
                        .iter()
                        .map(|name| name.as_str().unwrap().to_owned())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            assert_eq!(failure.missing, missing, "{name}");
            assert_eq!(failure.path, root.join(".quarantine/42.zip"), "{name}");
            assert_eq!(
                encode_hex(&std::fs::read(&failure.path).unwrap()),
                zip_case["zip_hex"],
                "{name}"
            );
        }
        assert!(
            !client
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|call| call["kind"] == "encoder_port"),
            "{name}"
        );
        for (path, _) in client.temp_paths.lock().unwrap().iter() {
            assert!(!path.exists(), "{name}: staging archive leaked");
        }
    }
}

#[test]
fn frozen_ugoira_manager_and_zip_go_sources_are_unchanged() {
    use sha2::{Digest, Sha256};
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for contents in [
        include_str!("fixtures/download_ugoira.json"),
        include_str!("fixtures/ugoira_zip_directory.json"),
    ] {
        let fixture: Value = serde_json::from_str(contents).unwrap();
        assert_eq!(
            fixture["reference"],
            "4b4426487ef18bed276706daec385e0d0a6979f9"
        );
        for (path, expected) in fixture["source_sha256"].as_object().unwrap() {
            let bytes = std::fs::read(root.join(path)).unwrap();
            assert_eq!(
                format!("{:x}", Sha256::digest(bytes)),
                expected.as_str().unwrap(),
                "{path}"
            );
        }
    }
}
