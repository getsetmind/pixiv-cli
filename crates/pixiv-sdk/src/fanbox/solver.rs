use super::{Failure, context_cause, identity::json_name_eq, options::FlareSolverrOptions};
use crate::{
    Reason,
    context::{Context, ContextError, ContextFuture, ContextKey, ContextValue, RequestContext},
    diagnostics::{Event, Scope},
    error::Cause,
};
use serde::{
    Deserialize, Deserializer,
    de::{MapAccess, Visitor},
};
use serde_json::value::RawValue;
use std::{
    any::TypeId,
    fmt,
    sync::{Arc, Mutex},
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;

const UNIX_TO_INTERNAL_SECONDS: i64 = 62_135_596_800;

pub(super) struct Solver {
    inner: Arc<Inner>,
}
struct Inner {
    options: FlareSolverrOptions,
    control: Option<reqwest::Client>,
    state: Mutex<Cache>,
}
#[derive(Default)]
struct Cache {
    cached: Option<State>,
    active: Option<Active>,
}
struct Active {
    call: Arc<Call>,
    waiters: usize,
}
struct Call {
    context: Arc<SharedContext>,
    done: watch::Sender<Option<std::result::Result<(), SharedFailure>>>,
}
#[derive(Clone)]
struct State {
    user_agent: String,
    clearance: String,
    expiry: Option<Expiry>,
}
#[derive(Clone, Copy)]
struct Expiry {
    seconds: i64,
    nanoseconds: u32,
}
impl State {
    fn usable(&self) -> bool {
        self.expiry.is_none_or(|expiry| {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default();
            let seconds = (now.as_secs() as i64).wrapping_add(UNIX_TO_INTERNAL_SECONDS);
            (seconds, now.subsec_nanos()) < (expiry.seconds, expiry.nanoseconds)
        })
    }
}
#[derive(Clone)]
struct SharedFailure {
    code: Reason,
    cause: Cause,
}
impl From<Failure> for SharedFailure {
    fn from(failure: Failure) -> Self {
        Self {
            code: failure.code,
            cause: failure.cause,
        }
    }
}
impl From<SharedFailure> for Failure {
    fn from(failure: SharedFailure) -> Self {
        Self {
            code: failure.code,
            cause: failure.cause,
        }
    }
}
struct SharedContext {
    caller: Arc<dyn RequestContext>,
    cancellation: Context,
}
impl fmt::Debug for SharedContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FANBOX solver context")
    }
}
impl RequestContext for SharedContext {
    fn error(&self) -> Option<ContextError> {
        self.cancellation.error()
    }
    fn deadline(&self) -> Option<Instant> {
        None
    }
    fn cancelled(&self) -> ContextFuture<'_> {
        Box::pin(self.cancellation.cancelled())
    }
    fn value(&self, key: &ContextKey) -> Option<ContextValue> {
        self.caller.value(key)
    }
    fn extension(&self, key: TypeId) -> Option<ContextValue> {
        self.caller.extension(key)
    }
    fn scope(&self) -> Option<&Scope> {
        self.caller.scope()
    }
}
struct Waiter {
    inner: Arc<Inner>,
    call: Arc<Call>,
}
impl Drop for Waiter {
    fn drop(&mut self) {
        let mut cache = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if let Some(active) = &mut cache.active
            && Arc::ptr_eq(&active.call, &self.call)
            && active.waiters > 0
        {
            active.waiters -= 1;
            if active.waiters == 0 {
                active.call.context.cancellation.cancel();
            }
        }
    }
}
impl Solver {
    pub(super) fn new(options: FlareSolverrOptions) -> Self {
        let control = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .http1_only()
            .pool_max_idle_per_host(0)
            .build()
            .ok();
        Self {
            inner: Arc::new(Inner {
                options,
                control,
                state: Mutex::new(Cache::default()),
            }),
        }
    }
    pub(super) fn native_state(&self) -> Option<(String, String)> {
        let mut cache = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if cache.cached.as_ref().is_some_and(|state| !state.usable()) {
            cache.cached = None;
        }
        cache
            .cached
            .as_ref()
            .map(|state| (state.user_agent.clone(), state.clearance.clone()))
    }
    pub(super) fn invalidate(&self) {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .cached = None;
    }
    pub(super) async fn wait(
        &self,
        context: Arc<dyn RequestContext>,
    ) -> std::result::Result<(), Failure> {
        if let Some(error) = context.error() {
            return Err(Failure {
                code: Reason::UpstreamError,
                cause: context_cause(error),
            });
        }
        let call = {
            let mut cache = self
                .inner
                .state
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            if cache.cached.as_ref().is_some_and(State::usable) {
                return Ok(());
            }
            cache.cached = None;
            if let Some(active) = &mut cache.active
                && active.waiters > 0
            {
                active.waiters += 1;
                active.call.clone()
            } else {
                if let Some(active) = &cache.active {
                    active.call.context.cancellation.cancel();
                }
                let (done, _) = watch::channel(None);
                let call = Arc::new(Call {
                    context: Arc::new(SharedContext {
                        caller: context.clone(),
                        cancellation: Context::new(),
                    }),
                    done,
                });
                cache.active = Some(Active {
                    call: call.clone(),
                    waiters: 1,
                });
                let inner = self.inner.clone();
                let running = call.clone();
                tokio::spawn(async move {
                    inner.run(running).await;
                });
                call
            }
        };
        let _waiter = Waiter {
            inner: self.inner.clone(),
            call: call.clone(),
        };
        let mut done = call.done.subscribe();
        loop {
            let result = done.borrow().clone();
            if let Some(result) = result {
                return result.map_err(Failure::from);
            }
            tokio::select! {changed=done.changed()=>{if changed.is_err(){return Err(unavailable());}},error=context.cancelled()=>return Err(Failure {code:Reason::UpstreamError,cause:context_cause(error)})}
        }
    }
}
impl Inner {
    async fn run(self: Arc<Self>, call: Arc<Call>) {
        let result = self.solve(call.context.as_ref()).await;
        match &result {
            Ok(_) => call.context.emit(Event {
                module: "FANBOX FlareSolverr".into(),
                kind: "solver_completed".into(),
                operation: "clearance".into(),
                ..Default::default()
            }),
            Err(failure) if call.context.error().is_none() => call.context.emit(Event {
                module: "FANBOX FlareSolverr".into(),
                kind: "failed".into(),
                operation: "challenge recovery".into(),
                reason: match failure.code {
                    Reason::UpstreamUnavailable => "solver unavailable",
                    Reason::ChallengeRequired => "solver failed",
                    Reason::MalformedUpstreamResponse => "malformed solver response",
                    _ => "command failed",
                }
                .into(),
                ..Default::default()
            }),
            Err(_) => {}
        }
        let outcome = result
            .as_ref()
            .map(|_| ())
            .map_err(|failure| SharedFailure {
                code: failure.code,
                cause: failure.cause.clone(),
            });
        let mut cache = self
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if let Some(active) = &cache.active
            && Arc::ptr_eq(&active.call, &call)
        {
            if active.waiters > 0
                && let Ok(state) = result
            {
                cache.cached = Some(state);
            }
            cache.active = None;
        }
        call.done.send_replace(Some(outcome));
    }
    async fn solve(&self, context: &dyn RequestContext) -> std::result::Result<State, Failure> {
        context.emit(Event {
            module: "FANBOX FlareSolverr".into(),
            kind: "solver_started".into(),
            operation: "challenge recovery".into(),
            ..Default::default()
        });
        let Some(client) = &self.control else {
            return Err(unavailable());
        };
        let mut payload =
            String::from("{\"cmd\":\"request.get\",\"url\":\"https://www.fanbox.cc/\"");
        if !self.options.proxy_url.is_empty() {
            payload.push_str(",\"proxy\":");
            payload.push_str(
                &serde_json::to_string(&self.options.proxy_url).map_err(|_| unavailable())?,
            );
        }
        payload.push('}');
        let endpoint = format!("{}/v1", self.options.url.trim_end_matches('/'));
        let request = client
            .post(&endpoint)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .body(payload)
            .send();
        let mut response = tokio::select! {response=request=>response.map_err(|_|context.error().map(canceled).unwrap_or_else(unavailable))?,error=context.cancelled()=>return Err(canceled(error))};
        if matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308)
            && let Some(location) = response.headers().get("Location")
            && !location.is_empty()
        {
            let valid = location
                .to_str()
                .ok()
                .is_some_and(|location| valid_redirect(&endpoint, location));
            if !valid {
                drop(response);
                return Err(unavailable());
            }
        }
        if !response.status().is_success() {
            drop(response);
            return Err(failed());
        }
        let mut bytes = Vec::new();
        loop {
            let chunk = tokio::select! {chunk=response.chunk()=>chunk.map_err(|_|malformed())?,error=context.cancelled()=>return Err(canceled(error))};
            let Some(chunk) = chunk else {
                drop(response);
                return Err(malformed());
            };
            bytes.extend_from_slice(&chunk);
            let mut decoder = serde_json::Deserializer::from_slice(&bytes);
            match Box::<RawValue>::deserialize(&mut decoder) {
                Ok(document) => {
                    drop(response);
                    return decode_solution(document.as_ref());
                }
                Err(error) if error.is_eof() => {}
                Err(_) => {
                    drop(response);
                    return Err(malformed());
                }
            }
        }
    }
}
fn canceled(error: ContextError) -> Failure {
    Failure {
        code: Reason::UpstreamError,
        cause: context_cause(error),
    }
}
fn unavailable() -> Failure {
    Failure::new(
        Reason::UpstreamUnavailable,
        "fanbox: FlareSolverr service unavailable",
    )
}
fn failed() -> Failure {
    Failure::new(
        Reason::ChallengeRequired,
        "fanbox: FlareSolverr could not solve challenge",
    )
}
fn malformed() -> Failure {
    Failure::new(
        Reason::MalformedUpstreamResponse,
        "fanbox: malformed FlareSolverr response",
    )
}
fn valid_redirect(endpoint: &str, location: &str) -> bool {
    if location.bytes().any(|byte| byte < 32 || byte == 127) {
        return false;
    }
    let path = location.split(['?', '#']).next().unwrap_or("");
    if crate::reference::decode_url_component(path, false).is_none() {
        return false;
    }
    reqwest::Url::parse(endpoint)
        .ok()
        .and_then(|url| url.join(location).ok())
        .is_some()
}

struct Members(Vec<(String, Box<RawValue>)>);
impl<'de> Deserialize<'de> for Members {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct MemberVisitor;
        impl<'de> Visitor<'de> for MemberVisitor {
            type Value = Members;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a JSON object")
            }
            fn visit_map<M: MapAccess<'de>>(
                self,
                mut map: M,
            ) -> std::result::Result<Self::Value, M::Error> {
                let mut members = vec![];
                while let Some(member) = map.next_entry()? {
                    members.push(member);
                }
                Ok(Members(members))
            }
        }
        deserializer.deserialize_map(MemberVisitor)
    }
}
#[derive(Default)]
struct Solution {
    user_agent: String,
    cookies: Vec<Cookie>,
}
#[derive(Default)]
struct Cookie {
    name: String,
    value: String,
    expires: Option<Box<RawValue>>,
    expiry: Option<Box<RawValue>>,
}
fn members(raw: &RawValue) -> std::result::Result<Members, Failure> {
    serde_json::from_str(raw.get()).map_err(|_| malformed())
}
fn string_field(raw: &RawValue, target: &mut String) -> std::result::Result<(), Failure> {
    if raw.get() != "null" {
        *target = serde_json::from_str(raw.get()).map_err(|_| malformed())?;
    }
    Ok(())
}
fn decode_solution(raw: &RawValue) -> std::result::Result<State, Failure> {
    let mut status = String::new();
    let mut solution = None;
    if raw.get() != "null" {
        for (key, value) in members(raw)?.0 {
            if json_name_eq(&key, "status") {
                string_field(&value, &mut status)?;
            } else if json_name_eq(&key, "solution") {
                if value.get() == "null" {
                    solution = None;
                } else {
                    let current = solution.get_or_insert_with(Solution::default);
                    decode_solution_fields(&value, current)?;
                }
            }
        }
    }
    if status != "ok" {
        return Err(failed());
    }
    let solution = solution.ok_or_else(malformed)?;
    if solution.user_agent.trim().is_empty()
        || solution
            .user_agent
            .chars()
            .any(|character| character < ' ' || character == '\u{7f}')
    {
        return Err(malformed());
    }
    let mut selected = None;
    for cookie in solution.cookies {
        if cookie.name != "cf_clearance" {
            continue;
        }
        if selected.is_some()
            || cookie.value.is_empty()
            || !cookie
                .value
                .bytes()
                .all(|byte| (0x21..=0x7e).contains(&byte) && !b"\",;\\".contains(&byte))
        {
            return Err(malformed());
        }
        selected = Some(cookie);
    }
    let cookie = selected.ok_or_else(malformed)?;
    let expiry = cookie
        .expiry
        .as_deref()
        .filter(|raw| raw.get() != "null")
        .or_else(|| cookie.expires.as_deref().filter(|raw| raw.get() != "null"));
    let expiry = expiry.map(parse_expiry).transpose()?;
    Ok(State {
        user_agent: solution.user_agent,
        clearance: cookie.value,
        expiry,
    })
}
fn decode_solution_fields(
    raw: &RawValue,
    solution: &mut Solution,
) -> std::result::Result<(), Failure> {
    for (key, value) in members(raw)?.0 {
        if json_name_eq(&key, "userAgent") {
            string_field(&value, &mut solution.user_agent)?;
        } else if json_name_eq(&key, "cookies") {
            solution.cookies = if value.get() == "null" {
                vec![]
            } else {
                let raw_cookies: Vec<Box<RawValue>> =
                    serde_json::from_str(value.get()).map_err(|_| malformed())?;
                let mut cookies = Vec::with_capacity(raw_cookies.len());
                for raw_cookie in raw_cookies {
                    let mut cookie = Cookie::default();
                    if raw_cookie.get() != "null" {
                        for (key, value) in members(&raw_cookie)?.0 {
                            if json_name_eq(&key, "name") {
                                string_field(&value, &mut cookie.name)?;
                            } else if json_name_eq(&key, "value") {
                                string_field(&value, &mut cookie.value)?;
                            } else if json_name_eq(&key, "expires") {
                                cookie.expires = Some(value);
                            } else if json_name_eq(&key, "expiry") {
                                cookie.expiry = Some(value);
                            }
                        }
                    }
                    cookies.push(cookie);
                }
                cookies
            };
        }
    }
    Ok(())
}
fn parse_expiry(raw: &RawValue) -> std::result::Result<Expiry, Failure> {
    let number = serde_json::from_str::<serde_json::Number>(raw.get()).ok();
    let text = serde_json::from_str::<String>(raw.get()).ok();
    let number = number.or_else(|| {
        text.as_deref()
            .and_then(|text| serde_json::from_str::<serde_json::Number>(text).ok())
    });
    if let Some(number) = number {
        let seconds = number
            .as_i64()
            .filter(|seconds| *seconds > 0)
            .ok_or_else(malformed)?;
        // A checked Unix conversion would reject Go's accepted int64 expiry domain.
        return Ok(Expiry {
            seconds: seconds.wrapping_add(UNIX_TO_INTERNAL_SECONDS),
            nanoseconds: 0,
        });
    }
    let text = text.ok_or_else(malformed)?;
    let date = chrono::DateTime::parse_from_rfc3339(&text)
        .map(|date| date.with_timezone(&chrono::Utc))
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(&text, "%a, %d %b %Y %H:%M:%S GMT")
                .map(|date| date.and_utc())
        })
        .map_err(|_| malformed())?;
    Ok(Expiry {
        seconds: date.timestamp().wrapping_add(UNIX_TO_INTERNAL_SECONDS),
        nanoseconds: date.timestamp_subsec_nanos(),
    })
}
