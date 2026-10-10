use super::*;
use pixiv_app::reverse_search::{
    ASCII2DClient, ASCII2DSession, Aggregator, AggregatorDependencies, Dependencies, ErrorCode,
    Facade, Loader, Match, Provider, ProviderClient, ProviderResponse, Quota, Request,
    ReverseFuture, Searcher, Snapshot, SourceLoaderOptions,
};
use std::{collections::BTreeMap, sync::Mutex};
use tokio::sync::Barrier;

#[derive(Default)]
struct Observations {
    calls: BTreeMap<String, usize>,
    reads: BTreeMap<String, Vec<String>>,
    snapshots: Vec<Arc<Snapshot>>,
    close_order: Option<Vec<String>>,
    modes: usize,
    branches: usize,
    timed_out: bool,
}
struct Ports {
    input: Value,
    observed: Mutex<Observations>,
    context: CallerContext,
    modes: Barrier,
    branches: Barrier,
}
impl Ports {
    fn call(&self, name: &str, context: Option<&CallerContext>) {
        if let Some(context) = context {
            assert!(Arc::ptr_eq(context, &self.context));
        }
        *self
            .observed
            .lock()
            .unwrap()
            .calls
            .entry(name.to_owned())
            .or_default() += 1;
    }
    fn failure(&self, name: &str) -> Option<Error> {
        match self.input["errors"][name].as_str().unwrap_or("") {
            "generic" => Some(external("owned-private-provider-canary")),
            "classified" => Some(Error::new(
                ErrorCode::UpstreamHttpStatus,
                "owned reviewed provider failure",
                Some(external("owned-private-cause-canary")),
            )),
            "canceled" => Some(ContextError::Canceled.into()),
            "deadline" => Some(ContextError::DeadlineExceeded.into()),
            "wrapped-canceled" => Some(Error::wrap(
                "owned cancellation wrapper: context canceled",
                ContextError::Canceled.into(),
            )),
            _ => None,
        }
    }
    async fn gate(&self, context: &CallerContext, modes: bool) -> std::result::Result<(), Error> {
        {
            let mut observed = self.observed.lock().unwrap();
            if modes {
                observed.modes += 1;
            } else {
                observed.branches += 1;
            }
        }
        let barrier = if modes { &self.modes } else { &self.branches };
        tokio::select! {
            _=barrier.wait()=>Ok(()),
            error=context.cancelled()=>Err(error.into()),
            _=tokio::time::sleep(Duration::from_secs(2))=>{
                self.observed.lock().unwrap().timed_out=true;
                Err(external(if modes {"owned concurrency gate timed out"} else {"owned provider branch concurrency gate timed out"}))
            }
        }
    }
    async fn read(
        &self,
        context: CallerContext,
        name: &str,
        snapshot: Arc<Snapshot>,
    ) -> std::result::Result<(), Error> {
        self.call(name, Some(&context));
        if boolean(&self.input, "branches_concurrent")
            && matches!(name, "sauce.search" | "ascii.upload")
        {
            self.gate(&context, false).await?;
        }
        self.observed
            .lock()
            .unwrap()
            .snapshots
            .push(snapshot.clone());
        for _ in 0..2 {
            let read = read_snapshot(&snapshot);
            self.observed
                .lock()
                .unwrap()
                .reads
                .entry(name.to_owned())
                .or_default()
                .push(read);
        }
        self.failure(name).map_or(Ok(()), Err)
    }
    fn matches(&self, key: &str) -> Vec<Match> {
        if self.input["matches"][key].is_null() {
            Vec::new()
        } else {
            serde_json::from_value(self.input["matches"][key].clone()).unwrap()
        }
    }
    fn close(&self, provider: &str) -> std::result::Result<(), Error> {
        self.observed
            .lock()
            .unwrap()
            .close_order
            .get_or_insert_with(Vec::new)
            .push(provider.to_owned());
        let name = format!("{provider}.close");
        self.call(&name, None);
        self.failure(&name).map_or(Ok(()), Err)
    }
}
struct Sauce(Arc<Ports>);
impl ProviderClient for Sauce {
    fn preflight(
        &self,
        context: CallerContext,
    ) -> ReverseFuture<'_, std::result::Result<(), Error>> {
        Box::pin(async move {
            self.0.call("sauce.preflight", Some(&context));
            self.0.failure("sauce.preflight").map_or(Ok(()), Err)
        })
    }
    fn search(
        &self,
        context: CallerContext,
        snapshot: Arc<Snapshot>,
    ) -> ReverseFuture<'_, std::result::Result<ProviderResponse, Error>> {
        Box::pin(async move {
            self.0.read(context, "sauce.search", snapshot).await?;
            Ok(ProviderResponse {
                provider: Provider::SauceNao,
                matches: self.0.matches("sauce"),
                quota: Some(Quota {
                    short_remaining: 3,
                    long_remaining: 7,
                    short_limit: 4,
                    long_limit: 8,
                }),
            })
        })
    }
    fn close(&self) -> ReverseFuture<'_, std::result::Result<(), Error>> {
        Box::pin(async { self.0.close("sauce") })
    }
}
struct Ascii(Arc<Ports>);
impl ASCII2DClient for Ascii {
    fn preflight(
        &self,
        context: CallerContext,
    ) -> ReverseFuture<'_, std::result::Result<(), Error>> {
        Box::pin(async move {
            self.0.call("ascii.preflight", Some(&context));
            self.0.failure("ascii.preflight").map_or(Ok(()), Err)
        })
    }
    fn upload(
        &self,
        context: CallerContext,
        snapshot: Arc<Snapshot>,
    ) -> ReverseFuture<'_, std::result::Result<Arc<dyn ASCII2DSession>, Error>> {
        Box::pin(async move {
            self.0
                .read(context, "ascii.upload", snapshot.clone())
                .await?;
            Ok(Arc::new(Session {
                ports: self.0.clone(),
                snapshot,
            }) as Arc<dyn ASCII2DSession>)
        })
    }
    fn close(&self) -> ReverseFuture<'_, std::result::Result<(), Error>> {
        Box::pin(async { self.0.close("ascii") })
    }
}
struct Session {
    ports: Arc<Ports>,
    snapshot: Arc<Snapshot>,
}
impl ASCII2DSession for Session {
    fn search(
        &self,
        context: CallerContext,
        provider: Provider,
    ) -> ReverseFuture<'_, std::result::Result<ProviderResponse, Error>> {
        Box::pin(async move {
            if boolean(&self.ports.input, "concurrent") {
                self.ports.gate(&context, true).await?;
            }
            let (name, key) = if provider == Provider::Ascii2dBovw {
                ("ascii.bovw", "bovw")
            } else {
                ("ascii.color", "color")
            };
            self.ports
                .read(context, name, self.snapshot.clone())
                .await?;
            Ok(ProviderResponse {
                provider,
                matches: self.ports.matches(key),
                quota: None,
            })
        })
    }
}
pub async fn run(row: &Row) {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("owned.bin");
    std::fs::write(&source, bytes(string(&row.input, "payload_hex"))).unwrap();
    let (_, context) = context(string(&row.input, "context"));
    let ports = Arc::new(Ports {
        input: row.input.clone(),
        observed: Mutex::new(Observations::default()),
        context: context.clone(),
        modes: Barrier::new(2),
        branches: Barrier::new(2),
    });
    let aggregator = Arc::new(Aggregator::new(AggregatorDependencies {
        sauce_nao: (!boolean(&row.input, "missing_sauce"))
            .then(|| Arc::new(Sauce(ports.clone())) as Arc<dyn ProviderClient>),
        ascii2d: (!boolean(&row.input, "missing_ascii"))
            .then(|| Arc::new(Ascii(ports.clone())) as Arc<dyn ASCII2DClient>),
    }));
    let facade = Facade::new(Dependencies {
        sources: Some(Arc::new(Loader::new(SourceLoaderOptions {
            temp_dir: root.path().to_owned(),
            ..SourceLoaderOptions::default()
        }))),
        payloads: Some(aggregator),
    });
    let outcome = facade
        .search(
            context,
            Request {
                source: source.to_str().unwrap().to_owned(),
                provider: Provider::from(string(&row.input, "provider")),
                pixiv_only: boolean(&row.input, "pixiv_only"),
            },
        )
        .await;
    let first = facade.close().await.err();
    let repeat = facade.close().await.err();
    let observed = ports.observed.lock().unwrap();
    assert!(
        !observed.timed_out,
        "{} provider concurrency timed out",
        row.name
    );
    if boolean(&row.input, "concurrent") {
        assert_eq!(observed.modes, 2);
    }
    if boolean(&row.input, "branches_concurrent") {
        assert_eq!(observed.branches, 2);
    }
    let same = observed
        .snapshots
        .windows(2)
        .all(|pair| Arc::ptr_eq(&pair[0], &pair[1]));
    let closed = observed
        .snapshots
        .iter()
        .all(|snapshot| snapshot.open().is_err());
    let out = json!({"response":outcome.response,"error":error(outcome.error.as_ref()),"calls":observed.calls,"reads_hex":observed.reads,"same_snapshot":same,"closed_after_search":closed,"temporary_files_after":count_files(root.path())-1,"close_order":observed.close_order,"close_first":error(first.as_ref()),"close_repeat":error(repeat.as_ref()),"concurrent_modes_started":observed.modes,"concurrent_branches_started":observed.branches,"concurrent_gate_timed_out":observed.timed_out});
    assert_eq!(out, row.output, "aggregate row {}", row.name);
}

struct ClosingPort {
    name: &'static str,
    order: Arc<Mutex<Vec<&'static str>>>,
    release: Option<Arc<tokio::sync::Notify>>,
    failure: Error,
}
impl ProviderClient for ClosingPort {
    fn preflight(
        &self,
        _context: CallerContext,
    ) -> ReverseFuture<'_, std::result::Result<(), Error>> {
        Box::pin(async { Ok(()) })
    }
    fn search(
        &self,
        _context: CallerContext,
        _snapshot: Arc<Snapshot>,
    ) -> ReverseFuture<'_, std::result::Result<ProviderResponse, Error>> {
        Box::pin(async { Ok(ProviderResponse::default()) })
    }
    fn close(&self) -> ReverseFuture<'_, std::result::Result<(), Error>> {
        Box::pin(async {
            self.order.lock().unwrap().push(self.name);
            if let Some(release) = &self.release {
                release.notified().await;
            }
            Err(self.failure.clone())
        })
    }
}
impl ASCII2DClient for ClosingPort {
    fn preflight(
        &self,
        context: CallerContext,
    ) -> ReverseFuture<'_, std::result::Result<(), Error>> {
        ProviderClient::preflight(self, context)
    }
    fn upload(
        &self,
        _context: CallerContext,
        _snapshot: Arc<Snapshot>,
    ) -> ReverseFuture<'_, std::result::Result<Arc<dyn ASCII2DSession>, Error>> {
        Box::pin(async { Err(external("unused upload")) })
    }
    fn close(&self) -> ReverseFuture<'_, std::result::Result<(), Error>> {
        ProviderClient::close(self)
    }
}
pub async fn suspended_close_is_owned_once() {
    let order = Arc::new(Mutex::new(Vec::new()));
    let release = Arc::new(tokio::sync::Notify::new());
    let ascii_failure = external("owned ASCII close failure");
    let sauce_failure = Error::new(ErrorCode::ProviderFailed, "owned Sauce close failure", None);
    let aggregator = Arc::new(Aggregator::new(AggregatorDependencies {
        ascii2d: Some(Arc::new(ClosingPort {
            name: "ascii",
            order: order.clone(),
            release: Some(release.clone()),
            failure: ascii_failure.clone(),
        })),
        sauce_nao: Some(Arc::new(ClosingPort {
            name: "sauce",
            order: order.clone(),
            release: None,
            failure: sauce_failure.clone(),
        })),
    }));
    let facade = Facade::new(Dependencies {
        sources: None,
        payloads: Some(aggregator),
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(10), facade.close())
            .await
            .is_err()
    );
    assert_eq!(*order.lock().unwrap(), vec!["ascii"]);
    release.notify_one();
    let (first, second) = tokio::join!(facade.close(), facade.close());
    let first = first.unwrap_err();
    let second = second.unwrap_err();
    assert!(first.contains(&ascii_failure));
    assert!(first.contains(&sauce_failure));
    assert!(second.contains(&ascii_failure));
    assert!(second.contains(&sauce_failure));
    assert_eq!(
        first.to_string(),
        "owned ASCII close failure\nowned Sauce close failure"
    );
    assert_eq!(first.code(), ErrorCode::ProviderFailed);
    assert_eq!(*order.lock().unwrap(), vec!["ascii", "sauce"]);
    assert_eq!(
        facade.close().await.unwrap_err().to_string(),
        first.to_string()
    );
    assert_eq!(*order.lock().unwrap(), vec!["ascii", "sauce"]);
}
