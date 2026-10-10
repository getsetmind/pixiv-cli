mod expiry_date;
mod identity;
mod json;
pub mod models;
pub mod native;
pub mod options;
mod solver;
pub mod transport;
pub use models::{CurrentUserRequest, SessionCredentials, User, UserDto};
pub use options::{FlareSolverrOptions, Options};

use crate::{
    Error, Reason, Result,
    context::{ContextError, RequestContext},
    diagnostics::Event,
    error::{Cause, is_canceled, is_deadline_exceeded},
};
use std::{error::Error as StdError, fmt, sync::Arc};
use transport::{EmptyBody, Headers, RawBody, RawRequest, RawResponse, RawTransport, header};

const WEB_BASE_URL: &str = "https://www.fanbox.cc/";
const IDENTITY_ACCEPT: &str = "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8";

pub struct Client {
    session: Session,
}
impl fmt::Display for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FANBOX Client")
    }
}
impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("fanbox.Client{}")
    }
}
impl Client {
    pub fn open(credentials: SessionCredentials) -> Result<Self> {
        Self::open_with(credentials, Options::default())
    }
    pub fn open_with(credentials: SessionCredentials, options: Options) -> Result<Self> {
        Session::new(
            format!("FANBOXSESSID={}", credentials.fanbox_sessid),
            options,
        )
        .map(|session| Self { session })
        .map_err(|failure| failure.classify("Open"))
    }
    pub async fn current_user(
        &self,
        context: Arc<dyn RequestContext>,
        _request: CurrentUserRequest,
    ) -> Result<User> {
        self.session
            .current_user(context)
            .await
            .map_err(|failure| failure.classify("CurrentUser"))
    }
    pub async fn validate_session(&self, context: Arc<dyn RequestContext>) -> Result<()> {
        self.session
            .current_user(context)
            .await
            .map(|_| ())
            .map_err(|failure| failure.classify("ValidateSession"))
    }
    pub fn close_idle_connections(&self) {
        self.session.transport.close_idle_connections();
    }
}
struct Session {
    transport: Arc<dyn RawTransport>,
    proxy_url: String,
    user_agent: String,
    cookie: String,
    solver: Option<solver::Solver>,
}
impl Session {
    fn new(cookie: String, options: Options) -> std::result::Result<Self, Failure> {
        let user_agent = options::validate_user_agent(&options.user_agent)?;
        let proxy_url = options::validate_proxy_url(&options.proxy_url)?;
        let solver_options = options::normalize_solver(options.flare_solverr)?;
        let transport = match options.http_client {
            Some(transport) => transport,
            None => Arc::new(native::NativeTransport::new(&proxy_url).map_err(|_| {
                Failure::new(
                    Reason::CredentialsExpired,
                    "create FANBOX TLS transport failed",
                )
            })?),
        };
        let cookie = options::normalize_cookie(&cookie)?;
        Ok(Self {
            transport,
            proxy_url,
            user_agent,
            cookie,
            solver: solver_options.map(solver::Solver::new),
        })
    }
    fn native_state(&self) -> (String, String) {
        self.solver
            .as_ref()
            .and_then(solver::Solver::native_state)
            .unwrap_or_else(|| (self.user_agent.clone(), String::new()))
    }
    async fn request(
        &self,
        context: Arc<dyn RequestContext>,
    ) -> std::result::Result<RawResponse, Failure> {
        let result = self.native_request(context.clone()).await;
        if !matches!(&result,Err(failure) if failure.code == Reason::ChallengeRequired) {
            return result;
        }
        let Some(solver) = &self.solver else {
            return result;
        };
        solver.invalidate();
        solver.wait(context.clone()).await?;
        context.emit(Event {
            module: "FANBOX network".into(),
            kind: "replay".into(),
            operation: "request".into(),
            route: "native transport".into(),
            ..Default::default()
        });
        let result = self.native_request(context).await;
        if matches!(&result,Err(failure) if failure.code == Reason::ChallengeRequired) {
            solver.invalidate();
        }
        result
    }
    async fn native_request(
        &self,
        context: Arc<dyn RequestContext>,
    ) -> std::result::Result<RawResponse, Failure> {
        let mut target = options::parse_url(WEB_BASE_URL).expect("fixed FANBOX URL");
        let mut visited = std::collections::BTreeSet::new();
        let mut redirected = false;
        loop {
            if target.parsed.scheme() != "https" || target.authority.contains('@') {
                return Err(Failure::message("FANBOX URL is not an allowed HTTPS URL"));
            }
            let host = target.parsed.host_str().unwrap_or("").to_ascii_lowercase();
            if host != "fanbox.cc" && !host.ends_with(".fanbox.cc") {
                return Err(Failure::message("FANBOX URL host is not allowed"));
            }
            if !visited.insert(target.wire.clone()) {
                return Err(Failure::message("FANBOX redirect loop detected"));
            }
            let (agent, clearance) = self.native_state();
            let mut headers = Headers::from([
                ("Origin".into(), vec!["https://www.fanbox.cc".into()]),
                ("Referer".into(), vec![WEB_BASE_URL.into()]),
                ("User-Agent".into(), vec![agent.clone()]),
                ("Accept".into(), vec![IDENTITY_ACCEPT.into()]),
            ]);
            if !redirected && matches!(host.as_str(), "www.fanbox.cc" | "api.fanbox.cc") {
                let mut cookie = self.cookie.clone();
                if !clearance.is_empty() {
                    cookie.push_str("; cf_clearance=");
                    cookie.push_str(&clearance);
                }
                headers.insert("Cookie".into(), vec![cookie]);
            }
            let sent = self
                .transport
                .send(RawRequest {
                    method: "GET".into(),
                    url: target.wire.clone(),
                    logical_host: None,
                    headers,
                    body: None,
                    content_length: 0,
                    context: context.clone(),
                })
                .await;
            let mut response = match sent {
                Ok(Some(response)) if response.body.is_some() || response.content_length <= 0 => {
                    response
                }
                Ok(_) => {
                    return Err(Failure {
                        code: Reason::UpstreamError,
                        cause: failed_request(context.as_ref(), None),
                    });
                }
                Err(error) => {
                    return Err(Failure {
                        code: Reason::UpstreamError,
                        cause: failed_request(context.as_ref(), Some(error.as_ref())),
                    });
                }
            };
            if response.body.is_none() {
                response.body = Some(Box::new(EmptyBody));
            }
            let redirect = matches!(response.status, 301 | 302 | 303 | 307 | 308);
            let location = redirect.then(|| header(&response.headers, "Location").to_owned());
            let next = if let Some(location) =
                location.as_deref().filter(|location| !location.is_empty())
            {
                match redirect_target(&target, location) {
                    Some(target) => Some(target),
                    None => {
                        // Go Client.Do rejects malformed Location before ErrUseLastResponse and discards close failures.
                        if let Some(body) = response.body.as_mut() {
                            let _ = body.close().await;
                        }
                        return Err(Failure {
                            code: Reason::UpstreamError,
                            cause: failed_request(context.as_ref(), None),
                        });
                    }
                }
            } else {
                None
            };
            context.emit(Event {
                module: "FANBOX network".into(),
                kind: "network_request".into(),
                operation: "retrieving".into(),
                resource: target.parsed.path().into(),
                route: "native transport".into(),
                proxy: self.proxy_url.clone(),
                user_agent: agent,
                status: i64::from(response.status),
                ..Default::default()
            });
            if let Some(location) = location {
                close_body(
                    context.as_ref(),
                    &mut response,
                    "close FANBOX response failed",
                )
                .await?;
                if location.trim().is_empty() {
                    return Err(Failure::message("FANBOX redirect has no location"));
                }
                target = next.expect("nonempty redirect location was parsed");
                redirected = true;
                continue;
            }
            if response.status == 304 || (200..300).contains(&response.status) {
                return Ok(response);
            }
            return Err(classify_response(context.as_ref(), response).await);
        }
    }
}
fn redirect_target(target: &options::ParsedUrl, location: &str) -> Option<options::ParsedUrl> {
    if location.bytes().any(|byte| byte < 32 || byte == 127) {
        return None;
    }
    let path = location.split(['?', '#']).next().unwrap_or("");
    crate::reference::decode_url_component(path, false)?;
    let joined = target.parsed.join(location).ok()?;
    let raw_authority = if let Some((_, rest)) = location.split_once("://") {
        &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())]
    } else if let Some(rest) = location.strip_prefix("//") {
        &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())]
    } else {
        &target.authority
    };
    let mut wire = format!("{}://{raw_authority}{}", joined.scheme(), joined.path());
    if let Some(query) = joined.query() {
        wire.push('?');
        wire.push_str(query);
    }
    if let Some(fragment) = joined.fragment().filter(|fragment| !fragment.is_empty()) {
        wire.push('#');
        wire.push_str(fragment);
    }
    options::parse_url(&wire)
}
fn failed_request(context: &dyn RequestContext, error: Option<&(dyn StdError + 'static)>) -> Cause {
    context.emit(Event {
        module: "FANBOX network".into(),
        kind: "failed".into(),
        operation: "network request".into(),
        route: "native transport".into(),
        ..Default::default()
    });
    match error {
        Some(error) => safe_external_error(context, "FANBOX request failed", error),
        None => context
            .error()
            .map(context_cause)
            .unwrap_or_else(|| Cause::Redacted("FANBOX request failed".into())),
    }
}
pub(super) fn context_cause(error: ContextError) -> Cause {
    match error {
        ContextError::Canceled => Cause::Canceled,
        ContextError::DeadlineExceeded => Cause::DeadlineExceeded,
    }
}
pub(super) fn safe_external_error(
    context: &dyn RequestContext,
    message: &str,
    error: &(dyn StdError + 'static),
) -> Cause {
    if let Some(error) = context.error() {
        return context_cause(error);
    }
    if is_canceled(error) {
        return Cause::Canceled;
    }
    if is_deadline_exceeded(error) {
        return Cause::DeadlineExceeded;
    }
    let mut current = Some(error);
    while let Some(error) = current {
        if let Some(error) = error.downcast_ref::<ContextError>() {
            return context_cause(*error);
        }
        current = error.source();
    }
    Cause::Redacted(message.into())
}
pub(super) struct Failure {
    pub code: Reason,
    pub cause: Cause,
}
impl Failure {
    pub(super) fn new(code: Reason, message: impl Into<String>) -> Self {
        Self {
            code,
            cause: Cause::Redacted(message.into()),
        }
    }
    pub(super) fn message(message: impl Into<String>) -> Self {
        Self::new(Reason::UpstreamError, message)
    }
    fn classify(self, operation: &str) -> Error {
        if is_canceled(&self.cause) {
            return Error::with_product("fanbox", Reason::UpstreamError, operation)
                .with_cause(Cause::Canceled);
        }
        if is_deadline_exceeded(&self.cause) {
            return Error::with_product("fanbox", Reason::UpstreamError, operation)
                .with_cause(Cause::DeadlineExceeded);
        }
        Error::with_product("fanbox", self.code, operation).with_cause(self.cause)
    }
}
impl fmt::Debug for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Failure")
            .field("code", &self.code)
            .field("cause", &self.cause)
            .finish()
    }
}
pub(super) async fn read_body(
    body: &mut dyn RawBody,
) -> std::result::Result<Vec<u8>, transport::ExternalError> {
    let mut result = Vec::new();
    let mut buffer = [0u8; 32 * 1024];
    loop {
        let read = body.read(&mut buffer).await;
        if read.count > buffer.len() {
            return Err(Box::new(std::io::Error::other("invalid body read count")));
        }
        result.extend_from_slice(&buffer[..read.count]);
        if let Some(error) = read.error {
            return Err(error);
        }
        if read.eof {
            return Ok(result);
        }
    }
}
async fn close_body(
    context: &dyn RequestContext,
    response: &mut RawResponse,
    message: &str,
) -> std::result::Result<(), Failure> {
    let Some(body) = &mut response.body else {
        return Err(Failure::message("FANBOX response has no body"));
    };
    body.close().await.map_err(|error| Failure {
        code: Reason::UpstreamError,
        cause: safe_external_error(context, message, error.as_ref()),
    })
}
async fn classify_response(context: &dyn RequestContext, mut response: RawResponse) -> Failure {
    let scanned = if response.status == 403 {
        match response.body.as_mut() {
            Some(body) => scan_challenge_body(body.as_mut()).await,
            None => Ok(false),
        }
    } else {
        Ok(false)
    };
    if let Err(error) = close_body(context, &mut response, "close FANBOX response failed").await {
        return error;
    }
    let marker = match scanned {
        Ok(marker) => marker,
        Err(error) => {
            return Failure {
                code: Reason::UpstreamError,
                cause: safe_external_error(
                    context,
                    "read FANBOX error response failed",
                    error.as_ref(),
                ),
            };
        }
    };
    match response.status {
        401 => Failure::new(Reason::CredentialsExpired, "fanbox: not authenticated"),
        403 => {
            let mitigated = response.headers.iter().any(|(key, values)| {
                key.eq_ignore_ascii_case("Cf-Mitigated")
                    && values
                        .iter()
                        .any(|value| value.to_ascii_lowercase().contains("challenge"))
            });
            let html = header(&response.headers, "Content-Type")
                .trim()
                .to_ascii_lowercase()
                .starts_with("text/html")
                && (header(&response.headers, "Server")
                    .to_ascii_lowercase()
                    .contains("cloudflare")
                    || !header(&response.headers, "Cf-Ray").is_empty());
            if marker || mitigated || html {
                context.emit(Event {
                    module: "FANBOX network".into(),
                    kind: "challenge".into(),
                    operation: "request".into(),
                    status: 403,
                    ..Default::default()
                });
                Failure::new(Reason::ChallengeRequired, "fanbox: challenge required")
            } else {
                Failure::new(Reason::Forbidden, "fanbox: forbidden")
            }
        }
        status => Failure::message(format!("FANBOX request returned HTTP status {status}")),
    }
}

async fn scan_challenge_body(
    body: &mut dyn RawBody,
) -> std::result::Result<bool, transport::ExternalError> {
    let mut buffer = [0u8; 32 * 1024];
    let mut tail = Vec::new();
    let mut found = false;
    loop {
        let read = body.read(&mut buffer).await;
        if read.count > buffer.len() {
            return Err(Box::new(std::io::Error::other("invalid body read count")));
        }
        tail.extend(buffer[..read.count].iter().map(u8::to_ascii_lowercase));
        found |= tail
            .windows(6)
            .any(|window| window == b"cf-chl" || window == b"cf_chl");
        if tail.len() > 5 {
            tail.drain(..tail.len() - 5);
        }
        if let Some(error) = read.error {
            return Err(error);
        }
        if read.eof {
            return Ok(found);
        }
    }
}
