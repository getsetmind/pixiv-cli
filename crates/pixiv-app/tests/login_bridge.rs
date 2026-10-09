use chrono::Timelike;
use pixiv_app::{
    lifecycle::Context,
    login_bridge::{
        BridgeError, CallbackAccepter, LoginBridgeHooks, LoginBridgeResult, RelayCleanup,
        wait_for_login_code,
    },
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    io,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::mpsc,
    task::JoinHandle,
};

const LOGIN_URL: &str = "https://app-api.pixiv.net/web/v1/login?state=one&code_challenge=two";
const RELAY_URL: &str = "https://accounts.pixiv.net/post-redirect?return_to=https%3A%2F%2Fapp-api.pixiv.net%2Fweb%2Fv1%2Fusers%2Fauth%2Fpixiv%2Fstart%3Fcode_challenge%3Dtwo";
#[derive(Default)]
struct Hooks {
    log: Mutex<String>,
    events: Arc<Mutex<Vec<String>>>,
    ready: Mutex<Option<mpsc::UnboundedSender<String>>>,
    prompts: Mutex<VecDeque<Result<String, &'static str>>>,
    terminal: bool,
    install: bool,
    fail_hooks: bool,
    output: Mutex<String>,
    restored_listener_closed: Arc<Mutex<Option<bool>>>,
}
impl LoginBridgeHooks for Hooks {
    fn diagnostic(&self, message: &str) {
        self.log.lock().unwrap().push_str(message);
        if let Some(base) = message.strip_prefix("Manual fallback page: ")
            && let Some(ready) = self.ready.lock().unwrap().take()
        {
            ready
                .send(base.trim().trim_end_matches('/').to_owned())
                .unwrap();
        }
    }
    fn open_browser(&self, url: &str) -> Result<(), BridgeError> {
        self.events.lock().unwrap().push(format!("open:{url}"));
        if self.fail_hooks {
            Err(io::Error::other("open unavailable").into())
        } else {
            Ok(())
        }
    }
    fn ensure_scheme_relay(&self, _: &Context) -> Result<(), BridgeError> {
        self.events.lock().unwrap().push("ensure".into());
        if self.fail_hooks {
            Err(io::Error::other("ensure unavailable").into())
        } else {
            Ok(())
        }
    }
    fn install_scheme_relay(
        &self,
        _: &Context,
        url: &str,
    ) -> Result<Option<RelayCleanup>, BridgeError> {
        assert!(url.starts_with("http://127.0.0.1:"));
        assert!(url.ends_with("/callback"));
        self.events.lock().unwrap().push("install".into());
        if self.fail_hooks {
            return Err(io::Error::other("install unavailable").into());
        }
        let events = self.events.clone();
        let probe = self.restored_listener_closed.clone();
        let address = url
            .strip_prefix("http://")
            .unwrap()
            .strip_suffix("/callback")
            .unwrap()
            .to_owned();
        Ok(self.install.then(|| {
            Box::new(move || {
                *probe.lock().unwrap() = Some(std::net::TcpStream::connect(address).is_err());
                events.lock().unwrap().push("restore".into());
            }) as RelayCleanup
        }))
    }

    fn can_prompt(&self) -> bool {
        self.terminal
    }
    fn prompt_input(&self, message: &str, default: &str) -> Result<String, BridgeError> {
        assert_eq!(
            message,
            "Paste the returned Pixiv sign-in address, relay address, or value"
        );
        assert_eq!(default, "");
        self.prompts
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Err("prompt stopped"))
            .map_err(|s| io::Error::other(s).into())
    }
    fn output(&self, value: &str) {
        self.output.lock().unwrap().push_str(value);
    }
}
struct Session {
    context: Context,
    base: String,
    hooks: Arc<Hooks>,
    result: JoinHandle<Result<LoginBridgeResult, BridgeError>>,
}
async fn start(mut hooks: Hooks, accepts: Option<CallbackAccepter>, no_open: bool) -> Session {
    let (tx, mut rx) = mpsc::unbounded_channel();
    *hooks.ready.get_mut().unwrap() = Some(tx);
    let hooks = Arc::new(hooks);
    let context = Context::new();
    let task_context = context.clone();
    let task_hooks = hooks.clone();
    let result = tokio::spawn(async move {
        wait_for_login_code(
            &task_context,
            "127.0.0.1:0",
            accepts,
            LOGIN_URL,
            no_open,
            task_hooks,
        )
        .await
    });
    let base = tokio::time::timeout(Duration::from_secs(3), rx.recv())
        .await
        .unwrap()
        .unwrap();
    Session {
        context,
        base,
        hooks,
        result,
    }
}
async fn take(
    task: JoinHandle<Result<LoginBridgeResult, BridgeError>>,
) -> Result<LoginBridgeResult, BridgeError> {
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap()
}
struct Reply {
    headers_received_at: chrono::DateTime<chrono::Utc>,
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}
impl Reply {
    fn header(&self, name: &str) -> &str {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map_or("", |(_, v)| v)
    }
}
async fn connect(
    base: &str,
    method: &str,
    path: &str,
    body: &str,
    content_type: &str,
) -> TcpStream {
    let addr = base.strip_prefix("http://").unwrap();
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let path = path.split('#').next().unwrap();
    let content_type = if content_type.is_empty() {
        String::new()
    } else {
        format!("Content-Type: {content_type}\r\n")
    };
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n{content_type}Content-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    stream
}
async fn read(mut stream: TcpStream) -> Reply {
    let mut bytes = Vec::new();
    let end = tokio::time::timeout(Duration::from_secs(3), async {
        let mut buffer = [0; 8192];
        loop {
            let length = stream.read(&mut buffer).await.unwrap();
            assert_ne!(length, 0, "connection closed before response headers");
            bytes.extend_from_slice(&buffer[..length]);
            if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                break end;
            }
        }
    })
    .await
    .unwrap();
    let headers_received_at = chrono::Utc::now();
    tokio::time::timeout(Duration::from_secs(3), stream.read_to_end(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    let head = String::from_utf8(bytes[..end].to_vec()).unwrap();
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers: Vec<_> = lines
        .map(|line| {
            let (k, v) = line.split_once(':').unwrap();
            (k.to_owned(), v.trim().to_owned())
        })
        .collect();
    let mut body = bytes[end + 4..].to_vec();
    if headers
        .iter()
        .any(|(k, v)| k.eq_ignore_ascii_case("Transfer-Encoding") && v == "chunked")
    {
        let mut decoded = Vec::new();
        let mut remaining = body.as_slice();
        loop {
            let end = remaining.windows(2).position(|w| w == b"\r\n").unwrap();
            let size =
                usize::from_str_radix(std::str::from_utf8(&remaining[..end]).unwrap(), 16).unwrap();
            if size == 0 {
                break;
            }
            remaining = &remaining[end + 2..];
            decoded.extend_from_slice(&remaining[..size]);
            remaining = &remaining[size + 2..];
        }
        body = decoded;
    }
    Reply {
        headers_received_at,
        status,
        headers,
        body,
    }
}
async fn request(base: &str, method: &str, path: &str, body: &str) -> Reply {
    read(
        connect(
            base,
            method,
            path,
            body,
            "application/x-www-form-urlencoded",
        )
        .await,
    )
    .await
}
fn assert_page(reply: &Reply, kind: &str) {
    match kind {
        "method" => assert_eq!(reply.body, b"method not allowed\n"),
        "notfound" => assert_eq!(reply.body, b"404 page not found\n"),
        "empty" => assert!(reply.body.is_empty()),
        "redirect" => assert_eq!(
            reply.body,
            b"<a href=\"/manual?x=y\">Temporary Redirect</a>.\n\n"
        ),
        _ => {
            let pages: Vec<serde_json::Value> =
                serde_json::from_str(include_str!("fixtures/login_page.json")).unwrap();
            let page = pages
                .iter()
                .find(|page| page["name"] == kind && (kind != "manual" || page["url"] == LOGIN_URL))
                .unwrap();
            assert_eq!(
                format!("{:x}", Sha256::digest(&reply.body)),
                page["sha256"].as_str().unwrap()
            );
            assert_eq!(reply.header("Content-Type"), "text/html; charset=utf-8");
            assert_eq!(reply.header("Transfer-Encoding"), "chunked");
        }
    }
}
#[derive(Deserialize)]
struct Routes {
    routes: Vec<Route>,
    forms: Vec<Form>,
    framing: Vec<Framing>,
    mime: Vec<Mime>,
}
#[derive(Deserialize)]
struct Route {
    name: String,
    method: String,
    path: String,
    body: String,
    status: u16,
    page: String,
    location: String,
}
#[derive(Deserialize)]
struct Form {
    name: String,
    path: String,
    body: String,
    code: String,
    diagnostic: String,
}
#[tokio::test]
async fn routes_and_forms_match_the_frozen_go_protocol() {
    let fixture: Routes =
        serde_json::from_str(include_str!("fixtures/local_login_http.json")).unwrap();
    let session = start(Hooks::default(), Some(Arc::new(|_| false)), true).await;
    for row in fixture.routes {
        let reply = request(&session.base, &row.method, &row.path, &row.body).await;
        assert_eq!(reply.status, row.status, "{}", row.name);
        assert_eq!(reply.header("Location"), row.location, "{}", row.name);
        assert_page(&reply, &row.page);
        if matches!(row.page.as_str(), "method" | "notfound") {
            assert_eq!(reply.header("X-Content-Type-Options"), "nosniff");
        }
    }
    session.context.cancel();
    assert_eq!(
        take(session.result).await.err().unwrap().to_string(),
        "context canceled"
    );
    for row in fixture.forms {
        let session = start(Hooks::default(), Some(Arc::new(|_| true)), true).await;
        if row.code.is_empty() {
            let reply = request(&session.base, "POST", &row.path, &row.body).await;
            assert_eq!(reply.status, 400, "{}", row.name);
            assert_page(&reply, "failure");
            assert!(
                session
                    .hooks
                    .log
                    .lock()
                    .unwrap()
                    .contains(&format!("invalid login submission: {}\n", row.diagnostic)),
                "{}",
                row.name
            );
            assert!(!session.result.is_finished());
            session.context.cancel();
            take(session.result).await.err().unwrap();
        } else {
            let stream = connect(
                &session.base,
                "POST",
                &row.path,
                &row.body,
                "application/x-www-form-urlencoded",
            )
            .await;
            let result = take(session.result).await.unwrap();
            assert_eq!(result.code, row.code, "{}", row.name);
            assert_page(&request(&session.base, "GET", "/", "").await, "manual");
            result.server.notify_final(true).await;
            result.server.notify_final(false).await;
            let reply = read(stream).await;
            assert_eq!(reply.status, 200);
            assert_page(&reply, "success");
            result.server.cleanup().await;
            result.server.cleanup().await;
            assert!(
                TcpStream::connect(session.base.strip_prefix("http://").unwrap())
                    .await
                    .is_err()
            );
        }
    }
}
#[tokio::test]
async fn form_media_type_and_size_errors_keep_the_wait_open() {
    let session = start(Hooks::default(), None, true).await;
    for (content_type, body, diagnostic) in [
        (
            "application/x-www-form-urlencoded;bad",
            "login_result=fake".to_owned(),
            "mime: invalid media parameter",
        ),
        (
            "text/plain",
            "login_result=fake".to_owned(),
            "sign-in result cannot be empty",
        ),
        (
            "application/x-www-form-urlencoded",
            "x".repeat((10 << 20) + 1),
            "http: POST too large",
        ),
    ] {
        let reply =
            read(connect(&session.base, "POST", "/manual", &body, content_type).await).await;
        assert_eq!(reply.status, 400);
        assert_page(&reply, "failure");
        assert!(session.hooks.log.lock().unwrap().contains(diagnostic));
        assert!(!session.result.is_finished());
    }
    session.context.cancel();
    take(session.result).await.err().unwrap();
}
#[tokio::test]
async fn relay_continues_in_the_request_browser_without_submitting() {
    let session = start(Hooks::default(), None, true).await;
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("login_result", RELAY_URL)
        .finish();
    let reply = request(&session.base, "POST", "/manual", &body).await;
    assert_eq!(reply.status, 303);
    assert_eq!(reply.header("Location"), RELAY_URL);
    assert!(reply.body.is_empty());
    assert!(session.hooks.events.lock().unwrap().is_empty());
    assert!(!session.result.is_finished());
    session.context.cancel();
    take(session.result).await.err().unwrap();
}
#[tokio::test]
async fn callback_retains_raw_query_and_only_final_notification_releases_response() {
    let session = start(Hooks::default(), Some(Arc::new(|_| true)), true).await;
    let stream = connect(&session.base, "GET", "/callback?code=%zz#ignored", "", "").await;
    let result = take(session.result).await.unwrap();
    assert_eq!(result.code, format!("{}/callback?code=%zz", session.base));
    let reply = tokio::spawn(read(stream));
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(!reply.is_finished());
    result.server.notify_final(false).await;
    let reply = reply.await.unwrap();
    assert_eq!(reply.status, 400);
    assert_page(&reply, "failure");
    result.server.cleanup().await;
}
#[tokio::test]
async fn duplicates_share_one_final_item_and_disconnect_releases_other_waiter() {
    let session = start(Hooks::default(), None, true).await;
    let first = connect(
        &session.base,
        "POST",
        "/manual",
        "login_result=first",
        "application/x-www-form-urlencoded",
    )
    .await;
    let result = take(session.result).await.unwrap();
    let second = connect(
        &session.base,
        "POST",
        "/manual",
        "login_result=second",
        "application/x-www-form-urlencoded",
    )
    .await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    let server = result.server.clone();
    let notification = tokio::spawn(async move { server.notify_final(true).await });
    let reply = read(first).await;
    assert_page(&reply, "success");
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(!notification.is_finished());
    drop(second);
    tokio::time::timeout(Duration::from_secs(3), notification)
        .await
        .unwrap()
        .unwrap();
    result.server.cleanup().await;
}
#[tokio::test]
async fn context_cancellation_returns_failure_to_pending_final_page() {
    let session = start(Hooks::default(), None, true).await;
    let stream = connect(
        &session.base,
        "POST",
        "/manual",
        "code=synthetic",
        "application/x-www-form-urlencoded",
    )
    .await;
    let result = take(session.result).await.unwrap();
    session.context.cancel();
    let reply = read(stream).await;
    assert_eq!(reply.status, 400);
    assert_page(&reply, "failure");
    result.server.notify_final(true).await;
    result.server.cleanup().await;
}
#[tokio::test]
async fn temporary_scheme_restore_precedes_final_notification_and_server_cleanup() {
    let session = start(
        Hooks {
            install: true,
            ..Hooks::default()
        },
        None,
        false,
    )
    .await;
    let stream = connect(
        &session.base,
        "POST",
        "/manual",
        "code=synthetic",
        "application/x-www-form-urlencoded",
    )
    .await;
    let result = take(session.result).await.unwrap();
    assert_eq!(
        *session.hooks.events.lock().unwrap(),
        ["ensure", "install", &format!("open:{LOGIN_URL}"), "restore"]
    );
    assert_page(&request(&session.base, "GET", "/", "").await, "manual");
    result.server.notify_final(true).await;
    assert_page(&read(stream).await, "success");
    result.server.cleanup().await;
    assert_eq!(
        *session.hooks.restored_listener_closed.lock().unwrap(),
        Some(false)
    );
    let session = start(
        Hooks {
            install: true,
            ..Hooks::default()
        },
        None,
        false,
    )
    .await;
    session.context.cancel();
    assert_eq!(
        take(session.result).await.err().unwrap().to_string(),
        "context canceled"
    );
    assert_eq!(
        *session.hooks.restored_listener_closed.lock().unwrap(),
        Some(true)
    );
    assert_eq!(
        *session.hooks.events.lock().unwrap(),
        ["ensure", "install", &format!("open:{LOGIN_URL}"), "restore"]
    );
}
#[tokio::test]
async fn hook_warnings_and_no_open_terminal_retry_follow_go_order() {
    let session = start(
        Hooks {
            fail_hooks: true,
            ..Hooks::default()
        },
        None,
        false,
    )
    .await;
    let log = session.hooks.log.lock().unwrap().clone();
    let ensure = log.find("warning: persistent").unwrap();
    let install = log.find("warning: pixiv://").unwrap();
    let login = log.find("Open this Pixiv login URL:").unwrap();
    let open = log.find("warning: could not open browser:").unwrap();
    assert!(ensure < install && install < login && login < open);
    session.context.cancel();
    take(session.result).await.err().unwrap();
    let session = start(
        Hooks {
            terminal: true,
            prompts: Mutex::new(VecDeque::from([
                Ok("".into()),
                Ok(RELAY_URL.into()),
                Ok("terminal-value".into()),
            ])),
            ..Hooks::default()
        },
        None,
        true,
    )
    .await;
    let result = take(session.result).await.unwrap();
    assert_eq!(result.code, "terminal-value");
    assert_eq!(
        *session.hooks.events.lock().unwrap(),
        [format!("open:{RELAY_URL}")]
    );
    assert_eq!(*session.hooks.output.lock().unwrap(), "\n\n\n");
    let log = session.hooks.log.lock().unwrap().clone();
    assert!(log.contains("Browser opening is disabled;"));
    assert!(log.contains("invalid login submission: sign-in result cannot be empty\n"));
    assert!(
        log.contains("Detected Pixiv authorization relay page; opening Pixiv relay URL once.\n")
    );
    result.server.notify_final(true).await;
    result.server.cleanup().await;
}
#[tokio::test]
async fn bind_failure_preserves_the_io_error_source_without_running_hooks() {
    let hooks = Arc::new(Hooks::default());
    let result = wait_for_login_code(
        &Context::new(),
        "127.0.0.1:65536",
        None,
        LOGIN_URL,
        false,
        hooks.clone(),
    )
    .await;
    let error = result.err().unwrap();
    assert!(error.downcast_ref::<io::Error>().is_some());
    assert!(hooks.events.lock().unwrap().is_empty());
    assert!(hooks.log.lock().unwrap().is_empty());
}

#[derive(Deserialize)]
struct Framing {
    method: String,
    path: String,
    content_type: String,
    nosniff: String,
    content_length: i64,
    transfer_encoding: Vec<String>,
}
#[derive(Deserialize)]
struct Mime {
    name: String,
    content_type: Option<String>,
    code: String,
    diagnostic: String,
}
#[tokio::test]
async fn response_framing_and_headers_match_frozen_go_rows() {
    let fixture: Routes =
        serde_json::from_str(include_str!("fixtures/local_login_http.json")).unwrap();
    let session = start(Hooks::default(), None, true).await;
    for row in fixture.framing {
        let before = chrono::Utc::now();
        let reply = request(&session.base, &row.method, &row.path, "").await;
        assert_eq!(reply.header("Content-Type"), row.content_type);
        assert_eq!(reply.header("X-Content-Type-Options"), row.nosniff);
        let length = reply.header("Content-Length").parse::<i64>().unwrap_or(-1);
        assert_eq!(length, row.content_length);
        assert_eq!(
            reply.header("Transfer-Encoding"),
            row.transfer_encoding.join(", ")
        );
        let date = chrono::DateTime::parse_from_rfc2822(reply.header("Date"))
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert!(
            date >= before.with_nanosecond(0).unwrap() && date <= reply.headers_received_at,
            "Date={:?} outside {:?} .. {:?}",
            reply.header("Date"),
            before,
            reply.headers_received_at
        );
    }
    session.context.cancel();
    take(session.result).await.err().unwrap();
}
#[tokio::test]
async fn mime_parameters_and_absent_media_type_match_frozen_go_rows() {
    let fixture: Routes =
        serde_json::from_str(include_str!("fixtures/local_login_http.json")).unwrap();
    for row in fixture.mime {
        let session = start(Hooks::default(), None, true).await;
        let stream = connect(
            &session.base,
            "POST",
            "/manual",
            "login_result=fake",
            row.content_type.as_deref().unwrap_or(""),
        )
        .await;
        if row.code.is_empty() {
            let reply = read(stream).await;
            assert_eq!(reply.status, 400, "{}", row.name);
            assert!(
                session.hooks.log.lock().unwrap().contains(&row.diagnostic),
                "{}",
                row.name
            );
            assert!(!session.result.is_finished());
            session.context.cancel();
            take(session.result).await.err().unwrap();
        } else {
            let result = take(session.result).await.unwrap();
            assert_eq!(result.code, row.code, "{}", row.name);
            result.server.notify_final(true).await;
            assert_page(&read(stream).await, "success");
            result.server.cleanup().await;
        }
    }
}
#[tokio::test]
async fn terminal_notification_is_retained_for_a_later_http_waiter() {
    let session = start(
        Hooks {
            terminal: true,
            prompts: Mutex::new(VecDeque::from([Ok("terminal-first".into())])),
            ..Hooks::default()
        },
        None,
        true,
    )
    .await;
    let result = take(session.result).await.unwrap();
    assert_eq!(result.code, "terminal-first");
    result.server.notify_final(true).await;
    result.server.notify_final(false).await;
    let reply = request(&session.base, "POST", "/manual", "code=later").await;
    assert_eq!(reply.status, 200);
    assert_page(&reply, "success");
    result.server.cleanup().await;
}
#[tokio::test]
async fn a_late_waiter_after_final_consumption_waits_until_context_cancellation() {
    let session = start(Hooks::default(), Some(Arc::new(|_| true)), true).await;
    let stream = connect(&session.base, "GET", "/callback?code=first", "", "").await;
    let result = take(session.result).await.unwrap();
    result.server.notify_final(true).await;
    assert_page(&read(stream).await, "success");
    let stream = connect(&session.base, "GET", "/callback?code=late", "", "").await;
    let reply = tokio::spawn(read(stream));
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(!reply.is_finished());
    session.context.cancel();
    let reply = reply.await.unwrap();
    assert_eq!(reply.status, 400);
    assert_page(&reply, "failure");
    result.server.cleanup().await;
}
#[tokio::test]
async fn abandoning_login_wait_closes_owned_listener_without_canceling_caller_context() {
    let session = start(Hooks::default(), Some(Arc::new(|_| false)), true).await;
    let mut pending = TcpStream::connect(session.base.strip_prefix("http://").unwrap())
        .await
        .unwrap();
    pending.write_all(b"POST /manual HTTP/1.1\r\nHost: localhost\r\nContent-Length: 99\r\nContent-Type: application/x-www-form-urlencoded\r\n\r\nlogin_result=").await.unwrap();
    session.result.abort();
    assert!(matches!(session.result.await, Err(error) if error.is_cancelled()));
    let mut byte = [0];
    let closed = tokio::time::timeout(Duration::from_secs(3), pending.read(&mut byte))
        .await
        .unwrap();
    assert!(closed.is_err() || closed.unwrap() == 0);
    assert!(
        TcpStream::connect(session.base.strip_prefix("http://").unwrap())
            .await
            .is_err()
    );
    assert_eq!(session.context.error(), None);
}

#[tokio::test]
async fn interrupted_graceful_cleanup_can_be_retried_after_the_final_page_completes() {
    let session = start(Hooks::default(), None, true).await;
    let stream = connect(
        &session.base,
        "POST",
        "/manual",
        "code=synthetic",
        "application/x-www-form-urlencoded",
    )
    .await;
    let result = take(session.result).await.unwrap();
    let server = result.server.clone();
    let cleanup = tokio::spawn(async move { server.cleanup().await });
    tokio::time::timeout(Duration::from_secs(3), async {
        while TcpStream::connect(session.base.strip_prefix("http://").unwrap())
            .await
            .is_ok()
        {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    assert!(!cleanup.is_finished());
    cleanup.abort();
    assert!(matches!(cleanup.await, Err(error) if error.is_cancelled()));
    assert_eq!(session.context.error(), None);
    result.server.notify_final(true).await;
    assert_page(&read(stream).await, "success");
    tokio::time::timeout(Duration::from_secs(3), result.server.cleanup())
        .await
        .unwrap();
    result.server.cleanup().await;
}

#[tokio::test]
async fn interrupted_notification_retry_waits_without_sending_a_second_final_value() {
    let session = start(Hooks::default(), None, true).await;
    let first = connect(
        &session.base,
        "POST",
        "/manual",
        "code=first",
        "application/x-www-form-urlencoded",
    )
    .await;
    let result = take(session.result).await.unwrap();
    let second = connect(
        &session.base,
        "POST",
        "/manual",
        "code=second",
        "application/x-www-form-urlencoded",
    )
    .await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    let server = result.server.clone();
    let notification = tokio::spawn(async move { server.notify_final(true).await });
    assert_page(&read(first).await, "success");
    assert!(!notification.is_finished());
    notification.abort();
    assert!(matches!(notification.await, Err(error) if error.is_cancelled()));
    let server = result.server.clone();
    let retry = tokio::spawn(async move { server.notify_final(false).await });
    let second_reply = tokio::spawn(read(second));
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(!retry.is_finished());
    assert!(!second_reply.is_finished());
    session.context.cancel();
    let second_reply = second_reply.await.unwrap();
    assert_eq!(second_reply.status, 400);
    assert_page(&second_reply, "failure");
    tokio::time::timeout(Duration::from_secs(3), retry)
        .await
        .unwrap()
        .unwrap();
    result.server.notify_final(false).await;
    result.server.cleanup().await;
}
