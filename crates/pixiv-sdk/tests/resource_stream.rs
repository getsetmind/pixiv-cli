use pixiv_sdk::{
    Error, Reason,
    resource::ResourceHeaders,
    transport::{HttpTransport, ResourceReadRequest, ResourceTransport},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};
use tokio::io::AsyncReadExt;

#[derive(Clone, Deserialize)]
struct Case {
    name: String,
    method: String,
    status: u16,
    redirect: bool,
    blocked: bool,
    #[serde(rename = "loop")]
    looping: bool,
    partial: bool,
    result_status: i64,
    body: String,
    read_failed: bool,
    failure: String,
    headers: Option<ResourceHeaders>,
    requests: Vec<Value>,
    validated: Vec<String>,
}

fn read_head(stream: &mut TcpStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut bytes = Vec::new();
    while !bytes.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        stream.read_exact(&mut byte).unwrap();
        bytes.push(byte[0]);
        assert!(bytes.len() < 65536);
    }
    String::from_utf8(bytes).unwrap()
}

fn request(stream: &mut TcpStream) -> Value {
    let text = read_head(stream);
    let mut lines = text.split("\r\n");
    let mut first = lines.next().unwrap().split_whitespace();
    let method = first.next().unwrap();
    let path = first.next().unwrap();
    let headers: Vec<_> = lines.filter_map(|line| line.split_once(':')).collect();
    let header = |name: &str| {
        headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.trim())
            .unwrap_or_default()
    };
    json!({"method": method, "path": path,
        "referer": header("referer"), "user_agent": header("user-agent"),
        "range": header("range"), "if_none_match": header("if-none-match"),
        "cookie": header("cookie"), "authorization": header("authorization")})
}

fn read_request(url: String, method: String) -> ResourceReadRequest {
    ResourceReadRequest {
        url,
        method,
        operation: "OpenResource",
        headers: ResourceHeaders::from([
            ("Range".into(), vec!["bytes=0-2".into()]),
            ("If-None-Match".into(), vec!["\"fixture-match\"".into()]),
            ("Referer".into(), vec!["fixture-override".into()]),
            ("User-Agent".into(), vec!["fixture-override".into()]),
        ]),
        validate: None,
    }
}

#[tokio::test]
async fn resource_http_streams_preserve_frozen_go_behavior() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/resource-stream.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 15);
    for case in cases {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/start", listener.local_addr().unwrap());
        let server_case = case.clone();
        let server = thread::spawn(move || {
            let mut requests = Vec::new();
            for _ in 0..server_case.requests.len() {
                let (mut stream, _) = listener.accept().unwrap();
                let seen = request(&mut stream);
                let redirect =
                    server_case.redirect && (seen["path"] == "/start" || server_case.looping);
                let status = if server_case.redirect && !redirect {
                    200
                } else {
                    server_case.status
                };
                let mut headers = String::new();
                let body = if redirect {
                    let target = if server_case.blocked {
                        "/blocked"
                    } else if server_case.looping {
                        "/start"
                    } else {
                        "/target"
                    };
                    headers.push_str(&format!("Location: {target}\r\nContent-Length: 0\r\n"));
                    ""
                } else {
                    if status != 304 {
                        headers.push_str("Content-Type: application/octet-stream\r\n");
                    }
                    headers.push_str("ETag: \"fixture\"\r\nSet-Cookie: fixture-cookie-secret\r\nX-Internal: fixture-internal-secret\r\n");
                    if status != 204 && status != 304 {
                        headers.push_str(if server_case.partial {
                            "Content-Length: 99\r\n"
                        } else {
                            "Content-Length: 3\r\n"
                        });
                    }
                    if status == 204 || status == 304 || seen["method"] == "HEAD" {
                        ""
                    } else {
                        "abc"
                    }
                };
                write!(
                    stream,
                    "HTTP/1.1 {status} Fixture\r\nConnection: close\r\n{headers}\r\n{body}"
                )
                .unwrap();
                requests.push(seen);
            }
            requests
        });
        let validated = Arc::new(Mutex::new(Vec::new()));
        let recorded = validated.clone();
        let mut input = read_request(url, case.method.clone());
        input.validate = Some(Arc::new(move |raw| {
            let url = url::Url::parse(raw).unwrap();
            recorded.lock().unwrap().push(url.path().to_owned());
            if url.path() == "/blocked" {
                Err(Error::new(Reason::ResourceForbidden, "OpenResource")
                    .with_detail("fixture-policy-secret"))
            } else {
                Ok(())
            }
        }));
        let result = HttpTransport::new(None).unwrap().open_resource(input).await;
        if case.failure.is_empty() {
            let mut response = result.unwrap();
            assert_eq!(response.status_code, case.result_status, "{}", case.name);
            assert_eq!(response.header(), case.headers.unwrap(), "{}", case.name);
            let mut body = Vec::new();
            let read = response.body.read_to_end(&mut body).await;
            assert_eq!(body, case.body.as_bytes(), "{}", case.name);
            assert_eq!(read.is_err(), case.read_failed, "{}", case.name);
            if let Err(error) = read {
                assert!(!error.to_string().contains("127.0.0.1"));
            }
        } else {
            let error = result.unwrap_err();
            assert_eq!(error.code, Reason::UpstreamUnavailable, "{}", case.name);
            assert!(!format!("{error:?}").contains("fixture-policy-secret"));
        }
        assert_eq!(server.join().unwrap(), case.requests, "{}", case.name);
        assert_eq!(*validated.lock().unwrap(), case.validated, "{}", case.name);
    }
}

#[tokio::test]
async fn resource_open_returns_before_the_server_releases_the_body() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/start", listener.local_addr().unwrap());
    let (release, wait) = mpsc::channel();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        request(&mut stream);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\n")
            .unwrap();
        stream.flush().unwrap();
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
        stream.write_all(b"abcdef").unwrap();
    });
    let transport = HttpTransport::new(None).unwrap();
    let mut response = tokio::time::timeout(
        Duration::from_secs(2),
        transport.open_resource(read_request(url, String::new())),
    )
    .await
    .unwrap()
    .unwrap();
    release.send(()).unwrap();
    let mut body = String::new();
    response.body.read_to_string(&mut body).await.unwrap();
    assert_eq!(body, "abcdef");
    server.join().unwrap();
}

#[tokio::test]
async fn redirect_headers_preserve_the_go_host_trust_boundary() {
    #[derive(Deserialize)]
    struct HeadersCase {
        cross_host: bool,
        headers: ResourceHeaders,
    }
    let cases: Vec<HeadersCase> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/resource-redirect-headers.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 2);
    for case in cases {
        let initial = TcpListener::bind("127.0.0.1:0").unwrap();
        let target = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/start", initial.local_addr().unwrap());
        let host = if case.cross_host {
            "localhost"
        } else {
            "127.0.0.1"
        };
        let destination = format!(
            "http://{host}:{}/target",
            target.local_addr().unwrap().port()
        );
        let server = thread::spawn(move || {
            let (mut stream, _) = initial.accept().unwrap();
            read_head(&mut stream);
            write!(stream, "HTTP/1.1 302 Found\r\nLocation: {destination}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            drop(stream);
            let (mut stream, _) = target.accept().unwrap();
            let text = read_head(&mut stream);
            let mut seen = ResourceHeaders::new();
            for (key, value) in text.split("\r\n").filter_map(|line| line.split_once(':')) {
                for name in [
                    "Authorization",
                    "Www-Authenticate",
                    "Cookie",
                    "Cookie2",
                    "Range",
                    "Referer",
                    "User-Agent",
                ] {
                    if key.eq_ignore_ascii_case(name) {
                        seen.entry(name.to_owned())
                            .or_default()
                            .push(value.trim().to_owned());
                    }
                }
            }
            stream
                .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
                .unwrap();
            seen
        });
        let mut input = read_request(url, "GET".into());
        for (name, value) in [
            ("Authorization", "fixture-auth"),
            ("Www-Authenticate", "fixture-challenge"),
            ("Cookie", "fixture=cookie"),
            ("Cookie2", "fixture=cookie2"),
        ] {
            input.headers.insert(name.into(), vec![value.into()]);
        }
        let mut response = HttpTransport::new(None)
            .unwrap()
            .open_resource(input)
            .await
            .unwrap();
        assert_eq!(response.status_code, 204);
        let mut body = Vec::new();
        response.body.read_to_end(&mut body).await.unwrap();
        assert!(body.is_empty());
        assert_eq!(server.join().unwrap(), case.headers);
    }
}

#[tokio::test]
async fn initial_policy_errors_precede_request_validation_without_exposing_request_secrets() {
    let expected =
        Error::new(Reason::ResourceForbidden, "OpenResource").with_detail("fixture-policy-error");
    let rejected = expected.clone();
    let mut input = read_request("fixture-url-secret".into(), "POST".into());
    input
        .headers
        .insert("Authorization".into(), vec!["fixture-auth-secret".into()]);
    input.validate = Some(Arc::new(move |_| Err(rejected.clone())));
    let debug = format!("{input:?}");
    assert!(!debug.contains("fixture-url-secret"));
    assert!(!debug.contains("fixture-auth-secret"));
    let transport = HttpTransport::new(None).unwrap();
    assert_eq!(transport.open_resource(input).await.unwrap_err(), expected);

    let error = transport
        .open_resource(read_request("fixture-url-secret".into(), "POST".into()))
        .await
        .unwrap_err();
    assert_eq!(error.code, Reason::UpstreamUnavailable);
    assert!(!format!("{error:?}").contains("fixture-url-secret"));
}
