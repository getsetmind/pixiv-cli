use pixiv_app::handoff_client::{
    HandoffBody, HandoffClient, HandoffFuture, HandoffProxyEnvironment, HandoffRead,
    HandoffRequest, HandoffResponse, HandoffTransport, HandoffTransportError,
    NativeHandoffTransport,
};
use pixiv_app::handoff_protocol::RemoteLoginStart;
use pixiv_app::handoff_state::{ActiveRemoteLogin, HandoffState};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    io,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio_util::sync::CancellationToken;

#[derive(Deserialize)]
struct Case {
    name: String,
    body: String,
    error: String,
    #[serde(default)]
    read_error: bool,
    #[serde(default)]
    data_error: bool,
}
#[derive(Deserialize)]
struct ProxyCase {
    name: String,
    url: String,
    env: BTreeMap<String, String>,
    proxy: String,
    error: String,
}
#[derive(Deserialize)]
struct Fixture {
    authorization_url: String,
    start: Vec<Case>,
    completion: Vec<Case>,
    proxy: Vec<ProxyCase>,
}
fn fixture() -> Fixture {
    serde_json::from_str(include_str!("fixtures/handoff_client.json")).unwrap()
}
fn active() -> ActiveRemoteLogin {
    ActiveRemoteLogin {
        version: 1,
        origin: "https://relay.example/prefix".into(),
        session_id: "session".into(),
        proof: "synthetic".into(),
    }
}
fn start(a: &ActiveRemoteLogin) -> RemoteLoginStart {
    RemoteLoginStart {
        origin: a.origin.clone(),
        session_id: a.session_id.clone(),
        proof: a.proof.clone(),
    }
}
#[derive(Clone)]
struct Body {
    inner: Arc<Mutex<(Vec<u8>, usize)>>,
    reads: Arc<AtomicUsize>,
    closes: Arc<AtomicUsize>,
    read_error: bool,
    data_error: bool,
}
impl Body {
    fn new(c: &Case) -> Self {
        Self {
            inner: Arc::new(Mutex::new((c.body.as_bytes().to_vec(), 0))),
            reads: Arc::default(),
            closes: Arc::default(),
            read_error: c.read_error,
            data_error: c.data_error,
        }
    }
    fn text(s: &str) -> Self {
        Self::new(&Case {
            name: String::new(),
            body: s.into(),
            error: String::new(),
            read_error: false,
            data_error: false,
        })
    }
}
impl HandoffBody for Body {
    fn read<'a>(&'a self, buf: &'a mut [u8]) -> HandoffFuture<'a, HandoffRead> {
        Box::pin(async move {
            self.reads.fetch_add(1, Ordering::SeqCst);
            let mut inner = self.inner.lock().unwrap();
            let count = buf.len().min(inner.0.len() - inner.1);
            buf[..count].copy_from_slice(&inner.0[inner.1..inner.1 + count]);
            inner.1 += count;
            HandoffRead {
                count,
                error: ((count == 0 && self.read_error) || (count > 0 && self.data_error))
                    .then(|| io::Error::other("synthetic read failure")),
            }
        })
    }
    fn close(&self) {
        self.closes.fetch_add(1, Ordering::SeqCst);
    }
}
struct Transport {
    response: Mutex<Option<HandoffResponse>>,
    requests: Arc<Mutex<Vec<HandoffRequest>>>,
    mutation: Option<Box<dyn Fn() + Send + Sync>>,
}
impl HandoffTransport for Transport {
    fn send<'a>(
        &'a self,
        request: HandoffRequest,
        _: CancellationToken,
    ) -> HandoffFuture<'a, Result<HandoffResponse, HandoffTransportError>> {
        Box::pin(async move {
            self.requests.lock().unwrap().push(request);
            if let Some(mutation) = &self.mutation {
                mutation()
            };
            self.response
                .lock()
                .unwrap()
                .take()
                .ok_or(HandoffTransportError)
        })
    }
}
fn transport(
    body: &Body,
    status: u16,
    result: &str,
) -> (Transport, Arc<Mutex<Vec<HandoffRequest>>>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    (
        Transport {
            response: Mutex::new(Some(HandoffResponse {
                status,
                headers: vec![("X-Pixiv-Relay-Result-URL".into(), result.into())],
                body: Box::new(body.clone()),
            })),
            requests: requests.clone(),
            mutation: None,
        },
        requests,
    )
}
fn expected<T, E: std::fmt::Display>(result: Result<T, E>, want: &str, label: &str) -> Option<T> {
    match result {
        Ok(value) => {
            assert_eq!(want, "", "{label}");
            Some(value)
        }
        Err(error) => {
            assert_eq!(error.to_string(), want, "{label}");
            None
        }
    }
}

#[tokio::test]
async fn start_json_matches_frozen_go_contracts() {
    let f = fixture();
    for c in f.start {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let state = HandoffState::new(&path);
        let body = Body::new(&c);
        let (t, _) = transport(&body, 200, "");
        let client = HandoffClient::new(t, HandoffState::new(&path));
        let got = expected(
            client
                .start(&start(&active()), &CancellationToken::new())
                .await,
            &c.error,
            &c.name,
        );
        assert_eq!(body.closes.load(Ordering::SeqCst), 1, "{}", c.name);
        if let Some(url) = got {
            assert_eq!(url, f.authorization_url);
            assert_eq!(state.load().unwrap(), active())
        } else {
            assert_eq!(
                state.load().unwrap_err().to_string(),
                "no active remote login handoff"
            )
        }
    }
}

#[tokio::test]
async fn completion_json_and_close_once_match_frozen_go_contracts() {
    for c in fixture().completion {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let state = HandoffState::new(&path);
        let a = active();
        state.save(&a).unwrap();
        let body = Body::new(&c);
        let (t, _) = transport(&body, 200, &format!("{}/result/YWJj", a.origin));
        let client = HandoffClient::new(t, HandoffState::new(&path));
        let session = client
            .forward_callback(
                "pixiv://account/login?code=synthetic",
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(body.reads.load(Ordering::SeqCst), 0);
        assert_eq!(body.closes.load(Ordering::SeqCst), 0);
        assert_eq!(
            state.load().unwrap_err().to_string(),
            "no active remote login handoff"
        );
        expected(session.complete().await, &c.error, &c.name);
        assert_eq!(body.closes.load(Ordering::SeqCst), 1);
        session.abort();
        assert_eq!(body.closes.load(Ordering::SeqCst), 1);
        expected(
            session.complete().await,
            "remote Pixiv login relay did not return a final result",
            &c.name,
        );
        assert_eq!(body.closes.load(Ordering::SeqCst), 1)
    }
}

#[tokio::test]
async fn exact_request_bytes_method_headers_and_cleaned_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let mut a = active();
    a.session_id = "nested/../session space".into();
    a.proof = "<&>\"\\\n\u{2028}\u{2029}".into();
    let f = fixture();
    let body =
        Body::text(&serde_json::json!({"authorization_url":f.authorization_url}).to_string());
    let (t, requests) = transport(&body, 200, "");
    let client = HandoffClient::new(t, HandoffState::new(&path));
    client
        .start(&start(&a), &CancellationToken::new())
        .await
        .unwrap();
    {
        let requests = requests.lock().unwrap();
        let r = &requests[0];
        assert_eq!(r.method, "POST");
        assert_eq!(
            r.endpoint,
            "https://relay.example/prefix/start/session%20space"
        );
        assert_eq!(
            r.headers,
            [("Content-Type".into(), "application/json".into())]
        );
        assert_eq!(
            r.body,
            br#"{"proof":"\u003c\u0026\u003e\"\\\n\u2028\u2029"}"#
        )
    }
    let body = Body::text(r#"{"success":true}"#);
    let (t, requests) = transport(&body, 200, &format!(" \t{}/result/YWJj\r\n", a.origin));
    let client = HandoffClient::new(t, HandoffState::new(&path));
    let session = client
        .forward_callback(
            " pixiv://account/login?code=synthetic&extra=<tag> ",
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    {
        let requests = requests.lock().unwrap();
        let r = &requests[0];
        assert_eq!(r.method, "POST");
        assert_eq!(
            r.endpoint,
            "https://relay.example/prefix/callback/session%20space"
        );
        assert_eq!(
            r.headers,
            [("Content-Type".into(), "application/json".into())]
        );
        assert_eq!(r.body,br#"{"callback_url":" pixiv://account/login?code=synthetic\u0026extra=\u003ctag\u003e ","proof":"\u003c\u0026\u003e\"\\\n\u2028\u2029"}"#)
    }
    assert_eq!(session.result_url, format!("{}/result/YWJj", a.origin));
    session.complete().await.unwrap();
}

#[tokio::test]
async fn status_headers_and_matching_clear_precede_body_reads() {
    for (name, status, result, mutation, want) in [
        (
            "status",
            201,
            "bad",
            "",
            "remote Pixiv login relay rejected the login result",
        ),
        (
            "header",
            200,
            "https://other.example/result/YWJj",
            "",
            "invalid remote login relay result URL",
        ),
        (
            "clear",
            200,
            "https://relay.example/prefix/result/YWJj",
            "invalid",
            "could not clear active remote login handoff",
        ),
        (
            "newer",
            200,
            "https://relay.example/prefix/result/YWJj",
            "newer",
            "",
        ),
        (
            "missing",
            200,
            "https://relay.example/prefix/result/YWJj",
            "missing",
            "",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let state = HandoffState::new(&path);
        let a = active();
        state.save(&a).unwrap();
        let body = Body::text(r#"{"success":true}"#);
        let (mut t, _) = transport(&body, status, result);
        let mutated_path = path.clone();
        t.mutation = Some(Box::new(move || match mutation {
            "invalid" => std::fs::write(&mutated_path, "{").unwrap(),
            "newer" => {
                let mut newer = active();
                newer.session_id = "newer".into();
                HandoffState::new(&mutated_path).save(&newer).unwrap()
            }
            "missing" => std::fs::remove_file(&mutated_path).unwrap(),
            _ => (),
        }));
        let client = HandoffClient::new(t, HandoffState::new(&path));
        let session = expected(
            client
                .forward_callback(
                    "pixiv://account/login?code=synthetic",
                    &CancellationToken::new(),
                )
                .await,
            want,
            name,
        );
        assert_eq!(body.reads.load(Ordering::SeqCst), 0);
        if let Some(s) = session {
            assert_eq!(body.closes.load(Ordering::SeqCst), 0);
            s.abort();
            s.abort();
            assert_eq!(body.closes.load(Ordering::SeqCst), 1);
            s.complete().await.unwrap();
            assert_eq!(body.closes.load(Ordering::SeqCst), 1)
        } else {
            assert_eq!(body.closes.load(Ordering::SeqCst), 1)
        }
        match mutation {
            "newer" => assert_eq!(state.load().unwrap().session_id, "newer"),
            "missing" => assert_eq!(
                state.load().unwrap_err().to_string(),
                "no active remote login handoff"
            ),
            "invalid" => assert_eq!(
                state.load().unwrap_err().to_string(),
                "active remote login handoff is invalid"
            ),
            _ => assert_eq!(state.load().unwrap(), a),
        }
    }
}

#[test]
fn native_proxy_selection_matches_isolated_go_environment_cases() {
    for c in fixture().proxy {
        let environment = HandoffProxyEnvironment::from_values(&c.env);
        let got = expected(environment.proxy_for_url(&c.url), &c.error, &c.name);
        if let Some(proxy) = got {
            assert_eq!(proxy.unwrap_or_default(), c.proxy, "{}", c.name)
        }
    }
}

#[tokio::test]
async fn deep_trailing_json_keeps_invalid_final_error_phase() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let a = active();
    HandoffState::new(&path).save(&a).unwrap();
    let body = Body::text(&format!(
        "{{\"success\":true}} {}{}",
        "[".repeat(10001),
        "]".repeat(10001)
    ));
    let (t, _) = transport(&body, 200, &format!("{}/result/YWJj", a.origin));
    let client = HandoffClient::new(t, HandoffState::new(&path));
    let session = client
        .forward_callback(
            "pixiv://account/login?code=synthetic",
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    expected(
        session.complete().await,
        "remote Pixiv login relay returned an invalid final result",
        "deep trailing",
    );
    assert_eq!(body.closes.load(Ordering::SeqCst), 1);
}

fn read_http_request(socket: &mut std::net::TcpStream) -> String {
    use std::io::Read;
    socket
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    let mut bytes = Vec::new();
    let mut one = [0];
    while !bytes.ends_with(b"\r\n\r\n") {
        socket.read_exact(&mut one).unwrap();
        bytes.push(one[0]);
    }
    let header = String::from_utf8(bytes).unwrap();
    let length = header
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|s| s.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    let mut body = vec![0; length];
    socket.read_exact(&mut body).unwrap();
    format!("{header}{}", String::from_utf8(body).unwrap())
}
fn native() -> NativeHandoffTransport {
    NativeHandoffTransport::with_proxy_environment(HandoffProxyEnvironment::default())
}

#[tokio::test]
async fn native_response_headers_arrive_before_final_body_eof() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let mut a = active();
    a.origin = origin.clone();
    HandoffState::new(&path).save(&a).unwrap();
    let (release, wait) = std::sync::mpsc::channel();
    let expected_origin = origin.clone();
    let thread = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let request = read_http_request(&mut socket);
        write!(socket,"HTTP/1.1 200 OK\r\nX-Pixiv-Relay-Result-URL: {expected_origin}/result/YWJj\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").unwrap();
        socket.flush().unwrap();
        wait.recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        socket
            .write_all(b"10\r\n{\"success\":true}\r\n0\r\n\r\n")
            .unwrap();
        request
    });
    let client = HandoffClient::new(native(), HandoffState::new(&path));
    let session = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        client.forward_callback(
            "pixiv://account/login?code=synthetic",
            &CancellationToken::new(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(session.result_url, format!("{origin}/result/YWJj"));
    assert_eq!(
        HandoffState::new(&path).load().unwrap_err().to_string(),
        "no active remote login handoff"
    );
    let completion = session.complete();
    tokio::pin!(completion);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), &mut completion)
            .await
            .is_err()
    );
    release.send(()).unwrap();
    completion.await.unwrap();
    let request = thread.join().unwrap();
    assert!(request.starts_with("POST /callback/session HTTP/1.1\r\n"));
    assert!(request.contains("user-agent: Go-http-client/1.1\r\n"));
    assert!(
        request
            .to_ascii_lowercase()
            .contains("content-type: application/json\r\n")
    );
    assert!(request.ends_with(
        r#"{"callback_url":"pixiv://account/login?code=synthetic","proof":"synthetic"}"#
    ));
}

#[tokio::test]
async fn native_abort_and_cancellation_unblock_pending_completion() {
    use std::io::{Read, Write};
    for action in ["abort", "cancel"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let mut a = active();
        a.origin = origin.clone();
        HandoffState::new(&path).save(&a).unwrap();
        let thread = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            read_http_request(&mut socket);
            write!(socket,"HTTP/1.1 200 OK\r\nX-Pixiv-Relay-Result-URL: {origin}/result/YWJj\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").unwrap();
            socket.flush().unwrap();
            let mut byte = [0];
            match socket.read(&mut byte) {
                Ok(0) => (),
                Ok(count) => panic!("unexpected bytes after pending callback: {count}"),
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted
                    ) => {}
                Err(error) => panic!("callback connection did not close: {error}"),
            }
        });
        let client = HandoffClient::new(native(), HandoffState::new(&path));
        let cancellation = CancellationToken::new();
        let session = client
            .forward_callback("pixiv://account/login?code=synthetic", &cancellation)
            .await
            .unwrap();
        let completion = session.complete();
        tokio::pin!(completion);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut completion)
                .await
                .is_err()
        );
        if action == "abort" {
            session.abort()
        } else {
            cancellation.cancel()
        };
        expected(
            tokio::time::timeout(std::time::Duration::from_secs(3), completion)
                .await
                .unwrap(),
            "remote Pixiv login relay did not return a final result",
            action,
        );
        session.abort();
        tokio::task::spawn_blocking(move || thread.join().unwrap())
            .await
            .unwrap();
        assert_eq!(
            HandoffState::new(&path).load().unwrap_err().to_string(),
            "no active remote login handoff"
        );
    }
}

#[tokio::test]
async fn native_redirects_never_replay_proof_or_callback() {
    use std::io::Write;
    for status in [301, 302, 303, 307, 308] {
        for operation in ["start", "callback"] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("state.json");
            let target = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            target.set_nonblocking(true).unwrap();
            let target_url = format!("http://{}/capture", target.local_addr().unwrap());
            let relay = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let mut a = active();
            a.origin = format!("http://{}", relay.local_addr().unwrap());
            let thread = std::thread::spawn(move || {
                let (mut socket, _) = relay.accept().unwrap();
                let request = read_http_request(&mut socket);
                write!(socket,"HTTP/1.1 {status} Redirect\r\nLocation: {target_url}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                request
            });
            let client = HandoffClient::new(native(), HandoffState::new(&path));
            let cancellation = CancellationToken::new();
            if operation == "start" {
                expected(
                    client.start(&start(&a), &cancellation).await,
                    "remote Pixiv login relay rejected login handoff",
                    operation,
                );
            } else {
                HandoffState::new(&path).save(&a).unwrap();
                expected(
                    client
                        .forward_callback("pixiv://account/login?code=synthetic", &cancellation)
                        .await,
                    "remote Pixiv login relay rejected the login result",
                    operation,
                );
                assert_eq!(HandoffState::new(&path).load().unwrap(), a)
            }
            let request = thread.join().unwrap();
            assert!(request.starts_with("POST "));
            assert!(request.contains("synthetic"));
            assert_eq!(
                target.accept().unwrap_err().kind(),
                io::ErrorKind::WouldBlock
            );
        }
    }
}

#[tokio::test]
async fn invalid_input_is_rejected_before_transport_or_state_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let body = Body::text("");
    let (t, requests) = transport(&body, 200, "");
    let client = HandoffClient::new(t, HandoffState::new(&path));
    let a = active();
    for input in [
        RemoteLoginStart {
            origin: "https://user@relay.example".into(),
            ..start(&a)
        },
        RemoteLoginStart {
            session_id: " ".into(),
            ..start(&a)
        },
        RemoteLoginStart {
            proof: "\u{0085}".into(),
            ..start(&a)
        },
    ] {
        expected(
            client.start(&input, &CancellationToken::new()).await,
            "invalid remote login start request",
            "invalid start",
        );
    }
    expected(
        client
            .forward_callback(
                "pixiv://other/login?code=synthetic",
                &CancellationToken::new(),
            )
            .await,
        "this Pixiv login link cannot be used for remote sign-in",
        "invalid callback",
    );
    expected(
        client
            .forward_callback(
                "pixiv://account/login?code=synthetic",
                &CancellationToken::new(),
            )
            .await,
        "no active remote login handoff",
        "no state",
    );
    assert!(requests.lock().unwrap().is_empty());
    assert_eq!(body.reads.load(Ordering::SeqCst), 0);
    assert_eq!(body.closes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn native_cancel_before_request_preserves_active_state() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut a = active();
    a.origin = format!("http://{}", listener.local_addr().unwrap());
    HandoffState::new(&path).save(&a).unwrap();
    let client = HandoffClient::new(native(), HandoffState::new(&path));
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    expected(
        client.start(&start(&a), &cancellation).await,
        "could not contact remote Pixiv login relay",
        "cancel start",
    );
    expected(
        client
            .forward_callback("pixiv://account/login?code=synthetic", &cancellation)
            .await,
        "could not contact remote Pixiv login relay",
        "cancel callback",
    );
    assert_eq!(HandoffState::new(&path).load().unwrap(), a);
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
}

#[tokio::test]
async fn native_start_and_callback_reuse_same_keepalive_connection() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let mut a = active();
    a.origin = origin.clone();
    let auth = fixture().authorization_url;
    let thread = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let start_request = read_http_request(&mut socket);
        let start_body = serde_json::json!({"authorization_url":auth}).to_string();
        write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{start_body}",start_body.len()).unwrap();
        socket.flush().unwrap();
        let callback_request = read_http_request(&mut socket);
        let final_body = r#"{"success":true}"#;
        write!(socket,"HTTP/1.1 200 OK\r\nX-Pixiv-Relay-Result-URL: {origin}/result/YWJj\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{final_body}",final_body.len()).unwrap();
        (start_request, callback_request)
    });
    let client = HandoffClient::new(native(), HandoffState::new(&path));
    let cancellation = CancellationToken::new();
    client.start(&start(&a), &cancellation).await.unwrap();
    let session = client
        .forward_callback("pixiv://account/login?code=synthetic", &cancellation)
        .await
        .unwrap();
    session.complete().await.unwrap();
    let (first, second) = thread.join().unwrap();
    assert!(first.starts_with("POST /start/session HTTP/1.1\r\n"));
    assert!(second.starts_with("POST /callback/session HTTP/1.1\r\n"));
    for request in [first, second] {
        assert!(request.contains("user-agent: Go-http-client/1.1\r\n"));
        assert!(
            request.contains("accept: */*\r\n"),
            "known compatibility gap: native reqwest adds Accept */*; Go omits Accept"
        );
    }
}

#[tokio::test]
async fn native_first_result_header_and_unicode_trim_match_go() {
    use std::io::Write;
    for kind in ["first_invalid_utf8", "unicode_trim"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let mut a = active();
        a.origin = origin.clone();
        HandoffState::new(&path).save(&a).unwrap();
        let thread = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            read_http_request(&mut socket);
            let mut header =
                format!("X-Pixiv-Relay-Result-URL: \u{00a0}{origin}/result/YWJj\u{00a0}\r\n")
                    .into_bytes();
            if kind == "first_invalid_utf8" {
                header = b"X-Pixiv-Relay-Result-URL: \xff\r\n".to_vec();
                header.extend_from_slice(
                    format!("X-Pixiv-Relay-Result-URL: {origin}/result/YWJj\r\n").as_bytes(),
                );
            }
            socket.write_all(b"HTTP/1.1 200 OK\r\n").unwrap();
            socket.write_all(&header).unwrap();
            socket
                .write_all(b"Content-Length: 16\r\nConnection: close\r\n\r\n{\"success\":true}")
                .unwrap();
        });
        let client = HandoffClient::new(native(), HandoffState::new(&path));
        let session = expected(
            client
                .forward_callback(
                    "pixiv://account/login?code=synthetic",
                    &CancellationToken::new(),
                )
                .await,
            if kind == "first_invalid_utf8" {
                "invalid remote login relay result URL"
            } else {
                ""
            },
            kind,
        );
        if let Some(s) = session {
            assert_eq!(s.result_url, format!("{}/result/YWJj", a.origin));
            s.complete().await.unwrap();
            assert_eq!(
                HandoffState::new(&path).load().unwrap_err().to_string(),
                "no active remote login handoff"
            );
        } else {
            assert_eq!(HandoffState::new(&path).load().unwrap(), a)
        }
        tokio::task::spawn_blocking(move || thread.join().unwrap())
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn unavailable_default_session_and_abort_match_go() {
    let session = pixiv_app::handoff_client::RemoteCallbackSession::default();
    session.abort();
    expected(
        session.complete().await,
        "remote Pixiv login relay session is unavailable",
        "default session",
    );
    session.abort();
    expected(
        session.complete().await,
        "remote Pixiv login relay session is unavailable",
        "repeated default session",
    );
}
