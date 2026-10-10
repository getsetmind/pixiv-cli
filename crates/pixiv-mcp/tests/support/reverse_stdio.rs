#[path = "reverse_search.rs"]
mod reverse_search;
use pixiv_app::{config::Store, database::Database, execution::Execution};
use pixiv_sdk::transport::{Request as TransportRequest, Response, Transport};
pub use reverse_search::*;
use serde_json::Value;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Clone)]
pub struct ForbiddenTransport;
impl Transport for ForbiddenTransport {
    async fn send(&self, _: TransportRequest) -> pixiv_sdk::Result<Response> {
        panic!("reverse search must not use Pixiv SDK transport")
    }
}
pub struct EmptyExecution {
    pub execution: Execution<ForbiddenTransport>,
    pub execute_calls: Arc<AtomicUsize>,
    _directory: tempfile::TempDir,
}
impl EmptyExecution {
    pub fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(&path, "").unwrap();
        let database = Arc::new(Mutex::new(Database::open(directory.path()).unwrap()));
        let execute_calls = Arc::new(AtomicUsize::new(0));
        let calls = execute_calls.clone();
        let execution = Execution::new(Store::new(path), database, move |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(ForbiddenTransport)
        });
        Self {
            execution,
            execute_calls,
            _directory: directory,
        }
    }
}

pub fn frames(messages: &[Value]) -> String {
    messages.iter().map(|value| format!("{value}\n")).collect()
}
pub fn parse_frames(bytes: &[u8]) -> Vec<Value> {
    std::str::from_utf8(bytes)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

pub fn standard_response() -> pixiv_app::reverse_search::Response {
    let fixture = fixture();
    let row = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "override-saucenao")
        .unwrap();
    response(&row["searcher_response"])
}

pub struct LocalSnapshotProvider {
    pub preflights: AtomicUsize,
    pub searches: AtomicUsize,
    pub closes: AtomicUsize,
    expected_bytes: Arc<Vec<u8>>,
}
impl LocalSnapshotProvider {
    pub fn new(expected_bytes: Arc<Vec<u8>>) -> Arc<Self> {
        Arc::new(Self {
            preflights: AtomicUsize::new(0),
            searches: AtomicUsize::new(0),
            closes: AtomicUsize::new(0),
            expected_bytes,
        })
    }
}
impl pixiv_app::reverse_search::ProviderClient for LocalSnapshotProvider {
    fn preflight(
        &self,
        _: pixiv_app::reverse_search::CallerContext,
    ) -> pixiv_app::reverse_search::ReverseFuture<'_, Result<(), pixiv_app::reverse_search::Error>>
    {
        self.preflights.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }

    fn search(
        &self,
        _: pixiv_app::reverse_search::CallerContext,
        snapshot: Arc<pixiv_app::reverse_search::Snapshot>,
    ) -> pixiv_app::reverse_search::ReverseFuture<
        '_,
        Result<pixiv_app::reverse_search::ProviderResponse, pixiv_app::reverse_search::Error>,
    > {
        self.searches.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut snapshot.open().unwrap(), &mut bytes).unwrap();
            assert_eq!(bytes, *self.expected_bytes);
            assert_eq!(snapshot.size(), self.expected_bytes.len() as i64);
            Ok(pixiv_app::reverse_search::ProviderResponse {
                provider: pixiv_app::reverse_search::Provider::SauceNao,
                ..Default::default()
            })
        })
    }

    fn close(
        &self,
    ) -> pixiv_app::reverse_search::ReverseFuture<'_, Result<(), pixiv_app::reverse_search::Error>>
    {
        self.closes.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }
}
