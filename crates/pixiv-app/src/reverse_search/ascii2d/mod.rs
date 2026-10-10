mod cookies;
mod html;
mod solver;

use super::http::{HttpRequest, HttpTransport, NativeHttpTransport};
use super::{
    ASCII2DClient, ASCII2DSession, CallerContext, Error, ErrorCode, Provider, ProviderResponse,
    RedirectDecision, RedirectHook, ReverseFuture, Snapshot,
};
use futures_util::{FutureExt, future::Shared as SharedFuture};
use pixiv_sdk::fanbox::{
    native::{HeaderPolicy, NativeOptions, NativeTransport},
    transport::{Headers, RawResponse},
};
use std::{
    io::{self, Cursor, Read},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use url::Url;
type CloseFuture = SharedFuture<ReverseFuture<'static, Result<(), Error>>>;

pub use solver::FlareSolverrOptions;
pub const MAX_IMAGE_BYTES: i64 = 10 * 1024 * 1024;
const DEFAULT_ENDPOINT: &str = "https://ascii2d.net";
const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/146.0.0.0 Safari/537.36";
const ACCEPT: &str = "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8,application/signed-exchange;v=b3;q=0.7";

#[derive(Default)]
pub struct Options {
    pub http_transport: Option<Arc<dyn HttpTransport>>,
    pub endpoint: String,
    pub proxy_url: String,
    pub user_agent: String,
    pub flaresolverr: Option<FlareSolverrOptions>,
    pub redirect_hook: Option<Arc<dyn RedirectHook>>,
}
#[derive(Clone)]
pub struct Client {
    shared: Arc<Shared>,
    endpoint: Url,
    endpoint_wire: String,
    user_agent: String,
}
struct Shared {
    transport: Arc<dyn HttpTransport>,
    redirect_hook: Option<Arc<dyn RedirectHook>>,
    solver: Option<Arc<solver::Solver>>,
    cache: Arc<solver::Cache>,
    closed: Mutex<Option<CloseFuture>>,
}
pub struct Session {
    client: Client,
    hash: String,
    jar: Arc<Mutex<cookies::Jar>>,
}
impl Client {
    pub fn new(options: Options) -> Result<Self, Error> {
        let raw = if options.endpoint.is_empty() {
            DEFAULT_ENDPOINT
        } else {
            &options.endpoint
        };
        let endpoint = Url::parse(raw).map_err(|_| invalid_endpoint())?;
        if endpoint.host_str().is_none()
            || has_user(raw)
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some_and(|query| !query.is_empty())
            || endpoint
                .fragment()
                .is_some_and(|fragment| !fragment.is_empty())
            || !matches!(endpoint.scheme(), "http" | "https")
        {
            return Err(invalid_endpoint());
        }
        let user_agent = normalize_agent(&options.user_agent)?;
        validate_proxy(&options.proxy_url)?;
        let transport = match options.http_transport {
            Some(transport) => transport,
            None => {
                let native = NativeTransport::with_options(NativeOptions {
                    proxy_url: if options.proxy_url.trim().is_empty() {
                        String::new()
                    } else {
                        options.proxy_url
                    },
                    header_policy: HeaderPolicy::Ascii2d,
                })
                .map_err(|_| {
                    domain(
                        ErrorCode::ProviderFailed,
                        "could not create ascii2d browser transport",
                    )
                })?;
                Arc::new(NativeHttpTransport::new(Arc::new(native))) as Arc<dyn HttpTransport>
            }
        };
        let solver = options
            .flaresolverr
            .map(solver::Solver::new)
            .transpose()?
            .map(Arc::new);
        let endpoint_wire = if let Some((_, rest)) = raw.split_once(':') {
            format!("{}:{rest}", endpoint.scheme())
        } else {
            raw.to_owned()
        };
        Ok(Self {
            shared: Arc::new(Shared {
                transport,
                redirect_hook: options.redirect_hook,
                solver,
                cache: Arc::new(solver::Cache::default()),
                closed: Mutex::new(None),
            }),
            endpoint,
            endpoint_wire,
            user_agent,
        })
    }
    async fn upload_inner(
        &self,
        context: CallerContext,
        snapshot: Arc<Snapshot>,
    ) -> Result<Arc<dyn ASCII2DSession>, Error> {
        self.preflight(context.clone()).await?;
        let media = validate_snapshot(&snapshot)?;
        let jar = Arc::new(Mutex::new(cookies::Jar::default()));
        match self
            .upload_attempt(context.clone(), snapshot.clone(), media, jar)
            .await
        {
            Ok(session) => Ok(Arc::new(session)),
            Err(AttemptError { error, protected }) => {
                let Some(protected) = protected else {
                    return Err(error);
                };
                let Some(solver) = &self.shared.solver else {
                    return Err(Error::new(
                        ErrorCode::ChallengeRequired,
                        "ascii2d challenge requires solver recovery",
                        Some(error),
                    ));
                };
                let state = self
                    .shared
                    .cache
                    .get(context.clone(), solver.clone(), protected)
                    .await?;
                let user_agent =
                    normalize_agent(&state.user_agent).map_err(|_| solver::malformed())?;
                let client = Self {
                    user_agent,
                    ..self.clone()
                };
                let mut cookies = cookies::Jar::default();
                cookies.clearance(&client.endpoint, &state.clearance);
                let jar = Arc::new(Mutex::new(cookies));
                match client.upload_attempt(context, snapshot, media, jar).await {
                    Ok(session) => Ok(Arc::new(session)),
                    Err(AttemptError {
                        protected: Some(_), ..
                    }) => {
                        self.shared.cache.invalidate();
                        Err(solver::failed())
                    }
                    Err(AttemptError { error, .. }) => Err(error),
                }
            }
        }
    }
    async fn upload_attempt(
        &self,
        context: CallerContext,
        snapshot: Arc<Snapshot>,
        media: &'static str,
        jar: Arc<Mutex<cookies::Jar>>,
    ) -> Result<Session, AttemptError> {
        let mut response = self
            .get(
                context.clone(),
                &self.endpoint_wire,
                self.navigation("none", ""),
                jar.clone(),
                "ascii2d session request failed",
            )
            .await
            .map_err(AttemptError::plain)?;
        let result = async {
            self.response_status(context.clone(), &mut response, 200..300)
                .await
                .map_err(|error| AttemptError::response(error, self.endpoint_wire.clone()))?;
            let bytes = read_body(&mut response).await.map_err(|_| {
                AttemptError::plain(domain(
                    ErrorCode::MalformedUpstreamResponse,
                    "ascii2d returned a malformed response",
                ))
            })?;
            html::upload_form(&bytes).ok_or_else(|| {
                AttemptError::plain(domain(
                    ErrorCode::MalformedUpstreamResponse,
                    "ascii2d returned a malformed response",
                ))
            })
        }
        .await;
        close_response(&mut response).await;
        let token = result?;
        let url = self
            .endpoint
            .join("/search/file")
            .expect("absolute ASCII2D path")
            .to_string();
        let UploadBody {
            reader: body,
            boundary,
            complete,
        } = multipart(snapshot, media, &token, context.clone()).map_err(AttemptError::plain)?;
        let mut headers = self.form();
        headers.insert(
            "Content-Type".into(),
            vec![format!("multipart/form-data; boundary={boundary}")],
        );
        let mut response = self
            .send(
                HttpRequest {
                    context: context.clone(),
                    url: url.clone(),
                    method: "POST".into(),
                    headers,
                    body: Some(body),
                    content_length: 0,
                    logical_host: None,
                },
                jar.clone(),
                "ascii2d upload request failed",
            )
            .await
            .map_err(AttemptError::plain)?;
        let result = async {
            if let Some(error) = context.error() {
                return Err(AttemptError::plain(error.into()));
            }
            if matches!(response.status, 301 | 302 | 303 | 307 | 308)
                && invalid_reference(header(&response.headers, "location"))
            {
                return Err(AttemptError::plain(domain(
                    ErrorCode::ProviderFailed,
                    "ascii2d upload request failed",
                )));
            }
            self.response_status(context, &mut response, 300..400)
                .await
                .map_err(|error| AttemptError::response(error, url))?;
            let hash = self
                .upload_location(header(&response.headers, "location"))
                .map_err(AttemptError::plain)?;
            if !complete.load(Ordering::SeqCst) {
                return Err(AttemptError::plain(domain(
                    ErrorCode::ProviderFailed,
                    "could not upload image to ascii2d",
                )));
            }
            Ok(Session {
                client: self.clone(),
                hash,
                jar,
            })
        }
        .await;
        close_response(&mut response).await;
        result
    }
    async fn send(
        &self,
        mut request: HttpRequest,
        jar: Arc<Mutex<cookies::Jar>>,
        failure: &str,
    ) -> Result<RawResponse, Error> {
        let parsed =
            Url::parse(&request.url).map_err(|_| domain(ErrorCode::ProviderFailed, failure))?;
        let cookie = jar
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .header(&parsed);
        if !cookie.is_empty() {
            request.headers.insert("Cookie".into(), vec![cookie]);
        }
        let context = request.context.clone();
        let response = self.shared.transport.send(request).await;
        match response {
            Ok(Some(response)) => {
                jar.lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .store(&parsed, &response.headers);
                Ok(response)
            }
            _ => Err(context
                .error()
                .map(Error::from)
                .unwrap_or_else(|| domain(ErrorCode::ProviderFailed, failure))),
        }
    }
    async fn get(
        &self,
        context: CallerContext,
        raw: &str,
        mut headers: Headers,
        jar: Arc<Mutex<cookies::Jar>>,
        failure: &str,
    ) -> Result<RawResponse, Error> {
        let mut url = raw.to_owned();
        let fixed_referer = !header(&headers, "referer").is_empty();
        let mut via = Vec::new();
        loop {
            let mut response = self
                .send(
                    HttpRequest {
                        context: context.clone(),
                        url: url.clone(),
                        method: "GET".into(),
                        headers: headers.clone(),
                        body: None,
                        content_length: 0,
                        logical_host: None,
                    },
                    jar.clone(),
                    failure,
                )
                .await?;
            let location = header(&response.headers, "location").to_owned();
            if matches!(response.status, 301 | 302 | 303 | 307 | 308) && !location.is_empty() {
                if invalid_reference(&location) {
                    let _ = read_body(&mut response).await;
                    close_response(&mut response).await;
                    return Err(context
                        .error()
                        .map(Error::from)
                        .unwrap_or_else(|| domain(ErrorCode::ProviderFailed, failure)));
                }
                let current = Url::parse(&url).expect("validated ASCII2D URL");
                let next = current.join(&location);
                let decision = match next {
                    Ok(next) if same_origin(&self.endpoint, &next) => {
                        via.push(url.clone());
                        if let Some(hook) = &self.shared.redirect_hook {
                            hook.check(next.as_str(), &via)
                                .map(|decision| (decision, next))
                        } else if via.len() >= 10 {
                            Err(domain(ErrorCode::ProviderFailed, failure))
                        } else {
                            Ok((RedirectDecision::Follow, next))
                        }
                    }
                    Ok(_) => Err(domain(
                        ErrorCode::MalformedUpstreamResponse,
                        "ascii2d redirected outside its origin",
                    )),
                    Err(_) => Err(domain(ErrorCode::ProviderFailed, failure)),
                };
                match decision {
                    Ok((RedirectDecision::Stop, _)) => return Ok(response),
                    Ok((RedirectDecision::Follow, next)) => {
                        let _ = read_body(&mut response).await;
                        close_response(&mut response).await;
                        if !(fixed_referer
                            || current.scheme() == "https" && next.scheme() == "http")
                        {
                            headers.insert("Referer".into(), vec![url.clone()]);
                        }
                        url = next.to_string();
                        continue;
                    }
                    Err(error) => {
                        let _ = read_body(&mut response).await;
                        close_response(&mut response).await;
                        return Err(if error.code() == ErrorCode::MalformedUpstreamResponse {
                            error
                        } else {
                            context
                                .error()
                                .map(Error::from)
                                .unwrap_or_else(|| domain(ErrorCode::ProviderFailed, failure))
                        });
                    }
                }
            }
            return Ok(response);
        }
    }
    async fn response_status(
        &self,
        context: CallerContext,
        response: &mut RawResponse,
        success: std::ops::Range<u16>,
    ) -> Result<(), Error> {
        if let Some(error) = context.error() {
            return Err(error.into());
        }
        let mut challenged = response
            .headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("cf-mitigated"))
            .flat_map(|(_, values)| values)
            .flat_map(|value| value.split(','))
            .any(|value| value.trim().eq_ignore_ascii_case("challenge"));
        let content = header(&response.headers, "content-type");
        let media = content
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let valid_media = !content.contains('"')
            && content
                .split(';')
                .skip(1)
                .all(|parameter| parameter.trim().contains('='));
        if !challenged
            && response.status == 403
            && valid_media
            && (media == "text/html"
                || media == "application/xhtml+xml"
                || media.ends_with("+html"))
        {
            let bytes = read_body(response).await;
            if let Some(error) = context.error() {
                return Err(error.into());
            }
            challenged = html::challenge(&bytes.map_err(|_| {
                domain(
                    ErrorCode::ProviderFailed,
                    "could not read ascii2d error response",
                )
            })?);
        }
        if let Some(error) = context.error() {
            return Err(error.into());
        }
        if challenged {
            return Err(Error::new(
                ErrorCode::UpstreamHttpStatus,
                "ascii2d challenge detected",
                Some(Error::external(Challenge)),
            ));
        }
        if !success.contains(&response.status) {
            return Err(domain(
                ErrorCode::UpstreamHttpStatus,
                "ascii2d returned an unsuccessful HTTP status",
            ));
        }
        Ok(())
    }
    fn upload_location(&self, raw: &str) -> Result<String, Error> {
        let malformed = || {
            domain(
                ErrorCode::MalformedUpstreamResponse,
                "ascii2d returned a malformed upload location",
            )
        };
        if raw.is_empty() {
            return Err(malformed());
        }
        if raw.starts_with(":") {
            return Err(malformed());
        }
        let mut location = self.endpoint.join(raw).map_err(|_| malformed())?;
        if has_user(raw) || !location.username().is_empty() || location.password().is_some() {
            return Err(malformed());
        }
        if self.endpoint.scheme() == "https"
            && location.scheme() == "http"
            && location.host_str() == self.endpoint.host_str()
        {
            let explicit_port = raw
                .get(..7)
                .filter(|scheme| scheme.eq_ignore_ascii_case("http://"))
                .map(|_| &raw[7..])
                .and_then(|rest| rest.split('/').next())
                .and_then(|authority| authority.rsplit_once(':'))
                .and_then(|(_, port)| port.parse::<u16>().ok());
            let _ = location.set_scheme("https");
            if let Some(port) = explicit_port {
                let _ = location.set_port(Some(port));
            }
        }
        if !same_origin(&self.endpoint, &location) {
            return Err(domain(
                ErrorCode::MalformedUpstreamResponse,
                "ascii2d returned an unsafe upload location",
            ));
        }
        if location.query().is_some_and(|query| !query.is_empty())
            || location
                .fragment()
                .is_some_and(|fragment| !fragment.is_empty())
        {
            return Err(malformed());
        }
        let segments: Vec<_> = location.path().trim_start_matches('/').split('/').collect();
        if segments.len() != 3
            || segments[0] != "search"
            || segments[1] != "color"
            || !valid_hash(segments[2])
        {
            return Err(malformed());
        }
        Ok(segments[2].to_owned())
    }
    fn navigation(&self, site: &str, referer: &str) -> Headers {
        let mut headers = self.base_headers(site);
        headers.insert("Upgrade-Insecure-Requests".into(), vec!["1".into()]);
        if !referer.is_empty() {
            headers.insert("Referer".into(), vec![referer.into()]);
        }
        headers
    }
    fn form(&self) -> Headers {
        let mut headers = self.base_headers("same-origin");
        headers.insert("Origin".into(), vec![self.origin()]);
        headers.insert("Referer".into(), vec![self.home()]);
        headers
    }
    fn base_headers(&self, site: &str) -> Headers {
        let mut headers = Headers::new();
        for (name, value) in [
            ("User-Agent", self.user_agent.as_str()),
            ("Accept", ACCEPT),
            ("Sec-Fetch-Site", site),
            ("Sec-Fetch-Mode", "navigate"),
            ("Sec-Fetch-User", "?1"),
            ("Sec-Fetch-Dest", "document"),
            ("Accept-Encoding", "gzip, deflate, br"),
            ("Accept-Language", "en-US,en;q=0.9"),
        ] {
            headers.insert(name.into(), vec![value.into()]);
        }
        if let Some((version, brand)) = agent_brand(&self.user_agent) {
            let mut value = format!("\"Not(A:Brand\";v=\"24\", \"Chromium\";v=\"{version}\"");
            if brand != "Chromium" {
                value.push_str(&format!(", \"{brand}\";v=\"{version}\""));
            }
            headers.insert("Sec-CH-UA".into(), vec![value]);
            let platform = platform(&self.user_agent);
            headers.insert(
                "Sec-CH-UA-Mobile".into(),
                vec![
                    if matches!(platform, "Android" | "iOS") {
                        "?1"
                    } else {
                        "?0"
                    }
                    .into(),
                ],
            );
            if !platform.is_empty() {
                headers.insert("Sec-CH-UA-Platform".into(), vec![format!("\"{platform}\"")]);
            }
        }
        headers
    }
    fn origin(&self) -> String {
        self.endpoint.origin().ascii_serialization()
    }
    fn home(&self) -> String {
        self.endpoint.to_string()
    }
}
impl ASCII2DClient for Client {
    fn preflight(&self, context: CallerContext) -> ReverseFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            if let Some(error) = context.error() {
                Err(error.into())
            } else {
                Ok(())
            }
        })
    }
    fn upload(
        &self,
        context: CallerContext,
        snapshot: Arc<Snapshot>,
    ) -> ReverseFuture<'_, Result<Arc<dyn ASCII2DSession>, Error>> {
        Box::pin(self.upload_inner(context, snapshot))
    }
    fn close(&self) -> ReverseFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            let future = {
                let mut stored = self
                    .shared
                    .closed
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner());
                stored
                    .get_or_insert_with(|| {
                        let shared = self.shared.clone();
                        let task = tokio::spawn(async move {
                            shared.cache.close();
                            let result = if let Some(solver) = &shared.solver {
                                let result = solver.destroy().await;
                                solver.close_idle();
                                result
                            } else {
                                Ok(())
                            };
                            shared.transport.close_idle_connections();
                            result
                        });
                        let future: ReverseFuture<'static, Result<(), Error>> =
                            Box::pin(async move { task.await.map_err(Error::external)? });
                        future.shared()
                    })
                    .clone()
            };
            future.await
        })
    }
}
impl ASCII2DSession for Session {
    fn search(
        &self,
        context: CallerContext,
        provider: Provider,
    ) -> ReverseFuture<'_, Result<ProviderResponse, Error>> {
        Box::pin(async move {
            if let Some(error) = context.error() {
                return Err(error.into());
            }
            let mode = match provider {
                Provider::Ascii2dColor => "color",
                Provider::Ascii2dBovw => "bovw",
                _ => {
                    return Err(domain(
                        ErrorCode::InvalidRequest,
                        "ascii2d search mode is invalid",
                    ));
                }
            };
            let url = self
                .client
                .endpoint
                .join(&format!("/search/{mode}/{}", self.hash))
                .expect("ASCII2D result path")
                .to_string();
            let mut response = self
                .client
                .get(
                    context.clone(),
                    &url,
                    self.client.navigation("same-origin", &self.client.home()),
                    self.jar.clone(),
                    "ascii2d result request failed",
                )
                .await?;
            let result = async {
                self.client
                    .response_status(context, &mut response, 200..300)
                    .await?;
                let bytes = read_body(&mut response).await.map_err(|_| {
                    domain(
                        ErrorCode::MalformedUpstreamResponse,
                        "ascii2d returned a malformed result page",
                    )
                })?;
                let matches = html::results(&bytes).ok_or_else(|| {
                    domain(
                        ErrorCode::MalformedUpstreamResponse,
                        "ascii2d returned a malformed result page",
                    )
                })?;
                Ok(ProviderResponse {
                    provider,
                    matches,
                    quota: None,
                })
            }
            .await;
            close_response(&mut response).await;
            result
        })
    }
}
#[derive(Debug)]
struct Challenge;
impl std::fmt::Display for Challenge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ascii2d challenge detected")
    }
}
impl std::error::Error for Challenge {}
struct AttemptError {
    error: Error,
    protected: Option<String>,
}
impl AttemptError {
    fn plain(error: Error) -> Self {
        Self {
            error,
            protected: None,
        }
    }
    fn response(error: Error, url: String) -> Self {
        let protected = (error.code() == ErrorCode::UpstreamHttpStatus
            && error.to_string() == "ascii2d challenge detected")
            .then_some(url);
        Self { error, protected }
    }
}
fn domain(code: ErrorCode, message: &str) -> Error {
    Error::new(code, message, None)
}
fn invalid_endpoint() -> Error {
    domain(ErrorCode::InvalidRequest, "ascii2d endpoint is invalid")
}
fn normalize_agent(raw: &str) -> Result<String, Error> {
    if raw.contains(['\r', '\n', '\0']) {
        Err(domain(
            ErrorCode::InvalidRequest,
            "ascii2d user-agent contains invalid header characters",
        ))
    } else {
        Ok(if raw.is_empty() {
            DEFAULT_USER_AGENT
        } else {
            raw
        }
        .to_owned())
    }
}
fn validate_proxy(raw: &str) -> Result<(), Error> {
    if raw.trim().is_empty() {
        return Ok(());
    }
    let url = Url::parse(raw)
        .map_err(|_| domain(ErrorCode::InvalidRequest, "ascii2d proxy URL is invalid"))?;
    if url.host_str().is_none()
        || !matches!(url.scheme(), "http" | "https" | "socks5" | "socks5h")
        || !matches!(url.path(), "" | "/")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(domain(
            ErrorCode::InvalidRequest,
            "ascii2d proxy URL is invalid",
        ));
    }
    Ok(())
}
fn invalid_reference(raw: &str) -> bool {
    if raw
        .chars()
        .any(|character| character < ' ' || character == '\x7f')
    {
        return true;
    }
    let bytes = raw.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'%'
            && (index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit())
        {
            return true;
        }
    }
    let first = raw.split('/').next().unwrap_or("");
    if let Some((scheme, _)) = first.split_once(':') {
        return scheme.is_empty()
            || !scheme.as_bytes()[0].is_ascii_alphabetic()
            || !scheme
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"+-.".contains(&byte));
    }
    false
}
fn has_user(raw: &str) -> bool {
    raw.split_once("://")
        .map(|(_, rest)| rest)
        .or_else(|| raw.strip_prefix("//"))
        .is_some_and(|authority| {
            authority
                .split(['/', '?', '#'])
                .next()
                .unwrap_or("")
                .contains('@')
        })
}
fn valid_hash(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}
pub(super) fn header<'a>(headers: &'a Headers, name: &str) -> &'a str {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .and_then(|(_, values)| values.first())
        .map(String::as_str)
        .unwrap_or("")
}
pub(super) async fn read_body(response: &mut RawResponse) -> Result<Vec<u8>, ()> {
    let mut bytes = Vec::new();
    if let Some(body) = &mut response.body {
        let mut buffer = [0; 8192];
        loop {
            let read = body.read(&mut buffer).await;
            if read.count > buffer.len() {
                return Err(());
            }
            bytes.extend_from_slice(&buffer[..read.count]);
            if read.error.is_some() {
                return Err(());
            }
            if read.eof {
                break;
            }
            if read.count == 0 {
                tokio::task::yield_now().await;
            }
        }
    }
    Ok(bytes)
}
pub(super) async fn close_response(response: &mut RawResponse) {
    if let Some(body) = &mut response.body {
        let _ = body.close().await;
    }
}
fn agent_brand(agent: &str) -> Option<(&str, &str)> {
    for (marker, brand) in [
        ("EdgiOS/", "Microsoft Edge"),
        ("EdgA/", "Microsoft Edge"),
        ("Edg/", "Microsoft Edge"),
        ("OPR/", "Opera"),
        ("CriOS/", "Google Chrome"),
        ("Chrome/", "Google Chrome"),
        ("Chromium/", "Chromium"),
    ] {
        for (at, _) in agent.match_indices(marker) {
            if at != 0
                && !agent[..at].ends_with(|character: char| {
                    character.is_whitespace() || character == '(' || character == ';'
                })
            {
                continue;
            }
            let rest = &agent[at + marker.len()..];
            let end = rest
                .find(|character: char| !character.is_ascii_digit())
                .unwrap_or(rest.len());
            if end > 0 {
                return Some((&rest[..end], brand));
            }
        }
    }
    None
}
fn platform(agent: &str) -> &'static str {
    if agent.contains("Android") {
        "Android"
    } else if ["iPhone", "iPad", "iPod"]
        .iter()
        .any(|marker| agent.contains(marker))
    {
        "iOS"
    } else if agent.contains("Windows") {
        "Windows"
    } else if agent.contains("Macintosh") {
        "macOS"
    } else if agent.contains("Linux") {
        "Linux"
    } else {
        ""
    }
}
fn validate_snapshot(snapshot: &Snapshot) -> Result<&'static str, Error> {
    if snapshot.size() > MAX_IMAGE_BYTES {
        return Err(domain(
            ErrorCode::InvalidSource,
            "ascii2d image exceeds the 10 MB limit",
        ));
    }
    let reader = snapshot.open()?;
    let mut bytes = Vec::with_capacity(512);
    reader.take(512).read_to_end(&mut bytes).map_err(|_| {
        domain(
            ErrorCode::SnapshotFailed,
            "could not inspect image snapshot",
        )
    })?;
    if bytes.starts_with(b"\xff\xd8\xff") {
        Ok("image/jpeg")
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Ok("image/png")
    } else if bytes.len() >= 14 && &bytes[..4] == b"RIFF" && &bytes[8..14] == b"WEBPVP" {
        Ok("image/webp")
    } else {
        Err(domain(
            ErrorCode::InvalidSource,
            "ascii2d supports only JPEG, PNG, or WEBP images",
        ))
    }
}
struct UploadReader {
    reader: Box<dyn Read + Send>,
    context: CallerContext,
    complete: Arc<AtomicBool>,
    remaining: i64,
}
impl Read for UploadReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if let Some(error) = self.context.error() {
            return Err(io::Error::other(error));
        }
        let count = self.reader.read(buffer)?;
        self.remaining -= count as i64;
        if self.remaining == 0 {
            self.complete.store(true, Ordering::SeqCst);
        }
        Ok(count)
    }
}
struct UploadBody {
    reader: Box<dyn Read + Send>,
    boundary: String,
    complete: Arc<AtomicBool>,
}
fn multipart(
    snapshot: Arc<Snapshot>,
    media: &str,
    token: &str,
    context: CallerContext,
) -> Result<UploadBody, Error> {
    let mut random = [0; 30];
    getrandom::fill(&mut random).map_err(|_| {
        domain(
            ErrorCode::ProviderFailed,
            "could not upload image to ascii2d",
        )
    })?;
    let boundary = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let extension = match media {
        "image/jpeg" => ".jpg",
        "image/png" => ".png",
        _ => ".webp",
    };
    let prefix=format!("--{boundary}\r\nContent-Disposition: form-data; name=\"authenticity_token\"\r\n\r\n{token}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"image{extension}\"\r\nContent-Type: {media}\r\n\r\n").into_bytes();
    let suffix = format!("\r\n--{boundary}--\r\n").into_bytes();
    let remaining = prefix.len() as i64 + snapshot.size() + suffix.len() as i64;
    let reader = Cursor::new(prefix)
        .chain(snapshot.open()?)
        .chain(Cursor::new(suffix));
    let complete = Arc::new(AtomicBool::new(false));
    Ok(UploadBody {
        reader: Box::new(UploadReader {
            reader: Box::new(reader),
            context,
            complete: complete.clone(),
            remaining,
        }),
        boundary,
        complete,
    })
}
