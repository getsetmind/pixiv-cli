use chrono::{DateTime, FixedOffset, TimeZone};
use pixiv_app::{
    reverse_search::{
        ReverseFuture,
        http::{HttpRequest, HttpTransport},
    },
    update::{
        CallerContext, ExternalError, ReleaseCache, ReleaseCheckOptions, UpdateFuture,
        http::ClientTransport,
        release::{GitHubReleaseClient, ReleaseClientOptions, SemanticVersion},
        source::{ReleaseSourceSelector, parse_release_sources},
    },
};
use pixiv_sdk::{
    context::{Context, ContextError},
    fanbox::transport::{BodyFuture, Headers, RawBody, RawRead, RawResponse},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    error::Error,
    fmt,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::Notify;

#[derive(Clone, Default, Deserialize)]
#[serde(default)]
struct Input {
    api_base_url: String,
    repository: String,
    nil_cache: bool,
    nil_client: bool,
    cache_exists: bool,
    cache_before: String,
    cache_read_error: String,
    cache_write_error: String,
    cache_cancel_at: String,
    cache_wait_context: bool,
    cancel_before: bool,
    now_cancel: bool,
    now_date_year: i32,
    now: String,
    parent_deadline: String,
    automatic: bool,
    include_prerelease: bool,
    public_source_winner: String,
    private_single_mirror: bool,
    replies: Vec<Reply>,
}
#[derive(Clone, Default, Deserialize)]
#[serde(default)]
struct Reply {
    url: String,
    status: u16,
    headers: Headers,
    body: String,
    request_error: String,
    read_error: String,
    error_with_data: bool,
    close_error: String,
    cancel_at: String,
    wait_context: bool,
}
#[derive(Default, Serialize)]
struct BodyState {
    reads: usize,
    bytes: usize,
    closes: usize,
    read_error: String,
    close_error: String,
}
#[derive(Serialize)]
struct RequestState {
    method: String,
    url: String,
    headers: Headers,
    has_body: bool,
    deadline: String,
}
#[derive(Default, Serialize)]
struct Observation {
    release: Option<Value>,
    throttled: bool,
    error: String,
    error_chain: Vec<String>,
    canceled: bool,
    deadline_error: bool,
    trace: Vec<String>,
    requests: Vec<RequestState>,
    bodies: Vec<BodyState>,
    cache_write_attempts: Vec<String>,
    cache_after: String,
    cache_exists_after: bool,
    cache_port_deadlines: Vec<String>,
    probe_losers_canceled: usize,
    now_calls: usize,
}
#[derive(Clone)]
struct State {
    input: Input,
    observed: Arc<Mutex<Observation>>,
    parent: Arc<Context>,
}
#[derive(Debug)]
struct OwnedError(String);
impl fmt::Display for OwnedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl Error for OwnedError {}
fn owned(s: impl Into<String>) -> ExternalError {
    Box::new(OwnedError(s.into()))
}
fn deadline(ctx: &CallerContext, parent: &Context) -> String {
    match ctx.deadline() {
        None => "none",
        Some(d) if Some(d) == parent.deadline() => "parent-preserved",
        Some(d)
            if d > Instant::now() + Duration::from_secs(3)
                || d + Duration::from_millis(100) < Instant::now() =>
        {
            "unexpected-child-deadline"
        }
        Some(_) => "automatic-3s-child",
    }
    .into()
}
impl ReleaseCache for State {
    fn read(
        &self,
        context: CallerContext,
    ) -> UpdateFuture<'_, Result<Option<Vec<u8>>, ExternalError>> {
        Box::pin(async move {
            {
                let mut s = self.observed.lock().unwrap();
                s.trace.push("cache.read".into());
                s.cache_port_deadlines
                    .push(deadline(&context, &self.parent));
            }
            if self.input.cache_cancel_at == "read" {
                self.parent.cancel();
            }
            if self.input.cache_wait_context {
                return Err(Box::new(context.cancelled().await) as ExternalError);
            }
            if !self.input.cache_read_error.is_empty() {
                return Err(owned(&self.input.cache_read_error));
            }
            let s = self.observed.lock().unwrap();
            Ok(s.cache_exists_after
                .then(|| s.cache_after.as_bytes().to_vec()))
        })
    }
    fn write(
        &self,
        context: CallerContext,
        data: Vec<u8>,
    ) -> UpdateFuture<'_, Result<(), ExternalError>> {
        Box::pin(async move {
            let data = String::from_utf8(data).unwrap();
            {
                let mut s = self.observed.lock().unwrap();
                s.trace.push("cache.write".into());
                s.cache_port_deadlines
                    .push(deadline(&context, &self.parent));
                s.cache_write_attempts.push(data.clone());
            }
            if self.input.cache_cancel_at == "write" {
                self.parent.cancel();
            }
            if !self.input.cache_write_error.is_empty() {
                return Err(owned(&self.input.cache_write_error));
            }
            let mut s = self.observed.lock().unwrap();
            s.cache_after = data;
            s.cache_exists_after = true;
            Ok(())
        })
    }
}
struct Body {
    state: State,
    reply: Reply,
    index: usize,
    offset: usize,
}
impl RawBody for Body {
    fn read<'a>(&'a mut self, output: &'a mut [u8]) -> BodyFuture<'a, RawRead> {
        Box::pin(async move {
            let mut s = self.state.observed.lock().unwrap();
            s.trace.push(format!("body.{}.read", self.index + 1));
            s.bodies[self.index].reads += 1;
            if self.reply.cancel_at == "read" {
                self.state.parent.cancel();
            }
            if !self.reply.read_error.is_empty()
                && (!self.reply.error_with_data || self.offset == self.reply.body.len())
            {
                s.bodies[self.index].read_error = self.reply.read_error.clone();
                return RawRead {
                    count: 0,
                    eof: false,
                    error: Some(owned(&self.reply.read_error)),
                };
            }
            let count = (self.reply.body.len() - self.offset).min(output.len());
            output[..count]
                .copy_from_slice(&self.reply.body.as_bytes()[self.offset..self.offset + count]);
            self.offset += count;
            s.bodies[self.index].bytes += count;
            let error = if !self.reply.read_error.is_empty() {
                s.bodies[self.index].read_error = self.reply.read_error.clone();
                Some(owned(&self.reply.read_error))
            } else {
                None
            };
            RawRead {
                count,
                eof: count == 0,
                error,
            }
        })
    }
    fn close(&mut self) -> BodyFuture<'_, Result<(), ExternalError>> {
        Box::pin(async move {
            let mut s = self.state.observed.lock().unwrap();
            s.trace.push(format!("body.{}.close", self.index + 1));
            s.bodies[self.index].closes += 1;
            if self.reply.cancel_at == "close" {
                self.state.parent.cancel();
            }
            if !self.reply.close_error.is_empty() {
                s.bodies[self.index].close_error = self.reply.close_error.clone();
                return Err(owned(&self.reply.close_error));
            }
            Ok(())
        })
    }
}
struct Transport {
    state: State,
    winner_requests: Mutex<usize>,
    winner_recorded: Notify,
    loser_recorded: Notify,
    loser_done: Notify,
}
const ENDPOINT: &str = "https://api.github.com/repos/FlanChanXwO/pixiv-cli/releases";
impl HttpTransport for Transport {
    fn send(
        &self,
        request: HttpRequest,
    ) -> ReverseFuture<'_, Result<Option<RawResponse>, ExternalError>> {
        Box::pin(async move {
            let (mut first_winner, mut loser) = (false, false);
            if !self.state.input.public_source_winner.is_empty() {
                let mirror = format!("https://gh-proxy.com/{ENDPOINT}");
                let (winner_url, loser_url) =
                    if self.state.input.public_source_winner == "github-direct" {
                        (ENDPOINT, mirror.as_str())
                    } else {
                        (mirror.as_str(), ENDPOINT)
                    };
                loser = request.url == loser_url;
                if loser {
                    wait_witness(&self.winner_recorded, "winner request recorded").await;
                }
                if request.url == winner_url {
                    let mut calls = self.winner_requests.lock().unwrap();
                    *calls += 1;
                    first_winner = *calls == 1;
                }
                if request.url == winner_url && !first_winner {
                    wait_witness(&self.loser_done, "loser cancellation observed").await;
                }
            }
            let reply = {
                let mut s = self.state.observed.lock().unwrap();
                let n = s.requests.len();
                s.trace.push(format!("request.{}", n + 1));
                s.requests.push(RequestState {
                    method: request.method.clone(),
                    url: request.url.clone(),
                    headers: request.headers.clone(),
                    has_body: request.body.is_some(),
                    deadline: deadline(&request.context, &self.state.parent),
                });
                self.state
                    .input
                    .replies
                    .get(n)
                    .unwrap_or_else(|| panic!("unexpected request {}", request.url))
                    .clone()
            };
            assert_eq!(request.url, reply.url);
            if first_winner {
                self.winner_recorded.notify_one();
                wait_witness(&self.loser_recorded, "loser request recorded").await;
            }
            if loser {
                self.loser_recorded.notify_one();
            }
            if reply.cancel_at == "request" {
                self.state.parent.cancel();
            }
            if reply.wait_context {
                let e = request.context.cancelled().await;
                if loser {
                    let mut s = self.state.observed.lock().unwrap();
                    if e == ContextError::Canceled {
                        s.probe_losers_canceled += 1;
                    }
                    s.trace.push("probe.loser.canceled".into());
                    self.loser_done.notify_one();
                }
                return Err(Box::new(e) as ExternalError);
            }
            if !reply.request_error.is_empty() {
                return Err(owned(&reply.request_error));
            }
            let index = {
                let mut s = self.state.observed.lock().unwrap();
                let n = s.bodies.len();
                s.bodies.push(BodyState::default());
                n
            };
            Ok(Some(RawResponse {
                status: reply.status,
                headers: reply.headers.clone(),
                content_length: -1,
                body: Some(Box::new(Body {
                    state: self.state.clone(),
                    reply,
                    index,
                    offset: 0,
                })),
            }))
        })
    }
}
async fn wait_witness(notification: &Notify, label: &str) {
    tokio::time::timeout(Duration::from_secs(5), notification.notified())
        .await
        .unwrap_or_else(|_| panic!("source race witness timed out: {label}"));
}
fn duration_seconds(mut value: &str) -> f64 {
    let sign = if let Some(rest) = value.strip_prefix('-') {
        value = rest;
        -1.
    } else {
        value = value.strip_prefix('+').unwrap_or(value);
        1.
    };
    if value == "0" {
        return 0.;
    }
    let mut seconds = 0.;
    while !value.is_empty() {
        let end = value
            .bytes()
            .take_while(|b| b.is_ascii_digit() || *b == b'.')
            .count();
        let number: f64 = value[..end].parse().expect("valid Go duration number");
        value = &value[end..];
        let (unit, factor) = [
            ("ns", 1e-9),
            ("us", 1e-6),
            ("µs", 1e-6),
            ("μs", 1e-6),
            ("ms", 1e-3),
            ("s", 1.),
            ("m", 60.),
            ("h", 3600.),
        ]
        .into_iter()
        .find(|(unit, _)| value.starts_with(unit))
        .expect("valid Go duration unit");
        value = &value[unit.len()..];
        seconds += number * factor;
    }
    sign * seconds
}
async fn observe(input: Input) -> Value {
    assert!(
        !input.nil_client,
        "Go nil receiver is a separate representation boundary"
    );
    let parent = Arc::new(if input.parent_deadline.is_empty() {
        Context::new()
    } else {
        let seconds = duration_seconds(&input.parent_deadline);
        let now = Instant::now();
        Context::with_deadline(if seconds < 0. {
            now - Duration::from_secs_f64(-seconds)
        } else {
            now + Duration::from_secs_f64(seconds)
        })
    });
    if input.cancel_before {
        parent.cancel();
    }
    let state = State {
        observed: Arc::new(Mutex::new(Observation {
            cache_after: input.cache_before.clone(),
            cache_exists_after: input.cache_exists,
            ..Observation::default()
        })),
        input: input.clone(),
        parent: parent.clone(),
    };
    let transport = Arc::new(Transport {
        state: state.clone(),
        winner_requests: Mutex::new(0),
        winner_recorded: Notify::new(),
        loser_recorded: Notify::new(),
        loser_done: Notify::new(),
    });
    let transport = Arc::new(ClientTransport::new(transport));
    let mut now = DateTime::parse_from_rfc3339(&input.now).unwrap();
    if input.now_date_year != 0 {
        now = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(input.now_date_year, 10, 10, 12, 0, 0)
            .unwrap();
    }
    let clock_state = state.clone();
    let source_selector = input.private_single_mirror.then(|| {
        Arc::new(ReleaseSourceSelector::new(
            parse_release_sources(
                b"fixture-mirror|https://mirror.invalid/{url}|https://mirror.invalid/{url}\n",
            )
            .unwrap(),
            transport.clone(),
        ))
    });
    let result = match GitHubReleaseClient::new(ReleaseClientOptions {
        api_base_url: input.api_base_url,
        repository: input.repository,
        transport: Some(transport),
        cache: (!input.nil_cache).then(|| Arc::new(state.clone()) as Arc<dyn ReleaseCache>),
        now: Some(Arc::new(move || {
            let mut s = clock_state.observed.lock().unwrap();
            s.now_calls += 1;
            s.trace.push("now".into());
            if clock_state.input.now_cancel {
                clock_state.parent.cancel();
            }
            now
        })),
        enable_public_release_sources: !input.public_source_winner.is_empty(),
        source_selector,
    }) {
        Err(e) => Err(e),
        Ok(client) => {
            client
                .check(
                    parent.clone(),
                    ReleaseCheckOptions {
                        automatic: input.automatic,
                        include_prerelease: input.include_prerelease,
                    },
                )
                .await
        }
    };
    let mut s = state.observed.lock().unwrap();
    match result {
        Ok(r) => {
            s.throttled = r.throttled;
            s.release=r.release.map(|r|json!({"tag_name":r.tag_name,"version":r.version,"prerelease":r.prerelease,"assets":if r.assets.is_empty(){Value::Null}else{serde_json::to_value(r.assets).unwrap()}}));
        }
        Err(e) => {
            s.error = e.to_string();
            let mut cause: Option<&dyn Error> = Some(e.as_ref());
            while let Some(e) = cause {
                s.error_chain.push(e.to_string());
                if e.downcast_ref::<ContextError>() == Some(&ContextError::Canceled) {
                    s.canceled = true;
                }
                if e.downcast_ref::<ContextError>() == Some(&ContextError::DeadlineExceeded) {
                    s.deadline_error = true;
                }
                cause = e.source();
            }
        }
    };
    serde_json::to_value(&*s).unwrap()
}
fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../pixiv-cli/tests/fixtures/updater-release-cache.json"
    ))
    .unwrap()
}
async fn compare_boundary(boundary: &str, count: usize) {
    let fixture = fixture();
    let cases = fixture["cases"].as_array().unwrap();
    let mut checked = 0;
    let mut differences = Vec::new();
    for c in cases.iter().filter(|c| c["boundary"] == boundary) {
        let actual = tokio::time::timeout(
            Duration::from_secs(10),
            observe(serde_json::from_value(c["input"].clone()).unwrap()),
        )
        .await
        .unwrap_or_else(|_| panic!("release fixture timed out: {}", c["name"]));
        checked += 1;
        if actual != c["observation"] {
            for key in c["observation"].as_object().unwrap().keys() {
                if actual[key] != c["observation"][key] {
                    differences.push(format!(
                        "{}.{key}: actual {} expected {}",
                        c["name"], actual[key], c["observation"][key]
                    ));
                }
            }
        }
    }
    assert_eq!(checked, count);
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}
#[tokio::test]
async fn public_constructor_check_transport_cache_matches_frozen_go_153_rows() {
    compare_boundary("public-constructor-check-transport-cache", 153).await;
}
#[tokio::test]
async fn configured_selector_mapping_matches_frozen_go_six_private_rows() {
    compare_boundary("private-selector-injection-connected-check", 6).await;
}
#[test]
fn go_nil_receiver_row_is_retained_as_unrepresentable_rust_reference() {
    let f = fixture();
    let rows: Vec<_> = f["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["boundary"] == "go-nil-receiver-only")
        .collect();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "nil-client-check-error");
    assert_eq!(
        rows[0]["observation"]["error"],
        "GitHub release client is nil"
    );
}
#[test]
fn canonical_semver_keeps_unlimited_decimals_and_metadata() {
    let large = "9".repeat(10000);
    let a = SemanticVersion::parse(&format!("v{large}.0.0-alpha.{large}+001")).unwrap();
    let b = SemanticVersion::parse("v999.0.0").unwrap();
    assert_eq!(a.compare(&b), 1);
    assert!(a.is_prerelease());
    assert_eq!(a.to_string(), format!("{large}.0.0-alpha.{large}+001"));
    assert_eq!(
        SemanticVersion::parse("v1.0.0+001")
            .unwrap()
            .compare(&SemanticVersion::parse("v1.0.0+002").unwrap()),
        0
    );
}

fn edge_oracle() -> Value {
    serde_json::from_str(include_str!("support/updater_release_edge_oracle.json")).unwrap()
}
#[tokio::test]
async fn invalid_percent_escapes_keep_go_byte_diagnostics_without_utf8_panics() {
    let oracle = edge_oracle();
    let cases: Vec<_> = oracle
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["base"] != "")
        .collect();
    assert_eq!(cases.len(), 6);
    for row in cases {
        let actual = observe(Input {
            api_base_url: row["base"].as_str().unwrap().into(),
            now: "2026-10-10T12:00:00Z".into(),
            ..Input::default()
        })
        .await;
        assert_eq!(actual["error"], row["error"], "{}", row["base"]);
        assert_eq!(actual["error_chain"], row["chain"], "{}", row["base"]);
        assert_eq!(
            actual["cache_write_attempts"], row["writes"],
            "{}",
            row["base"]
        );
    }
}
#[tokio::test]
async fn cache_timestamps_keep_go_nanosecond_trim_offset_and_year_spelling() {
    let oracle = edge_oracle();
    let cases: Vec<_> = oracle
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["now"] != "")
        .collect();
    assert_eq!(cases.len(), 7);
    let mut differences = Vec::new();
    for row in cases {
        let actual = observe(Input {
            now: row["now"].as_str().unwrap().into(),
            replies: vec![Reply {
                url: ENDPOINT.into(),
                status: 200,
                body: "[]".into(),
                ..Reply::default()
            }],
            ..Input::default()
        })
        .await;
        for (actual_key, expected_key) in [
            ("error", "error"),
            ("error_chain", "chain"),
            ("cache_write_attempts", "writes"),
        ] {
            if actual[actual_key] != row[expected_key] {
                differences.push(format!(
                    "{}.{actual_key}: actual {} expected {}",
                    row["now"], actual[actual_key], row[expected_key]
                ));
            }
        }
    }
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

#[tokio::test]
async fn cache_checked_at_zero_instant_uses_absolute_nanosecond_precision() {
    let oracle: Value =
        serde_json::from_str(include_str!("support/updater_release_zero_oracle.json")).unwrap();
    let rows = oracle.as_array().unwrap();
    assert_eq!(rows.len(), 8);
    assert_eq!(
        rows.iter().filter(|row| row["go_is_zero"] == true).count(),
        3
    );
    let mut differences = Vec::new();
    for row in rows {
        let actual = observe(Input {
            cache_exists: true,
            cache_before: row["cache_before"].as_str().unwrap().into(),
            now: "2026-10-10T12:00:00Z".into(),
            automatic: true,
            replies: vec![Reply {
                url: ENDPOINT.into(),
                status: 200,
                body: "[{\"tag_name\":\"v2.0.0\"}]".into(),
                ..Reply::default()
            }],
            ..Input::default()
        })
        .await;
        for key in [
            "release",
            "throttled",
            "error",
            "error_chain",
            "now_calls",
            "cache_write_attempts",
        ] {
            if actual[key] != row[key] {
                differences.push(format!(
                    "{}.{key}: actual {} expected {}",
                    row["checked_at"], actual[key], row[key]
                ));
            }
        }
        let requests = actual["requests"].as_array().unwrap().len() as u64;
        if requests != row["requests"].as_u64().unwrap() {
            differences.push(format!(
                "{}.requests: actual {requests} expected {}",
                row["checked_at"], row["requests"]
            ));
        }
    }
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}
