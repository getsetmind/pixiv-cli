use pixiv_app::reverse_search::{
    CallerContext, Error, ErrorCode, Request, Response as SearchResponse, ReverseFuture,
    SearchOutcome, Searcher,
};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

pub fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../pixiv-cli/tests/fixtures/reverse-search-mcp.json"
    ))
    .unwrap()
}

pub fn response(value: &Value) -> SearchResponse {
    serde_json::from_value(value.clone()).unwrap()
}

pub fn row_error(row: &Value) -> Option<Error> {
    let message = "private-source-secret; request-header-secret; cookie-secret; key-secret";
    match row["searcher_error_kind"].as_str().unwrap() {
        "unclassified" => Some(Error::external(std::io::Error::other(message))),
        "canceled" => Some(pixiv_sdk::context::ContextError::Canceled.into()),
        "deadline" => Some(pixiv_sdk::context::ContextError::DeadlineExceeded.into()),
        "" => match row["searcher_error_code"].as_str().unwrap() {
            "" => None,
            code => Some(Error::new(
                serde_json::from_value::<ErrorCode>(json!(code)).unwrap(),
                message,
                None,
            )),
        },
        kind => panic!("unrecognized frozen error kind: {kind}"),
    }
}

pub fn request_value(request: &Request) -> Value {
    json!({"Source":request.source,"Provider":request.provider,"PixivOnly":request.pixiv_only})
}

type Search = dyn Fn(CallerContext, Request) -> ReverseFuture<'static, SearchOutcome> + Send + Sync;

pub struct FixtureSearcher {
    pub requests: Mutex<Vec<Value>>,
    pub closed: AtomicUsize,
    search: Arc<Search>,
}
impl FixtureSearcher {
    pub fn new(
        search: impl Fn(CallerContext, Request) -> ReverseFuture<'static, SearchOutcome>
        + Send
        + Sync
        + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            requests: Mutex::new(Vec::new()),
            closed: AtomicUsize::new(0),
            search: Arc::new(search),
        })
    }
    pub fn from_row(row: &Value) -> Arc<Self> {
        let response = response(&row["searcher_response"]);
        let error = row_error(row);
        Self::new(move |_, _| {
            let response = response.clone();
            let error = error.clone();
            Box::pin(async move { SearchOutcome { response, error } })
        })
    }
}
impl Searcher for FixtureSearcher {
    fn search(&self, context: CallerContext, request: Request) -> ReverseFuture<'_, SearchOutcome> {
        self.requests.lock().unwrap().push(request_value(&request));
        (self.search)(context, request)
    }
    fn close(&self) -> ReverseFuture<'_, Result<(), Error>> {
        self.closed.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }
}
