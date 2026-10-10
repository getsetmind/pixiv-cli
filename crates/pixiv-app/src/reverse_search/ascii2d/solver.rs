use super::{close_response, read_body};
use crate::reverse_search::{
    CallerContext, Error, ErrorCode,
    http::{HttpRequest, HttpTransport, ReqwestTransport},
};
use pixiv_sdk::{
    context::{Context, ContextError, ContextFuture, ContextKey, ContextValue, RequestContext},
    diagnostics::Scope,
    fanbox::transport::Headers,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    any::TypeId,
    fmt,
    io::Cursor,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{Mutex as AsyncMutex, Notify};
use url::Url;

#[derive(Default)]
pub struct FlareSolverrOptions {
    pub url: String,
    pub proxy_url: String,
    pub http_transport: Option<Arc<dyn HttpTransport>>,
}
pub(super) struct Solver {
    endpoint: String,
    proxy_url: String,
    transport: Arc<dyn HttpTransport>,
    session: String,
    created: AsyncMutex<bool>,
}
#[derive(Clone)]
pub(super) struct State {
    pub user_agent: String,
    pub clearance: String,
    expiry: Option<SystemTime>,
}
impl State {
    fn expired(&self) -> bool {
        self.expiry
            .is_some_and(|expiry| SystemTime::now() >= expiry)
    }
}
#[derive(Serialize)]
struct Payload<'a> {
    cmd: &'a str,
    session: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    proxy: Option<Proxy<'a>>,
    #[serde(rename = "maxTimeout", skip_serializing_if = "Option::is_none")]
    max_timeout: Option<u32>,
}
#[derive(Serialize)]
struct Proxy<'a> {
    url: &'a str,
}
#[derive(Deserialize)]
struct Response {
    status: String,
    solution: Option<Solution>,
}
#[derive(Deserialize)]
struct Solution {
    #[serde(rename = "userAgent", default)]
    user_agent: String,
    #[serde(default)]
    cookies: Vec<Cookie>,
}
#[derive(Deserialize)]
struct Cookie {
    #[serde(default)]
    name: String,
    #[serde(default)]
    value: String,
    expires: Option<Value>,
    expiry: Option<Value>,
}
impl Solver {
    pub fn new(options: FlareSolverrOptions) -> Result<Self, Error> {
        let raw = options.url.trim();
        let url = Url::parse(raw).map_err(|_| unavailable())?;
        if url.host_str().is_none()
            || super::has_user(raw)
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some_and(|query| !query.is_empty())
            || url.fragment().is_some_and(|fragment| !fragment.is_empty())
            || !matches!(url.scheme(), "http" | "https")
        {
            return Err(unavailable());
        }
        let mut random = [0; 16];
        getrandom::fill(&mut random).map_err(|_| unavailable())?;
        let session = format!(
            "pixiv-cli-ascii2d-{}",
            random
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        let transport = match options.http_transport {
            Some(transport) => transport,
            None => Arc::new(ReqwestTransport::new("").map_err(|_| unavailable())?)
                as Arc<dyn HttpTransport>,
        };
        Ok(Self {
            endpoint: raw.trim_end_matches('/').to_owned(),
            proxy_url: options.proxy_url,
            transport,
            session,
            created: AsyncMutex::new(false),
        })
    }
    async fn call(&self, context: CallerContext, payload: Payload<'_>) -> Result<Response, Error> {
        let bytes = serde_json::to_vec(&payload).map_err(|_| malformed())?;
        let mut headers = Headers::new();
        headers.insert("Content-Type".into(), vec!["application/json".into()]);
        headers.insert("Accept".into(), vec!["application/json".into()]);
        let result = self
            .transport
            .send(HttpRequest {
                method: "POST".into(),
                url: format!("{}/v1", self.endpoint),
                logical_host: None,
                headers,
                content_length: bytes.len() as i64,
                body: Some(Box::new(Cursor::new(bytes))),
                context: context.clone(),
            })
            .await;
        let mut response = match result {
            Ok(Some(response)) => response,
            Ok(None) => return Err(malformed()),
            Err(_) => return Err(context.error().map(Error::from).unwrap_or_else(unavailable)),
        };
        let result = async {
            if response.body.is_none() {
                return Err(malformed());
            }
            if !(200..300).contains(&response.status) {
                return Err(failed());
            }
            let bytes = read_body(&mut response).await.map_err(|_| malformed())?;
            let decoded: Response = serde_json::from_slice(&bytes).map_err(|_| malformed())?;
            match decoded.status.as_str() {
                "ok" => Ok(decoded),
                "error" => Err(failed()),
                _ => Err(malformed()),
            }
        }
        .await;
        close_response(&mut response).await;
        result
    }
    async fn create(&self, context: CallerContext) -> Result<(), Error> {
        let mut created = self.created.lock().await;
        if *created {
            return Ok(());
        }
        self.call(
            context,
            Payload {
                cmd: "sessions.create",
                session: &self.session,
                url: None,
                proxy: (!self.proxy_url.is_empty()).then_some(Proxy {
                    url: &self.proxy_url,
                }),
                max_timeout: None,
            },
        )
        .await?;
        *created = true;
        Ok(())
    }
    async fn solve(&self, context: CallerContext, url: &str) -> Result<State, Error> {
        self.create(context.clone()).await?;
        let response = self
            .call(
                context,
                Payload {
                    cmd: "request.get",
                    session: &self.session,
                    url: Some(url),
                    proxy: None,
                    max_timeout: Some(180000),
                },
            )
            .await?;
        let solution = response.solution.ok_or_else(malformed)?;
        if solution.user_agent.trim().is_empty()
            || solution
                .user_agent
                .chars()
                .any(|character| character < ' ' || character == '\x7f')
        {
            return Err(malformed());
        }
        let mut cookies = solution
            .cookies
            .into_iter()
            .filter(|cookie| cookie.name == "cf_clearance");
        let cookie = cookies.next().ok_or_else(malformed)?;
        if cookies.next().is_some()
            || cookie.value.is_empty()
            || cookie
                .value
                .chars()
                .any(|character| !('!'..='~').contains(&character) || "\",;\\".contains(character))
        {
            return Err(malformed());
        }
        let expiry = expiry(cookie.expiry.or(cookie.expires))?;
        Ok(State {
            user_agent: solution.user_agent,
            clearance: cookie.value,
            expiry,
        })
    }
    pub async fn destroy(&self) -> Result<(), Error> {
        let mut created = self.created.lock().await;
        if !*created {
            return Ok(());
        }
        self.call(
            Arc::new(Context::background()),
            Payload {
                cmd: "sessions.destroy",
                session: &self.session,
                url: None,
                proxy: None,
                max_timeout: None,
            },
        )
        .await
        .map_err(raw_solver_error)?;
        *created = false;
        Ok(())
    }
    pub fn close_idle(&self) {
        self.transport.close_idle_connections();
    }
}
fn expiry(value: Option<Value>) -> Result<Option<SystemTime>, Error> {
    let Some(value) = value else { return Ok(None) };
    if value.is_null() {
        return Ok(None);
    }
    if let Some(number) = value.as_i64() {
        if number <= 0 {
            return Err(malformed());
        }
        return UNIX_EPOCH
            .checked_add(std::time::Duration::from_secs(number as u64))
            .map(Some)
            .ok_or_else(malformed);
    }
    if let Some(text) = value.as_str() {
        let timestamp = chrono::DateTime::parse_from_rfc3339(text)
            .or_else(|_| {
                chrono::NaiveDateTime::parse_from_str(text, "%a, %d %b %Y %H:%M:%S GMT")
                    .map(|date| date.and_utc().fixed_offset())
            })
            .map_err(|_| malformed())?
            .timestamp();
        return if timestamp >= 0 {
            UNIX_EPOCH.checked_add(std::time::Duration::from_secs(timestamp as u64))
        } else {
            UNIX_EPOCH.checked_sub(std::time::Duration::from_secs(timestamp.unsigned_abs()))
        }
        .map(Some)
        .ok_or_else(malformed);
    }
    Err(malformed())
}
#[derive(Debug)]
struct Sentinel(&'static str);
impl fmt::Display for Sentinel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for Sentinel {}
pub(super) fn malformed() -> Error {
    Error::new(
        ErrorCode::MalformedSolverResponse,
        "ascii2d challenge solver response is malformed",
        Some(Error::external(Sentinel(
            "ascii2d: malformed FlareSolverr response",
        ))),
    )
}
fn unavailable() -> Error {
    Error::new(
        ErrorCode::SolverUnavailable,
        "ascii2d challenge solver is unavailable",
        Some(Error::external(Sentinel(
            "ascii2d: FlareSolverr service unavailable",
        ))),
    )
}
pub(super) fn failed() -> Error {
    Error::new(
        ErrorCode::SolverFailed,
        "ascii2d challenge solver failed",
        Some(Error::external(Sentinel(
            "ascii2d: FlareSolverr could not solve challenge",
        ))),
    )
}
fn raw_solver_error(error: Error) -> Error {
    match error.code() {
        ErrorCode::SolverUnavailable => {
            Error::external(Sentinel("ascii2d: FlareSolverr service unavailable"))
        }
        ErrorCode::SolverFailed => {
            Error::external(Sentinel("ascii2d: FlareSolverr could not solve challenge"))
        }
        ErrorCode::MalformedSolverResponse => {
            Error::external(Sentinel("ascii2d: malformed FlareSolverr response"))
        }
        _ => error,
    }
}
fn closed() -> Error {
    Error::external(std::io::Error::other(
        "ascii2d: solver state cache is closed",
    ))
}
#[derive(Default)]
pub(super) struct Cache {
    inner: Mutex<CacheInner>,
}
#[derive(Default)]
struct CacheInner {
    state: Option<State>,
    active: Option<Arc<Call>>,
    closed: bool,
}
struct Call {
    cancel: Context,
    context: CallerContext,
    waiters: AtomicUsize,
    result: Mutex<Option<Result<State, Error>>>,
    notify: Notify,
}
impl Cache {
    pub async fn get(
        self: &Arc<Self>,
        context: CallerContext,
        solver: Arc<Solver>,
        url: String,
    ) -> Result<State, Error> {
        if let Some(error) = context.error() {
            return Err(error.into());
        }
        let call = {
            let mut inner = self
                .inner
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            if inner.closed {
                return Err(closed());
            }
            if let Some(state) = &inner.state {
                if !state.expired() {
                    return Ok(state.clone());
                }
                inner.state = None;
            }
            if let Some(call) = &inner.active {
                if call.waiters.load(Ordering::SeqCst) > 0 {
                    call.waiters.fetch_add(1, Ordering::SeqCst);
                    call.clone()
                } else {
                    self.start(&mut inner, context.clone(), solver, url)
                }
            } else {
                self.start(&mut inner, context.clone(), solver, url)
            }
        };
        let _waiter = Waiter {
            cache: self.clone(),
            call: call.clone(),
        };
        loop {
            let notified = call.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(result) = call
                .result
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .clone()
            {
                return result;
            }
            tokio::select! { _=&mut notified=>{},error=context.cancelled()=>return Err(error.into()) }
        }
    }
    fn start(
        self: &Arc<Self>,
        inner: &mut CacheInner,
        source: CallerContext,
        solver: Arc<Solver>,
        url: String,
    ) -> Arc<Call> {
        if let Some(previous) = &inner.active {
            previous.cancel.cancel();
        }
        let cancel = Context::background().child();
        let context = Arc::new(Detached {
            source,
            cancel: cancel.clone(),
        }) as CallerContext;
        let call = Arc::new(Call {
            cancel,
            context,
            waiters: AtomicUsize::new(1),
            result: Mutex::new(None),
            notify: Notify::new(),
        });
        inner.active = Some(call.clone());
        let cache = self.clone();
        let task_call = call.clone();
        tokio::spawn(async move {
            let mut result = solver.solve(task_call.context.clone(), &url).await;
            let mut inner = cache
                .inner
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            if inner.closed {
                result = Err(closed());
            }
            if inner
                .active
                .as_ref()
                .is_some_and(|active| Arc::ptr_eq(active, &task_call))
            {
                if !inner.closed
                    && task_call.waiters.load(Ordering::SeqCst) > 0
                    && let Ok(state) = &result
                    && !state.expired()
                {
                    inner.state = Some(state.clone());
                }
                inner.active = None;
            }
            *task_call
                .result
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()) = Some(result);
            task_call.notify.notify_waiters();
        });
        call
    }
    pub fn invalidate(&self) {
        self.inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .state = None;
    }
    pub fn close(&self) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        inner.closed = true;
        inner.state = None;
        if let Some(call) = &inner.active {
            call.cancel.cancel();
        }
    }
}
struct Waiter {
    cache: Arc<Cache>,
    call: Arc<Call>,
}
impl Drop for Waiter {
    fn drop(&mut self) {
        let inner = self
            .cache
            .inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if inner
            .active
            .as_ref()
            .is_some_and(|active| Arc::ptr_eq(active, &self.call))
            && self.call.waiters.fetch_sub(1, Ordering::SeqCst) == 1
        {
            self.call.cancel.cancel();
        }
    }
}
struct Detached {
    source: CallerContext,
    cancel: Context,
}
impl fmt::Debug for Detached {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SolverContext").finish_non_exhaustive()
    }
}
impl RequestContext for Detached {
    fn error(&self) -> Option<ContextError> {
        self.cancel.error()
    }
    fn deadline(&self) -> Option<Instant> {
        None
    }
    fn cancelled(&self) -> ContextFuture<'_> {
        Box::pin(self.cancel.cancelled())
    }
    fn value(&self, key: &ContextKey) -> Option<ContextValue> {
        self.source.value(key)
    }
    fn extension(&self, type_id: TypeId) -> Option<ContextValue> {
        self.source.extension(type_id)
    }
    fn scope(&self) -> Option<&Scope> {
        self.source.scope()
    }
}
