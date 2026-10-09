use super::{RelayLoginResult, RelayServerOptions, SessionCapabilities, handoff_relay_deep_link};
use crate::{
    handoff_client::{go_trim, json},
    handoff_protocol::{self, RELAY_RESULT_URL_HEADER},
    lifecycle::Context,
    login_bridge::{self, BridgeBody, BridgeError, CallbackAccepter, LoginBridgeHooks},
    login_input::login_code_from_input,
};
use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::{
    Request, Response, StatusCode,
    body::{Body, Frame, Incoming, SizeHint},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
use std::{
    convert::Infallible,
    future::Future,
    io,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context as TaskContext, Poll},
};
use subtle::ConstantTimeEq;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{Mutex as AsyncMutex, mpsc, watch},
    task::{JoinHandle, JoinSet},
};
use tokio_util::sync::CancellationToken;

const SERVE_ERROR: &str =
    "remote login relay server failed; verify its listener and TLS configuration";

struct SessionState {
    started: bool,
    submitted: bool,
}
struct Shared {
    context: Context,
    accepts: Option<CallbackAccepter>,
    login_url: String,
    session_id: String,
    proof: String,
    result_id: String,
    result_url: String,
    start_url: String,
    state: Mutex<SessionState>,
    results: mpsc::Sender<String>,
    final_ready: watch::Sender<Option<bool>>,
    page_finished: watch::Sender<bool>,
    final_page_written: watch::Sender<bool>,
    claimed: AtomicBool,
}
impl Shared {
    fn abandon(&self) {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.submitted && !self.claimed.swap(true, Ordering::SeqCst) {
            self.page_finished.send_replace(true);
        }
    }
    fn submit(&self, callback: &str) -> Result<(), &'static str> {
        if !handoff_protocol::is_allowed_pixiv_callback_url(callback) {
            return Err("invalid Pixiv login result");
        }
        let result = login_code_from_input(
            callback,
            self.accepts.as_deref().map(|f| f as &dyn Fn(&str) -> bool),
        );
        if result.error.is_some() {
            return Err("login result does not match this session");
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.started {
            return Err("remote login session is not ready");
        }
        if state.submitted {
            return Err("login result has already been received");
        }
        state.submitted = true;
        self.page_finished.send_replace(false);
        let _ = self.results.try_send(result.code);
        Ok(())
    }
}
struct PageGuard(Arc<Shared>);
impl Drop for PageGuard {
    fn drop(&mut self) {
        self.0.page_finished.send_replace(true);
    }
}
struct CallbackGuard(Arc<Shared>);
impl Drop for CallbackGuard {
    fn drop(&mut self) {
        self.0.abandon();
    }
}

type FinalBodyFuture = Pin<Box<dyn Future<Output = Option<Bytes>> + Send>>;
enum RelayBody {
    Fixed {
        inner: BridgeBody,
        page: Option<PageGuard>,
    },
    Callback {
        future: FinalBodyFuture,
        _guard: CallbackGuard,
        ended: bool,
    },
}
impl Body for RelayBody {
    type Data = Bytes;
    type Error = Infallible;
    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        match &mut *self {
            Self::Fixed { inner, page } => {
                let frame = Pin::new(&mut *inner).poll_frame(cx);
                if inner.is_end_stream() {
                    page.take();
                }
                frame
            }
            Self::Callback { future, ended, .. } => {
                if *ended {
                    return Poll::Ready(None);
                }
                match future.as_mut().poll(cx) {
                    Poll::Pending => Poll::Pending,
                    Poll::Ready(value) => {
                        *ended = true;
                        Poll::Ready(value.map(|bytes| Ok(Frame::data(bytes))))
                    }
                }
            }
        }
    }
    fn is_end_stream(&self) -> bool {
        match self {
            Self::Fixed { inner, .. } => inner.is_end_stream(),
            Self::Callback { ended, .. } => *ended,
        }
    }
    fn size_hint(&self) -> SizeHint {
        match self {
            Self::Fixed { inner, .. } => inner.size_hint(),
            Self::Callback { .. } => SizeHint::new(),
        }
    }
}
fn fixed(response: Response<BridgeBody>) -> Response<RelayBody> {
    response.map(|inner| RelayBody::Fixed { inner, page: None })
}
fn error(status: StatusCode, message: &str) -> Response<RelayBody> {
    fixed(login_bridge::http_error(status, message))
}
async fn until_true(mut receiver: watch::Receiver<bool>) {
    loop {
        if *receiver.borrow_and_update() {
            return;
        }
        if receiver.changed().await.is_err() {
            return;
        }
    }
}
async fn until_final(mut receiver: watch::Receiver<Option<bool>>) -> bool {
    loop {
        if let Some(value) = *receiver.borrow_and_update() {
            return value;
        }
        if receiver.changed().await.is_err() {
            return false;
        }
    }
}
async fn handle(
    request: Request<Incoming>,
    shared: Arc<Shared>,
) -> Result<Response<RelayBody>, Infallible> {
    let method = request.method().clone();
    let uri = request.uri().clone();
    let mut clean = if method == hyper::Method::CONNECT {
        uri.path().to_owned()
    } else {
        login_bridge::clean_path(uri.path())
    };
    if ["/session", "/start", "/callback", "/result"].contains(&clean.as_str()) {
        clean.push('/');
    }
    if clean != uri.path() {
        let location = match uri.query() {
            Some(query) => format!("{clean}?{query}"),
            None => clean,
        };
        let body = if method == hyper::Method::GET {
            format!(
                "<a href=\"{}\">Temporary Redirect</a>.\n\n",
                login_bridge::escape_html(&location)
            )
            .into_bytes()
        } else {
            Vec::new()
        };
        let mut response = login_bridge::response(
            StatusCode::TEMPORARY_REDIRECT,
            Some("text/html; charset=utf-8"),
            body,
        );
        response
            .headers_mut()
            .insert("Location", location.parse().expect("request URI header"));
        return Ok(fixed(response));
    }
    let path = pixiv_sdk::oauth::LoginUrl::parse(&uri.to_string())
        .map(|url| url.path().to_owned())
        .unwrap_or_else(|| uri.path().to_owned());
    if method == hyper::Method::GET && path == format!("/session/{}", shared.session_id) {
        let mut response = login_bridge::response(StatusCode::SEE_OTHER, None, Vec::new());
        response.headers_mut().insert(
            "Location",
            shared.start_url.parse().expect("generated deep link"),
        );
        return Ok(fixed(response));
    }
    if method == hyper::Method::GET && path == format!("/result/{}", shared.result_id) {
        if shared.claimed.swap(true, Ordering::SeqCst) {
            return Ok(error(
                StatusCode::CONFLICT,
                "login result has already been opened\n",
            ));
        }
        let page = PageGuard(shared.clone());
        let ok = tokio::select! { ok = until_final(shared.final_ready.subscribe()) => ok, _ = shared.context.cancelled() => false };
        let response = login_bridge::final_page(ok).map(|inner| RelayBody::Fixed {
            inner,
            page: Some(page),
        });
        return Ok(response);
    }
    let start = method == hyper::Method::POST && path == format!("/start/{}", shared.session_id);
    let callback =
        method == hyper::Method::POST && path == format!("/callback/{}", shared.session_id);
    if !start && !callback {
        return Ok(error(StatusCode::NOT_FOUND, "404 page not found\n"));
    }
    let values = match request.into_body().collect().await {
        Ok(body) => {
            let normalized = crate::auth_bundle::normalize_strings(&body.to_bytes());
            crate::handoff_state::validate_json_depth(&normalized)
                .map_err(|_| ())
                .and_then(|()| {
                    json::strings(
                        &normalized,
                        if start {
                            &["proof"]
                        } else {
                            &["proof", "callback_url"]
                        },
                    )
                })
        }
        Err(_) => Err(()),
    };
    let Ok(values) = values else {
        return Ok(error(
            StatusCode::UNAUTHORIZED,
            "invalid remote login session\n",
        ));
    };
    let proof = go_trim(&values[0]);
    if proof.is_empty() || !bool::from(proof.as_bytes().ct_eq(shared.proof.as_bytes())) {
        return Ok(error(
            StatusCode::UNAUTHORIZED,
            "invalid remote login session\n",
        ));
    }
    if start {
        let submitted = {
            let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            state.started = true;
            state.submitted
        };
        if submitted {
            return Ok(error(
                StatusCode::CONFLICT,
                "login result has already been received\n",
            ));
        }
        let json = serde_json::json!({"authorization_url": shared.login_url});
        let body = format!(
            "{}\n",
            crate::auth_bundle::escape_json_html(json.to_string())
        )
        .into_bytes();
        return Ok(fixed(login_bridge::response(
            StatusCode::OK,
            Some("application/json"),
            body,
        )));
    }
    if let Err(message) = shared.submit(&values[1]) {
        let status = if [
            "login result has already been received",
            "remote login session is not ready",
        ]
        .contains(&message)
        {
            StatusCode::CONFLICT
        } else {
            StatusCode::BAD_REQUEST
        };
        return Ok(error(status, &format!("{message}\n")));
    }
    let waiter = shared.clone();
    let future = Box::pin(async move {
        tokio::select! {
            _ = until_true(waiter.final_page_written.subscribe()) => {
                let success = waiter.final_ready.borrow().unwrap_or(false);
                Some(Bytes::from(format!("{{\"success\":{success}}}\n")))
            }
            _ = waiter.context.cancelled() => { waiter.abandon(); None }
        }
    });
    let mut response = Response::new(RelayBody::Callback {
        future,
        _guard: CallbackGuard(shared.clone()),
        ended: false,
    });
    response.headers_mut().insert(
        "Content-Type",
        "application/json".parse().expect("fixed header"),
    );
    response.headers_mut().insert(
        RELAY_RESULT_URL_HEADER,
        shared.result_url.parse().expect("generated result URL"),
    );
    Ok(response)
}

struct Ownership(tokio::task::AbortHandle);
impl Drop for Ownership {
    fn drop(&mut self) {
        self.0.abort();
    }
}
#[derive(Clone)]
pub struct RelayLoginServer {
    _ownership: Arc<Ownership>,
    shared: Arc<Shared>,
    notification: Arc<AsyncMutex<()>>,
    stop: CancellationToken,
    abort: CancellationToken,
    watcher_stop: CancellationToken,
    task: Arc<AsyncMutex<Option<JoinHandle<()>>>>,
}
impl RelayLoginServer {
    pub async fn notify_final(&self, ok: bool) {
        let _notification = self.notification.lock().await;
        if self.shared.final_ready.borrow().is_none() {
            self.shared.final_ready.send_replace(Some(ok));
        }
        until_true(self.shared.page_finished.subscribe()).await;
        self.shared.final_page_written.send_replace(true);
    }
    pub async fn cleanup(&self) {
        self.watcher_stop.cancel();
        self.stop.cancel();
        let mut task = self.task.lock().await;
        if let Some(running) = task.as_mut() {
            let _ = running.await;
            task.take();
        }
    }
    pub async fn abort(&self) {
        self.watcher_stop.cancel();
        self.abort.cancel();
        self.stop.cancel();
        let mut task = self.task.lock().await;
        if let Some(running) = task.as_mut() {
            let _ = running.await;
            task.take();
        }
    }
}
fn tls_acceptor(
    options: &RelayServerOptions,
) -> Result<Option<tokio_rustls::TlsAcceptor>, BridgeError> {
    if options.tls_cert_file.is_empty() {
        return Ok(None);
    }
    let certificates =
        CertificateDer::pem_file_iter(&options.tls_cert_file)?.collect::<Result<Vec<_>, _>>()?;
    let key = PrivateKeyDer::from_pem_file(&options.tls_key_file)?;
    let provider = rustls::crypto::aws_lc_rs::default_provider();
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(provider))
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(certificates, key)?;
    Ok(Some(tokio_rustls::TlsAcceptor::from(Arc::new(config))))
}
async fn serve_connection<S>(
    stream: S,
    shared: Arc<Shared>,
    stop: CancellationToken,
    abort: CancellationToken,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let connection = http1::Builder::new().serve_connection(
        TokioIo::new(stream),
        service_fn(move |request| handle(request, shared.clone())),
    );
    tokio::pin!(connection);
    tokio::select! {
        _ = &mut connection => {},
        _ = abort.cancelled() => {},
        _ = stop.cancelled() => { connection.as_mut().graceful_shutdown(); tokio::select! { _ = &mut connection => {}, _ = abort.cancelled() => {} } },
    }
}
async fn accept_connection(
    stream: TcpStream,
    tls: Option<tokio_rustls::TlsAcceptor>,
    shared: Arc<Shared>,
    stop: CancellationToken,
    abort: CancellationToken,
) {
    if let Some(tls) = tls {
        let stream = tokio::select! { stream = tls.accept(stream) => stream, _ = stop.cancelled() => return, _ = abort.cancelled() => return };
        if let Ok(stream) = stream {
            serve_connection(stream, shared, stop, abort).await;
        }
    } else {
        serve_connection(stream, shared, stop, abort).await;
    }
}

pub(super) async fn wait(
    context: &Context,
    options: RelayServerOptions,
    accepts: Option<CallbackAccepter>,
    login_url: &str,
    hooks: Arc<dyn LoginBridgeHooks>,
    listener: TcpListener,
    capabilities: SessionCapabilities,
) -> Result<RelayLoginResult, BridgeError> {
    let SessionCapabilities {
        public_url,
        session_id,
        proof,
        result_id,
    } = capabilities;
    let actual_address = listener.local_addr()?.to_string();
    let session_url = handoff_protocol::relay_endpoint_url(&public_url, "session", &session_id)?;
    let result_url = handoff_protocol::relay_endpoint_url(&public_url, "result", &result_id)?;
    let start_url = handoff_relay_deep_link(&public_url, &session_id, &proof);
    let (results_tx, mut results_rx) = mpsc::channel(1);
    let shared = Arc::new(Shared {
        context: context.clone(),
        accepts,
        login_url: login_url.to_owned(),
        session_id,
        proof,
        result_id,
        result_url,
        start_url,
        state: Mutex::new(SessionState {
            started: false,
            submitted: false,
        }),
        results: results_tx,
        final_ready: watch::channel(None).0,
        page_finished: watch::channel(true).0,
        final_page_written: watch::channel(false).0,
        claimed: AtomicBool::new(false),
    });
    let stop = CancellationToken::new();
    let abort = CancellationToken::new();
    let watcher_stop = CancellationToken::new();
    let (serve_tx, mut serve_rx) = mpsc::channel::<bool>(1);
    let server_shared = shared.clone();
    let server_stop = stop.clone();
    let server_abort = abort.clone();
    let server_watcher_stop = watcher_stop.clone();
    let task = tokio::spawn(async move {
        let tls = match tls_acceptor(&options) {
            Ok(tls) => tls,
            Err(_) => {
                let _ = serve_tx.try_send(true);
                return;
            }
        };
        let mut connections = JoinSet::new();
        let mut watching = true;
        loop {
            tokio::select! {
                _ = server_stop.cancelled() => break,
                _ = server_watcher_stop.cancelled(), if watching => watching = false,
                _ = server_shared.context.cancelled(), if watching => { server_shared.abandon(); watching = false; },
                _ = connections.join_next(), if !connections.is_empty() => {},
                accepted = listener.accept() => match accepted {
                    Err(_) => { let _ = serve_tx.try_send(true); break; },
                    Ok((stream, _)) => { connections.spawn(accept_connection(stream, tls.clone(), server_shared.clone(), server_stop.clone(), server_abort.clone())); },
                },
            }
        }
        drop(listener);
        while connections.join_next().await.is_some() {}
    });
    let server = RelayLoginServer {
        _ownership: Arc::new(Ownership(task.abort_handle())),
        shared,
        notification: Arc::new(AsyncMutex::new(())),
        stop,
        abort,
        watcher_stop,
        task: Arc::new(AsyncMutex::new(Some(task))),
    };
    hooks.diagnostic(&format!(
        "Remote Pixiv login relay is listening on {actual_address}.\n"
    ));
    hooks.diagnostic(&format!(
        "Open remote Pixiv login session:\n{session_url}\n"
    ));
    tokio::select! {
        code = results_rx.recv() => match code {
            Some(code) => { server.watcher_stop.cancel(); Ok(RelayLoginResult { code, server }) },
            None => { server.cleanup().await; Err(io::Error::other("remote login relay stopped before sign-in completed").into()) },
        },
        failed = serve_rx.recv() => { server.cleanup().await; Err(io::Error::other(if failed == Some(true) { SERVE_ERROR } else { "remote login relay stopped before sign-in completed" }).into()) },
        error = context.cancelled() => { server.abort().await; Err(Box::new(error) as BridgeError) },
    }
}
