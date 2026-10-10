use super::*;
use pixiv_app::reverse_search::{
    Loader, RedirectDecision, RedirectHook, SourceLoader, SourceLoaderOptions,
};
use pixiv_sdk::fanbox::transport::{
    BodyFuture, ExternalError, RawBody, RawRead, RawRequest, RawResponse, RawTransport,
    TransportFuture,
};
use std::{collections::BTreeMap, sync::Mutex};

#[derive(Default)]
struct BodyCounts {
    reads: usize,
    closes: usize,
}
struct Body {
    bytes: Vec<u8>,
    mode: String,
    counts: Arc<Mutex<BodyCounts>>,
    cancel: Arc<Context>,
}
impl RawBody for Body {
    fn read<'a>(&'a mut self, output: &'a mut [u8]) -> BodyFuture<'a, RawRead> {
        Box::pin(async move {
            self.counts.lock().unwrap().reads += 1;
            if !self.bytes.is_empty() {
                let count = output.len().min(self.bytes.len());
                output[..count].copy_from_slice(&self.bytes[..count]);
                self.bytes.drain(..count);
                match self.mode.as_str() {
                    "cancel-with-bytes" => {
                        self.cancel.cancel();
                        return RawRead {
                            count,
                            eof: true,
                            error: None,
                        };
                    }
                    "cancel-between" => self.cancel.cancel(),
                    "error-with-bytes" => {
                        return RawRead {
                            count,
                            eof: false,
                            error: Some(Box::new(std::io::Error::other("owned body read error"))),
                        };
                    }
                    _ => {}
                }
                return RawRead {
                    count,
                    eof: false,
                    error: None,
                };
            }
            RawRead {
                count: 0,
                eof: self.mode != "read-error",
                error: (self.mode == "read-error").then(|| {
                    Box::new(std::io::Error::other("owned body read error")) as ExternalError
                }),
            }
        })
    }
    fn close(&mut self) -> BodyFuture<'_, std::result::Result<(), ExternalError>> {
        self.counts.lock().unwrap().closes += 1;
        Box::pin(async { Ok(()) })
    }
}
struct Transport {
    input: Value,
    requests: Arc<Mutex<Vec<Value>>>,
    bodies: Arc<Mutex<Vec<Arc<Mutex<BodyCounts>>>>>,
    context: Arc<Context>,
    caller_context: CallerContext,
    redirect: bool,
}
impl RawTransport for Transport {
    fn send(
        &self,
        request: RawRequest,
    ) -> TransportFuture<'_, std::result::Result<Option<RawResponse>, ExternalError>> {
        Box::pin(async move {
            assert!(Arc::ptr_eq(&request.context, &self.caller_context));
            self.requests
                .lock()
                .unwrap()
                .push(json!({"method":request.method,"url":request.url,"headers":request.headers}));
            let state = string(&self.input, "state");
            if state == "transport-cancel" {
                self.context.cancel();
            }
            if matches!(state, "transport-error" | "transport-cancel") {
                return Err(
                    Box::new(std::io::Error::other("owned transport error")) as ExternalError
                );
            }
            let requests = self.requests.lock().unwrap().len();
            let redirecting =
                self.redirect && requests <= self.input["redirects"].as_u64().unwrap() as usize;
            let status = if redirecting {
                302
            } else {
                self.input["status"]
                    .as_u64()
                    .filter(|status| *status != 0)
                    .unwrap_or(200) as u16
            };
            let mut headers = BTreeMap::new();
            if redirecting {
                let next = string(&self.input, "next_url");
                headers.insert(
                    "Location".to_owned(),
                    vec![if next.is_empty() {
                        format!("http://source.test/hop/{requests}")
                    } else {
                        next.to_owned()
                    }],
                );
            }
            let counts = Arc::new(Mutex::new(BodyCounts::default()));
            self.bodies.lock().unwrap().push(counts.clone());
            Ok(Some(RawResponse {
                status,
                headers,
                content_length: 0,
                body: Some(Box::new(Body {
                    bytes: if redirecting {
                        b"owned redirect response".to_vec()
                    } else {
                        bytes(string(&self.input, "payload_hex"))
                    },
                    mode: string(&self.input, "body_mode").to_owned(),
                    counts,
                    cancel: self.context.clone(),
                })),
            }))
        })
    }
}
struct Hook {
    mode: String,
    observations: Arc<Mutex<Vec<Value>>>,
    failure: Error,
}
impl RedirectHook for Hook {
    fn check(&self, next: &str, via: &[String]) -> std::result::Result<RedirectDecision, Error> {
        self.observations
            .lock()
            .unwrap()
            .push(json!({"next_url":next,"via":via}));
        if self.mode == "failure" {
            Err(self.failure.clone())
        } else {
            Ok(RedirectDecision::Follow)
        }
    }
}
fn transport(
    row: &Row,
    caller: CallerContext,
    cancel: Arc<Context>,
    redirect: bool,
) -> Arc<Transport> {
    Arc::new(Transport {
        input: row.input.clone(),
        requests: Arc::new(Mutex::new(Vec::new())),
        bodies: Arc::new(Mutex::new(Vec::new())),
        context: cancel,
        caller_context: caller,
        redirect,
    })
}
pub async fn load(row: &Row) {
    let root = tempfile::tempdir().unwrap();
    let snapshots = root.path().join("snapshots");
    std::fs::create_dir(&snapshots).unwrap();
    let input = &row.input;
    let mut source = string(input, "source").to_owned();
    if source == "file" {
        let path = root.path().join("source.bin");
        match string(input, "state") {
            "missing" => {}
            "directory" => std::fs::create_dir(&path).unwrap(),
            "symlink" => {
                let target = root.path().join("target.bin");
                std::fs::write(&target, bytes(string(input, "payload_hex"))).unwrap();
                std::os::unix::fs::symlink(target, &path).unwrap();
            }
            _ => std::fs::write(&path, bytes(string(input, "payload_hex"))).unwrap(),
        }
        source = path.to_str().unwrap().to_owned();
    }
    let temp = if string(input, "state") == "bad-temp" {
        root.path().join("missing/snapshots")
    } else {
        snapshots.clone()
    };
    let (cancel, caller) = context(string(input, "context"));
    let raw = transport(row, caller.clone(), cancel, false);
    let loader = Loader::new(SourceLoaderOptions {
        temp_dir: temp,
        http_transport: Some(raw.clone()),
        redirect_hook: None,
    });
    let loaded = loader.load(caller, &source).await;
    let mut out = json!({"error":error(loaded.as_ref().err()),"requests":*raw.requests.lock().unwrap(),"body_reads":0,"body_closes":0,"snapshot":null,"temporary_files_after":0});
    if let Ok(snapshot) = loaded {
        let files: Vec<_> = std::fs::read_dir(&snapshots)
            .unwrap()
            .map(|entry| entry.unwrap())
            .collect();
        assert_eq!(files.len(), 1);
        use std::os::unix::fs::PermissionsExt;
        let mode = format!(
            "{:04o}",
            files[0].metadata().unwrap().permissions().mode() & 0o777
        );
        if string(input, "source") == "file" {
            std::fs::write(&source, b"owned-changed").unwrap();
            out["source_bytes_before_snapshot_reads_hex"] =
                json!(hex(&std::fs::read(&source).unwrap()));
        }
        let reads = vec![read_snapshot(&snapshot), read_snapshot(&snapshot)];
        out["snapshot"] = json!({"kind":snapshot.kind(),"sha256":snapshot.sha256(),"size":snapshot.size(),"mode":mode,"reads_hex":reads,"close_error":error(snapshot.close().err().as_ref()),"repeat_close_error":error(snapshot.close().err().as_ref())});
        out["after_close_open_error"] = error(snapshot.open().err().as_ref());
    }
    if let Some(body) = raw.bodies.lock().unwrap().first() {
        let counts = body.lock().unwrap();
        out["body_reads"] = json!(counts.reads);
        out["body_closes"] = json!(counts.closes);
    }
    out["temporary_files_after"] = json!(count_files(&snapshots));
    assert_eq!(out, row.output, "source row {}", row.name);
}
pub async fn redirect(row: &Row) {
    let dir = tempfile::tempdir().unwrap();
    let (cancel, caller) = context("");
    let raw = transport(row, caller.clone(), cancel, true);
    let hooks = Arc::new(Mutex::new(Vec::new()));
    let failure = external("owned caller redirect failure");
    let mode = string(&row.input, "caller");
    let hook = (!mode.is_empty()).then(|| {
        Arc::new(Hook {
            mode: mode.to_owned(),
            observations: hooks.clone(),
            failure: failure.clone(),
        }) as Arc<dyn RedirectHook>
    });
    let loader = Loader::new(SourceLoaderOptions {
        temp_dir: dir.path().to_owned(),
        http_transport: Some(raw.clone()),
        redirect_hook: hook,
    });
    let loaded = loader.load(caller, string(&row.input, "source")).await;
    let mut out = json!({"error":error(loaded.as_ref().err()),"is_caller_error":loaded.as_ref().err().is_some_and(|error|error.contains(&failure)),"requests":*raw.requests.lock().unwrap(),"caller_hooks":*hooks.lock().unwrap(),"snapshot":null});
    if let Ok(snapshot) = loaded {
        out["snapshot"] = json!({"kind":snapshot.kind(),"sha256":snapshot.sha256(),"size":snapshot.size(),"reads_hex":[read_snapshot(&snapshot),read_snapshot(&snapshot)],"close_error":error(snapshot.close().err().as_ref())});
    }
    out["response_bodies"] = json!(
        raw.bodies
            .lock()
            .unwrap()
            .iter()
            .map(|body| {
                let body = body.lock().unwrap();
                json!({"reads":body.reads,"closes":body.closes})
            })
            .collect::<Vec<_>>()
    );
    out["temporary_files_after"] = json!(count_files(dir.path()));
    let mut expected = row.output.clone();
    let chain = expected
        .as_object_mut()
        .unwrap()
        .remove("error_chain")
        .unwrap();
    assert_eq!(
        chain.as_array().unwrap().is_empty(),
        out["error"]["code"] == ""
    );
    if !chain.as_array().unwrap().is_empty() {
        assert_eq!(chain[0]["type"], "*reversesearch.Error");
        assert_eq!(chain[1]["type"], "*url.Error");
    }
    assert_eq!(
        out, expected,
        "redirect runtime row {}; concrete Go url.Error diagnostics remain representation evidence",
        row.name
    );
}
pub async fn lifecycle(row: &Row) {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("snapshots");
    std::fs::create_dir(&dir).unwrap();
    let source = root.path().join("owned.bin");
    std::fs::write(&source, bytes(string(&row.input, "payload_hex"))).unwrap();
    let (_, caller) = context("");
    let snapshot = Loader::new(SourceLoaderOptions {
        temp_dir: dir.clone(),
        ..SourceLoaderOptions::default()
    })
    .load(caller, source.to_str().unwrap())
    .await
    .unwrap();
    let mut out = json!({"kind":snapshot.kind(),"sha256":snapshot.sha256(),"size":snapshot.size()});
    let mut expected = row.output.clone();
    if row.name == "close-failure-retry" {
        let mut blocked = BlockedSnapshot::replace(&dir);
        let first = snapshot.close().unwrap_err();
        assert_errno(&first, libc::ENOTEMPTY);
        out["close_first"] = error(Some(&first));
        out["close_second_before_restore"] = error(snapshot.close().err().as_ref());
        let go_cause = expected
            .as_object_mut()
            .unwrap()
            .remove("close_cause")
            .unwrap();
        assert_eq!(
            go_cause,
            json!({"error":"directory not empty","operation":"remove","platform":"linux","type":"syscall.Errno"})
        );
        blocked.restore();
        out["open_after_failure"] = error(snapshot.open().err().as_ref());
        out["read_after_failure_hex"] = json!(read_snapshot(&snapshot));
        out["read_after_failure_error"] = error(None);
        out["reader_close"] = error(None);
        out["close_after_restore"] = error(snapshot.close().err().as_ref());
    } else {
        let mut reader = snapshot.open().unwrap();
        out["close_with_reader_held"] = error(snapshot.close().err().as_ref());
        let mut body = Vec::new();
        reader.read_to_end(&mut body).unwrap();
        out["read_after_close_hex"] = json!(hex(&body));
        out["read_after_close_error"] = error(None);
        drop(reader);
        out["reader_close"] = error(None);
    }
    out["close_repeat"] = error(snapshot.close().err().as_ref());
    out["open_after_close"] = error(snapshot.open().err().as_ref());
    out["temporary_files_after"] = json!(count_files(&dir));
    assert_eq!(
        out, expected,
        "snapshot row {}; Rust I/O cause identity is checked separately",
        row.name
    );
}

pub async fn buffered_copy_yields_and_cancels() {
    use std::{
        future::Future,
        task::{Context as TaskContext, Poll},
    };

    let dir = tempfile::tempdir().unwrap();
    let original = vec![0x5a; 1024 * 1024];
    let input = json!({"source":"https://source.test/owned-buffered-image","payload_hex":hex(&original),"context":"","state":"","body_mode":"","status":200});
    let row = Row {
        name: "owned-buffered-copy".to_owned(),
        operation: "source_load".to_owned(),
        input,
        output: Value::Null,
    };
    let (cancel, caller) = context("");
    let raw = transport(&row, caller.clone(), cancel.clone(), false);
    let loader = Loader::new(SourceLoaderOptions {
        temp_dir: dir.path().to_owned(),
        http_transport: Some(raw.clone()),
        redirect_hook: None,
    });
    let mut load = loader.load(caller, string(&row.input, "source"));
    let waker = futures_util::task::noop_waker();
    let mut task = TaskContext::from_waker(&waker);
    assert!(
        matches!(Future::poll(load.as_mut(), &mut task), Poll::Pending),
        "fully buffered URL copying must let the same-task MCP reader receive cancellation notifications"
    );
    assert_eq!(count_files(dir.path()), 1);
    let counts = raw.bodies.lock().unwrap()[0].clone();
    assert_eq!(counts.lock().unwrap().reads, 1);
    assert_eq!(counts.lock().unwrap().closes, 0);
    cancel.cancel();
    let canceled = load.await.unwrap_err();
    assert!(canceled.contains_context(ContextError::Canceled));
    assert_eq!(canceled.to_string(), "context canceled");
    assert_eq!(counts.lock().unwrap().reads, 1);
    assert_eq!(counts.lock().unwrap().closes, 1);
    assert_eq!(count_files(dir.path()), 0);
    assert_eq!(bytes(string(&raw.input, "payload_hex")), original);
}
