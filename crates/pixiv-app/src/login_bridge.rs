use crate::{
    lifecycle::Context,
    login_input::{
        classify_login_input, login_code_from_input, login_input_from_text,
        login_ssh_tunnel_command, pixiv_login_challenge,
    },
    login_page,
};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{
    Request, Response, StatusCode,
    body::{Body, Frame, Incoming, SizeHint},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use pixiv_sdk::oauth::LoginUrl;
use std::{
    convert::Infallible,
    error::Error,
    io,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    task::{Context as TaskContext, Poll},
};
use tokio::{
    net::TcpListener,
    sync::{Mutex as AsyncMutex, Notify, mpsc},
    task::{JoinHandle, JoinSet},
};
use tokio_util::sync::CancellationToken;

pub type BridgeError = Box<dyn Error + Send + Sync>;
pub type CallbackAccepter = Arc<dyn Fn(&str) -> bool + Send + Sync>;
pub type RelayCleanup = Box<dyn FnOnce() + Send>;

pub trait LoginBridgeHooks: Send + Sync + 'static {
    fn diagnostic(&self, message: &str);
    fn open_browser(&self, url: &str) -> Result<(), BridgeError>;
    fn ensure_scheme_relay(&self, _context: &Context) -> Result<(), BridgeError> {
        Ok(())
    }
    fn install_scheme_relay(
        &self,
        _context: &Context,
        _callback: &str,
    ) -> Result<Option<RelayCleanup>, BridgeError> {
        Ok(None)
    }
    fn can_prompt(&self) -> bool {
        false
    }
    fn prompt_input(&self, _message: &str, _default: &str) -> Result<String, BridgeError> {
        Err(io::Error::other("terminal prompt is not configured").into())
    }
    fn output(&self, _message: &str) {}
}

struct InstalledRelay(Option<RelayCleanup>);
impl Drop for InstalledRelay {
    fn drop(&mut self) {
        if let Some(cleanup) = self.0.take() {
            cleanup();
        }
    }
}

struct Shared {
    context: Context,
    accepts: Option<CallbackAccepter>,
    login_url: String,
    challenge: String,
    hooks: Arc<dyn LoginBridgeHooks>,
    diagnostics: Mutex<()>,
    submitted: AtomicBool,
    results: mpsc::Sender<String>,
    finals: AsyncMutex<mpsc::Receiver<bool>>,
    waiter_count: AtomicUsize,
    waiter_changed: Notify,
}
impl Shared {
    fn diagnostic(&self, message: &str) {
        let _lock = self.diagnostics.lock().unwrap_or_else(|e| e.into_inner());
        self.hooks.diagnostic(message);
    }
    fn report_invalid(&self, error: &dyn std::fmt::Display) {
        self.diagnostic(&format!("invalid login submission: {error}\n"));
    }
    fn submit(&self, code: String) {
        if !self.submitted.swap(true, Ordering::SeqCst) {
            let _ = self.results.try_send(code);
        }
    }
    fn accepter(&self) -> Option<&dyn Fn(&str) -> bool> {
        self.accepts.as_deref().map(|f| f as &dyn Fn(&str) -> bool)
    }
    async fn wait_final(self: &Arc<Self>, waiter: Waiter) -> Response<BridgeBody> {
        let ok = tokio::select! {
            _ = self.context.cancelled() => false,
            ok = async { self.finals.lock().await.recv().await.unwrap_or(false) } => ok,
        };
        let mut response = final_page(ok);
        response.body_mut().waiter = Some(waiter);
        response
    }
}

struct Waiter(Arc<Shared>);
impl Waiter {
    fn new(shared: Arc<Shared>) -> Self {
        shared.waiter_count.fetch_add(1, Ordering::SeqCst);
        Self(shared)
    }
}
impl Drop for Waiter {
    fn drop(&mut self) {
        self.0.waiter_count.fetch_sub(1, Ordering::SeqCst);
        self.0.waiter_changed.notify_one();
    }
}

pub(crate) struct BridgeBody {
    inner: Full<Bytes>,
    waiter: Option<Waiter>,
    chunked: bool,
}
impl Body for BridgeBody {
    type Data = Bytes;
    type Error = Infallible;
    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        let frame = Pin::new(&mut self.inner).poll_frame(cx);
        if self.inner.is_end_stream() {
            self.waiter.take();
        }
        frame
    }
    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }
    fn size_hint(&self) -> SizeHint {
        if self.chunked {
            SizeHint::new()
        } else {
            self.inner.size_hint()
        }
    }
}
pub(crate) fn response(
    status: StatusCode,
    content_type: Option<&str>,
    body: Vec<u8>,
) -> Response<BridgeBody> {
    let chunked = body.len() > 2048;
    let mut result = Response::new(BridgeBody {
        inner: Full::new(Bytes::from(body)),
        waiter: None,
        chunked,
    });
    *result.status_mut() = status;
    if let Some(content_type) = content_type {
        result.headers_mut().insert(
            "Content-Type",
            content_type.parse().expect("fixed content type"),
        );
    }
    result
}
pub(crate) fn html(
    status: StatusCode,
    render: impl FnOnce(&mut Vec<u8>) -> io::Result<()>,
) -> Response<BridgeBody> {
    let mut body = Vec::new();
    if render(&mut body).is_err() {
        return http_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not render login page\n",
        );
    }
    response(status, Some("text/html; charset=utf-8"), body)
}
pub(crate) fn final_page(ok: bool) -> Response<BridgeBody> {
    html(
        if ok {
            StatusCode::OK
        } else {
            StatusCode::BAD_REQUEST
        },
        |body| login_page::write_result(body, ok),
    )
}
pub(crate) fn http_error(status: StatusCode, message: &str) -> Response<BridgeBody> {
    let mut result = response(
        status,
        Some("text/plain; charset=utf-8"),
        message.as_bytes().to_vec(),
    );
    result.headers_mut().insert(
        "X-Content-Type-Options",
        "nosniff".parse().expect("fixed header"),
    );
    result
}

fn form_error(raw: &str) -> Option<String> {
    let mut error = None;
    for field in raw.split('&') {
        if field.contains(';') {
            error = Some("invalid semicolon separator in query".to_owned());
            continue;
        }
        let bytes = field.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' {
                if i + 2 >= bytes.len()
                    || !bytes[i + 1].is_ascii_hexdigit()
                    || !bytes[i + 2].is_ascii_hexdigit()
                {
                    if error.is_none() {
                        let end = (i + 3).min(bytes.len());
                        error = Some(format!(
                            "invalid URL escape {}",
                            crate::auth_bundle::go_quote(&String::from_utf8_lossy(&bytes[i..end]))
                        ));
                    }
                    break;
                }
                i += 2;
            }
            i += 1;
        }
    }
    error
}
fn form_value(raw: &str, name: &str) -> Option<String> {
    let url = format!("/?{raw}");
    let parsed = LoginUrl::parse(&url)?;
    // An empty first value still shadows query values in Go's merged form.
    let present = raw.split('&').any(|field| {
        let key = field.split_once('=').map_or(field, |(key, _)| key);
        let encoded = format!("/?key={key}");
        LoginUrl::parse(&encoded).is_some_and(|url| url.query_value("key") == name)
    });
    present.then(|| parsed.query_value(name))
}
fn mime_token(byte: u8) -> bool {
    byte > 32 && byte < 127 && !b"()<>@,;:\\\"/[]?=".contains(&byte)
}
fn take_mime_token(value: &str) -> (&str, &str) {
    let length = value.bytes().take_while(|b| mime_token(*b)).count();
    (&value[..length], &value[length..])
}
fn form_content_type(value: &str) -> Result<bool, String> {
    if value.is_empty() {
        return Ok(false);
    }
    let (base, mut rest) = value
        .split_once(';')
        .map_or((value, ""), |(base, _)| (base, &value[base.len()..]));
    let media = base.trim().to_ascii_lowercase();
    let (first, suffix) = take_mime_token(&media);
    if first.is_empty() {
        return Err("mime: no media type".into());
    }
    if !suffix.is_empty() {
        let suffix = suffix
            .strip_prefix('/')
            .ok_or("mime: expected slash after first token")?;
        let (second, suffix) = take_mime_token(suffix);
        if second.is_empty() {
            return Err("mime: expected token after slash".into());
        }
        if !suffix.is_empty() {
            return Err("mime: unexpected content after media subtype".into());
        }
    }
    let mut parameters = std::collections::HashMap::new();
    while !rest.trim_start().is_empty() {
        rest = rest.trim_start();
        if rest.trim() == ";" {
            break;
        }
        let Some(next) = rest.strip_prefix(';') else {
            return Err("mime: invalid media parameter".into());
        };
        let (key, next) = take_mime_token(next.trim_start());
        if key.is_empty() {
            return Err("mime: invalid media parameter".into());
        }
        let Some(next) = next.trim_start().strip_prefix('=') else {
            return Err("mime: invalid media parameter".into());
        };
        let next = next.trim_start();
        let (parameter, remaining) = if let Some(quoted) = next.strip_prefix('"') {
            let mut out = Vec::new();
            let bytes = quoted.as_bytes();
            let mut i = 0;
            let end = loop {
                if i == bytes.len() || matches!(bytes[i], b'\r' | b'\n') {
                    return Err("mime: invalid media parameter".into());
                }
                if bytes[i] == b'"' {
                    break i + 1;
                }
                if bytes[i] == b'\\'
                    && bytes
                        .get(i + 1)
                        .is_some_and(|b| b"()<>@,;:\\\"/[]?=".contains(b))
                {
                    i += 1;
                }
                out.push(bytes[i]);
                i += 1;
            };
            (String::from_utf8_lossy(&out).into_owned(), &quoted[end..])
        } else {
            let (token, remaining) = take_mime_token(next);
            if token.is_empty() {
                return Err("mime: invalid media parameter".into());
            }
            (token.to_owned(), remaining)
        };
        let key = key.to_ascii_lowercase();
        if let Some(previous) = parameters.insert(key, parameter.clone())
            && previous != parameter
        {
            return Err("mime: duplicate parameter name".into());
        }
        rest = remaining;
    }
    Ok(media == "application/x-www-form-urlencoded")
}

pub(crate) fn clean_path(path: &str) -> String {
    let mut segments = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            value => segments.push(value),
        }
    }
    let mut clean = format!("/{}", segments.join("/"));
    if path.ends_with('/') && clean != "/" {
        clean.push('/');
    }
    clean
}
pub(crate) fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&#34;")
        .replace('\'', "&#39;")
}
async fn handle(
    request: Request<Incoming>,
    shared: Arc<Shared>,
) -> Result<Response<BridgeBody>, Infallible> {
    let method = request.method().clone();
    let uri = request.uri().clone();
    let clean = clean_path(uri.path());
    if method != hyper::Method::CONNECT && clean != uri.path() {
        let location = match uri.query() {
            Some(query) => format!("{clean}?{query}"),
            None => clean,
        };
        let body = if method == hyper::Method::GET {
            format!(
                "<a href=\"{}\">Temporary Redirect</a>.\n\n",
                escape_html(&location)
            )
            .into_bytes()
        } else {
            Vec::new()
        };
        let mut result = response(
            StatusCode::TEMPORARY_REDIRECT,
            Some("text/html; charset=utf-8"),
            body,
        );
        result
            .headers_mut()
            .insert("Location", location.parse().expect("request URI header"));
        return Ok(result);
    }
    let path = LoginUrl::parse(&uri.to_string())
        .map(|u| u.path().to_owned())
        .unwrap_or_else(|| uri.path().to_owned());
    let result = match path.as_str() {
        "/" => html(StatusCode::OK, |body| {
            login_page::write_manual(body, &shared.login_url)
        }),
        "/callback" => {
            if method != hyper::Method::GET {
                http_error(StatusCode::METHOD_NOT_ALLOWED, "method not allowed\n")
            } else if uri.query().unwrap_or_default().is_empty() {
                html(StatusCode::OK, login_page::write_callback_relay)
            } else {
                let host = request
                    .headers()
                    .get("Host")
                    .and_then(|h| h.to_str().ok())
                    .unwrap_or_else(|| uri.authority().map_or("", |a| a.as_str()));
                let callback = format!(
                    "http://{host}{}",
                    uri.path_and_query().map_or("/", |p| p.as_str())
                );
                let result = login_code_from_input(&callback, shared.accepter());
                if let Some(error) = result.error {
                    shared.report_invalid(&error);
                    final_page(false)
                } else {
                    let waiter = Waiter::new(shared.clone());
                    shared.submit(result.code);
                    shared.wait_final(waiter).await
                }
            }
        }
        "/manual" if method == hyper::Method::GET => html(StatusCode::OK, |body| {
            login_page::write_manual(body, &shared.login_url)
        }),
        "/manual" if method == hyper::Method::POST => {
            let content_type = request
                .headers()
                .get("Content-Type")
                .and_then(|h| h.to_str().ok())
                .unwrap_or("");
            let body = match form_content_type(content_type) {
                Err(error) => Err(error),
                Ok(false) => Ok(String::new()),
                Ok(true) => match Limited::new(request.into_body(), 10 << 20).collect().await {
                    Ok(body) => Ok(String::from_utf8_lossy(&body.to_bytes()).into_owned()),
                    Err(error) => Err(if error.is::<http_body_util::LengthLimitError>() {
                        "http: POST too large".into()
                    } else {
                        error.to_string()
                    }),
                },
            };
            let input = body.and_then(|body| {
                if let Some(error) =
                    form_error(&body).or_else(|| form_error(uri.query().unwrap_or_default()))
                {
                    return Err(error);
                }
                Ok(["login_result", "code", "callback_url"]
                    .into_iter()
                    .find_map(|name| {
                        let value = form_value(&body, name)
                            .or_else(|| form_value(uri.query().unwrap_or_default(), name))
                            .unwrap_or_default();
                        (!value.is_empty()).then_some(value)
                    })
                    .unwrap_or_default())
            });
            match input {
                Err(error) => {
                    shared.report_invalid(&error);
                    final_page(false)
                }
                Ok(input) => {
                    let result = classify_login_input(&input, shared.accepter(), &shared.challenge);
                    if let Some(error) = result.result.error {
                        shared.report_invalid(&error);
                        final_page(false)
                    } else if result.relayed {
                        shared.diagnostic("Detected Pixiv authorization relay page; continuing it in the current browser.\n");
                        let mut response = response(StatusCode::SEE_OTHER, None, Vec::new());
                        response.headers_mut().insert(
                            "Location",
                            result.relay_url.parse().expect("validated relay URL"),
                        );
                        response
                    } else {
                        let waiter = Waiter::new(shared.clone());
                        shared.submit(result.result.code);
                        shared.wait_final(waiter).await
                    }
                }
            }
        }
        "/manual" => http_error(StatusCode::METHOD_NOT_ALLOWED, "method not allowed\n"),
        _ => http_error(StatusCode::NOT_FOUND, "404 page not found\n"),
    };
    Ok(result)
}

struct ServerOwnership(tokio::task::AbortHandle);
impl Drop for ServerOwnership {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct FinalNotification {
    sender: mpsc::Sender<bool>,
    sent: bool,
    completed: bool,
}

#[derive(Clone)]
pub struct LoginBridgeServer {
    _ownership: Arc<ServerOwnership>,
    shared: Arc<Shared>,
    final_sender: Arc<AsyncMutex<FinalNotification>>,
    stop: CancellationToken,
    server: Arc<AsyncMutex<Option<JoinHandle<()>>>>,
}
impl LoginBridgeServer {
    pub async fn notify_final(&self, ok: bool) {
        let mut notification = self.final_sender.lock().await;
        if notification.completed {
            return;
        }
        if !notification.sent {
            notification.sent = true;
            let _ = notification.sender.try_send(ok);
        }
        while self.shared.waiter_count.load(Ordering::SeqCst) != 0 {
            self.shared.waiter_changed.notified().await;
        }
        notification.completed = true;
    }

    pub async fn cleanup(&self) {
        let mut server = self.server.lock().await;
        if let Some(task) = server.as_mut() {
            self.stop.cancel();
            let _ = task.await;
            server.take();
        }
    }
}
pub struct LoginBridgeResult {
    pub code: String,
    pub server: LoginBridgeServer,
}

pub(crate) async fn bind_login_listener(address: &str) -> io::Result<TcpListener> {
    let normalized = crate::login_input::split_host_port(address)
        .is_ok_and(|(_, port)| port.is_empty())
        .then(|| format!("{address}0"));
    TcpListener::bind(normalized.as_deref().unwrap_or(address)).await
}

pub async fn wait_for_login_code(
    context: &Context,
    address: &str,
    accepts_callback: Option<CallbackAccepter>,
    login_url: &str,
    no_open: bool,
    hooks: Arc<dyn LoginBridgeHooks>,
) -> Result<LoginBridgeResult, BridgeError> {
    let listener = bind_login_listener(address).await?;
    let actual_address = listener.local_addr()?.to_string();
    let (result_tx, mut results) = mpsc::channel(1);
    let (final_tx, final_rx) = mpsc::channel(1);
    let shared = Arc::new(Shared {
        context: context.clone(),
        accepts: accepts_callback,
        login_url: login_url.to_owned(),
        challenge: pixiv_login_challenge(login_url),
        hooks,
        diagnostics: Mutex::new(()),
        submitted: AtomicBool::new(false),
        results: result_tx,
        finals: AsyncMutex::new(final_rx),
        waiter_count: AtomicUsize::new(0),
        waiter_changed: Notify::new(),
    });
    let mut installed = InstalledRelay(None);
    if !no_open {
        if let Err(error) = shared.hooks.ensure_scheme_relay(context) {
            shared.diagnostic(&format!(
                "warning: persistent pixiv:// callback handler is unavailable: {error}\n"
            ));
        }
        match shared
            .hooks
            .install_scheme_relay(context, &format!("http://{actual_address}/callback"))
        {
            Err(error) => shared.diagnostic(&format!(
                "warning: pixiv:// callback handler is unavailable: {error}\n"
            )),
            Ok(cleanup) => {
                installed.0 = cleanup;
                if installed.0.is_some() {
                    shared.diagnostic(
                        "Registered pixiv:// callback handler for this login attempt.\n",
                    );
                    shared.diagnostic("After confirming the Pixiv account, keep this terminal open while the browser shows the final result.\n");
                }
            }
        }
    }
    let stop = CancellationToken::new();
    let server_stop = stop.clone();
    let server_shared = shared.clone();
    let (serve_tx, mut serve_rx) = mpsc::channel::<io::Error>(1);
    let task = tokio::spawn(async move {
        let mut connections = JoinSet::new();
        loop {
            tokio::select! {
                _ = server_stop.cancelled() => break,
                _ = connections.join_next(), if !connections.is_empty() => {},
                accepted = listener.accept() => match accepted {
                    Err(error) => { let _ = serve_tx.try_send(error); break; },
                    Ok((stream, _)) => {
                        let shared = server_shared.clone();
                        let stop = server_stop.clone();
                        connections.spawn(async move {
                            let connection = http1::Builder::new().serve_connection(TokioIo::new(stream), service_fn(move |request| handle(request, shared.clone())));
                            tokio::pin!(connection);
                            tokio::select! {
                                _ = &mut connection => {},
                                _ = stop.cancelled() => { connection.as_mut().graceful_shutdown(); let _ = connection.await; },
                            }
                        });
                    }
                },
            }
        }
        drop(listener);
        while connections.join_next().await.is_some() {}
    });
    let server = LoginBridgeServer {
        _ownership: Arc::new(ServerOwnership(task.abort_handle())),
        shared: shared.clone(),
        final_sender: Arc::new(AsyncMutex::new(FinalNotification {
            sender: final_tx,
            sent: false,
            completed: false,
        })),
        stop,
        server: Arc::new(AsyncMutex::new(Some(task))),
    };
    shared.diagnostic(&format!("Open this Pixiv login URL:\n{login_url}\n"));
    if no_open {
        shared.diagnostic(
            "Browser opening is disabled; use the manual fallback page or terminal prompt.\n",
        );
        match login_ssh_tunnel_command(&actual_address) {
            Ok(command) => shared.diagnostic(&format!("When this CLI runs on an SSH host, forward its loopback listener from the browser machine:\n  {command}\n")),
            Err(_) => shared.diagnostic("warning: could not generate the SSH tunnel hint for this login listener.\n"),
        }
        shared.diagnostic("An SSH tunnel alone cannot receive Pixiv's final app link; remote browser login requires a desktop handoff.\n");
    } else {
        shared.diagnostic(
            "Complete sign-in in your browser; the local page will receive the result.\n",
        );
    }
    shared.diagnostic(&format!("Manual fallback page: http://{actual_address}/\n"));
    let enable_terminal = shared.hooks.can_prompt();
    if !no_open && let Err(error) = shared.hooks.open_browser(login_url) {
        shared.diagnostic(&format!("warning: could not open browser: {error}\n"));
    }
    if enable_terminal {
        let shared = shared.clone();
        std::thread::spawn(move || {
            loop {
                let Ok(input) = shared.hooks.prompt_input(
                    "Paste the returned Pixiv sign-in address, relay address, or value",
                    "",
                ) else {
                    return;
                };
                shared.hooks.output("\n");
                let mut opener = |url: &str| {
                    shared.diagnostic(
                        "Detected Pixiv authorization relay page; opening Pixiv relay URL once.\n",
                    );
                    shared.hooks.open_browser(url)
                };
                let result = login_input_from_text(
                    &input,
                    shared.accepter(),
                    &shared.challenge,
                    Some(&mut opener),
                );
                if let Some(error) = &result.result.error {
                    shared.report_invalid(error);
                }
                if result.relayed {
                    continue;
                }
                if result.result.error.is_none() {
                    shared.submit(result.result.code);
                    return;
                }
            }
        });
    }
    let result = tokio::select! {
        code = results.recv() => code.ok_or_else(|| io::Error::other("login server stopped before sign-in completed").into()),
        error = serve_rx.recv() => Err(error.unwrap_or_else(|| io::Error::other("login server stopped before sign-in completed")).into()),
        error = context.cancelled() => Err(Box::new(error) as BridgeError),
    };
    match result {
        Ok(code) => {
            drop(installed);
            Ok(LoginBridgeResult { code, server })
        }
        Err(error) => {
            server.cleanup().await;
            drop(installed);
            Err(error)
        }
    }
}
