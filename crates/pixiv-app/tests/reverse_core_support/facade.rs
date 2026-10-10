use super::*;
use pixiv_app::reverse_search::{
    Dependencies, ErrorCode, Facade, Input, Loader, PayloadQuery, PayloadRequest, PayloadSearcher,
    Provider, Request, Response, ReverseFuture, SearchOutcome, Searcher, Snapshot, SourceKind,
    SourceLoader, SourceLoaderOptions,
};
use std::sync::Mutex;

struct Ports {
    input: Value,
    source: String,
    directory: PathBuf,
    context: CallerContext,
    observed: Mutex<Value>,
    loaded: Mutex<Option<Arc<Snapshot>>>,
    blocked: Mutex<Option<BlockedSnapshot>>,
    preflight_error: Error,
    source_error: Error,
    payload_error: Option<Error>,
}
impl Ports {
    fn call(&self, name: &str, context: &CallerContext) {
        assert!(Arc::ptr_eq(context, &self.context));
        self.observed.lock().unwrap()["calls"]
            .as_array_mut()
            .unwrap()
            .push(json!(name));
    }
}
struct Sources(Arc<Ports>);
impl SourceLoader for Sources {
    fn load<'a>(
        &'a self,
        context: CallerContext,
        source: &'a str,
    ) -> ReverseFuture<'a, std::result::Result<Arc<Snapshot>, Error>> {
        Box::pin(async move {
            self.0.call("source.load", &context);
            self.0.observed.lock().unwrap()["source_matches_request"] =
                json!(source == self.0.source);
            if boolean(&self.0.input, "source_error") {
                return Err(self.0.source_error.clone());
            }
            let snapshot = Loader::new(SourceLoaderOptions {
                temp_dir: self.0.directory.clone(),
                ..SourceLoaderOptions::default()
            })
            .load(context, source)
            .await?;
            *self.0.loaded.lock().unwrap() = Some(snapshot.clone());
            Ok(snapshot)
        })
    }
}
struct Payloads(Arc<Ports>);
impl PayloadSearcher for Payloads {
    fn preflight(
        &self,
        context: CallerContext,
        query: PayloadQuery,
    ) -> ReverseFuture<'_, std::result::Result<(), Error>> {
        Box::pin(async move {
            self.0.call("payload.preflight", &context);
            self.0.observed.lock().unwrap()["preflight"] = json!({"provider":query.provider,"pixiv_only":query.pixiv_only,"context_error":error(context.error().map(Error::from).as_ref())});
            if boolean(&self.0.input, "preflight_error") {
                Err(self.0.preflight_error.clone())
            } else {
                Ok(())
            }
        })
    }
    fn search_payload(
        &self,
        context: CallerContext,
        request: PayloadRequest,
    ) -> ReverseFuture<'_, SearchOutcome> {
        Box::pin(async move {
            self.0.call("payload.search", &context);
            let same = self
                .0
                .loaded
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|snapshot| Arc::ptr_eq(snapshot, &request.snapshot));
            self.0.observed.lock().unwrap()["payload_request"] = json!({"provider":request.provider,"pixiv_only":request.pixiv_only,"same_loaded_snapshot":same});
            if boolean(&self.0.input, "block_snapshot_close") {
                *self.0.blocked.lock().unwrap() = Some(BlockedSnapshot::replace(&self.0.directory));
            }
            SearchOutcome {
                response: Response {
                    input: Input {
                        kind: SourceKind::Url,
                        sha256: "owned payload input".to_owned(),
                    },
                    partial: true,
                    ..Response::default()
                },
                error: self.0.payload_error.clone(),
            }
        })
    }
}
pub async fn run(row: &Row) {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("snapshots");
    std::fs::create_dir(&directory).unwrap();
    let source = root.path().join("owned.bin");
    std::fs::write(&source, bytes(string(&row.input, "payload_hex"))).unwrap();
    let (_, context) = context(string(&row.input, "context"));
    let payload_error = match string(&row.input, "payload_error") {
        "classified" => Some(Error::new(
            ErrorCode::ProviderFailed,
            "owned payload search failure",
            Some(external("owned payload cause")),
        )),
        "generic" => Some(external("owned unclassified payload failure")),
        "wrapped-canceled" => Some(Error::wrap(
            "owned payload cancellation: context canceled",
            ContextError::Canceled.into(),
        )),
        _ => None,
    };
    let ports = Arc::new(Ports {
        input: row.input.clone(),
        source: source.to_str().unwrap().to_owned(),
        directory: directory.clone(),
        context: context.clone(),
        observed: Mutex::new(json!({"calls":[],"preflight":null,"payload_request":null})),
        loaded: Mutex::new(None),
        blocked: Mutex::new(None),
        preflight_error: Error::new(
            ErrorCode::MissingCredential,
            "owned payload preflight failure",
            Some(external("owned preflight cause")),
        ),
        source_error: Error::new(
            ErrorCode::SourceReadFailed,
            "owned source failure",
            Some(external("owned source cause")),
        ),
        payload_error,
    });
    let facade = Facade::new(Dependencies {
        sources: (!boolean(&row.input, "nil_sources"))
            .then(|| Arc::new(Sources(ports.clone())) as Arc<dyn SourceLoader>),
        payloads: (!boolean(&row.input, "nil_payloads"))
            .then(|| Arc::new(Payloads(ports.clone())) as Arc<dyn PayloadSearcher>),
    });
    let outcome = facade
        .search(
            context,
            Request {
                source: source.to_str().unwrap().to_owned(),
                provider: Provider::SauceNao,
                pixiv_only: true,
            },
        )
        .await;
    let mut out = ports.observed.lock().unwrap().clone();
    out["response"] = serde_json::to_value(outcome.response).unwrap();
    out["error"] = error(outcome.error.as_ref());
    out["joined_errors"] =
        outcome
            .error
            .as_ref()
            .and_then(Error::joined)
            .map_or(Value::Null, |errors| {
                json!(
                    errors
                        .iter()
                        .map(|item| error(Some(item)))
                        .collect::<Vec<_>>()
                )
            });
    out["is_preflight_error"] = json!(
        outcome
            .error
            .as_ref()
            .is_some_and(|error| error.contains(&ports.preflight_error))
    );
    out["is_source_error"] = json!(
        outcome
            .error
            .as_ref()
            .is_some_and(|error| error.contains(&ports.source_error))
    );
    out["is_payload_error"] = json!(outcome.error.as_ref().is_some_and(|error| {
        ports
            .payload_error
            .as_ref()
            .is_some_and(|payload| error.contains(payload))
    }));
    out["temporary_files_after_search"] = json!(count_files(&directory));
    let blocked = ports.blocked.lock().unwrap().is_some();
    let mut expected = row.output.clone();
    let go_cause = expected
        .as_object_mut()
        .unwrap()
        .remove("filesystem_cause")
        .unwrap();
    if blocked {
        let joined = outcome.error.as_ref().unwrap().joined().unwrap();
        assert_errno(joined.last().unwrap(), libc::ENOTEMPTY);
        assert_eq!(
            go_cause,
            json!({"error":"directory not empty","operation":"remove","platform":"linux","type":"syscall.Errno"})
        );
        ports.blocked.lock().unwrap().as_mut().unwrap().restore();
    } else {
        assert_eq!(go_cause, Value::Null);
    }
    if let Some(snapshot) = ports.loaded.lock().unwrap().as_ref() {
        let opened = snapshot.open();
        out["snapshot_open_after_search"] = error(opened.as_ref().err());
        if let Ok(mut reader) = opened {
            let mut body = Vec::new();
            reader.read_to_end(&mut body).unwrap();
            drop(reader);
            out["snapshot_read_after_search_hex"] = json!(hex(&body));
            out["snapshot_read_after_search_error"] = error(None);
            out["snapshot_reader_close"] = error(None);
        }
        out["snapshot_close_retry"] = error(snapshot.close().err().as_ref());
        out["snapshot_close_repeat"] = error(snapshot.close().err().as_ref());
    }
    out["temporary_files_after_retry"] = json!(count_files(&directory));
    assert_eq!(
        out, expected,
        "facade row {}; Go filesystem representation retained as separate evidence",
        row.name
    );
}
