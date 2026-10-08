use pixiv_sdk::transport::{HttpTransport, Request, Transport};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

#[derive(Clone, Deserialize)]
struct Case {
    name: String,
    method: String,
    statuses: Vec<u16>,
    locations: Vec<String>,
    referer: String,
    interval_ms: u64,
    spaced: bool,
    calls: Value,
    status: u16,
    failed: bool,
}

const OBSERVED_HEADERS: &[&str] = &[
    "authorization",
    "www-authenticate",
    "cookie",
    "cookie2",
    "proxy-authorization",
    "proxy-authenticate",
    "content-type",
    "content-encoding",
    "content-language",
    "content-location",
    "referer",
    "x-fixture",
];

#[tokio::test(start_paused = true)]
async fn sdk_http_redirect_matches_go_methods_headers_body_and_limit() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/http-redirect.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 23);
    for (case, post_form) in cases
        .into_iter()
        .flat_map(|case| [(case.clone(), false), (case, true)])
    {
        let interval = Duration::from_millis(case.interval_ms);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let proxy = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let stopped = Arc::new(AtomicBool::new(false));
        let server_stopped = stopped.clone();
        let statuses = case.statuses;
        let locations = case.locations;
        let runtime = tokio::runtime::Handle::current();
        let server = thread::spawn(move || {
            let _runtime = runtime.enter();
            let mut calls = Vec::new();
            let mut last = None;
            let mut spaced = true;
            while !server_stopped.load(Ordering::SeqCst) {
                let (mut socket, _) = match listener.accept() {
                    Ok(socket) => socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    Err(error) => panic!("fixture proxy accept: {error}"),
                };
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut head = Vec::new();
                while !head.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    socket.read_exact(&mut byte).unwrap();
                    head.push(byte[0]);
                    assert!(head.len() < 65536);
                }
                let head = String::from_utf8(head).unwrap();
                let now = tokio::time::Instant::now();
                if let Some(previous) = last
                    && now.duration_since(previous) < interval
                {
                    spaced = false;
                }
                last = Some(now);
                let mut lines = head.split("\r\n");
                let mut request_line = lines.next().unwrap().split_whitespace();
                let method = request_line.next().unwrap();
                let url = request_line.next().unwrap();
                let headers: BTreeMap<_, _> = lines
                    .filter_map(|line| line.split_once(':'))
                    .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned()))
                    .collect();
                let size = headers
                    .get("content-length")
                    .map(|value| value.parse::<usize>().unwrap())
                    .unwrap_or_default();
                let mut body = vec![0; size];
                socket.read_exact(&mut body).unwrap();
                let observed: BTreeMap<_, _> = headers
                    .iter()
                    .filter(|(name, _)| OBSERVED_HEADERS.contains(&name.as_str()))
                    .collect();
                calls.push(json!({
                    "method": method,
                    "url": url,
                    "body": String::from_utf8(body).unwrap(),
                    "headers": observed,
                }));
                let index = calls.len() - 1;
                let status = statuses.get(index).copied().unwrap_or(200);
                let location = locations
                    .get(index)
                    .filter(|value| !value.is_empty())
                    .map(|value| format!("Location: {value}\r\n"))
                    .unwrap_or_default();
                write!(
                    socket,
                    "HTTP/1.1 {status} Fixture\r\n{location}Content-Length: 2\r\nConnection: close\r\n\r\n{{}}"
                )
                .unwrap();
            }
            (Value::Array(calls), spaced)
        });
        let mut headers: Vec<_> = OBSERVED_HEADERS
            .iter()
            .filter(|name| !matches!(**name, "content-type" | "referer"))
            .map(|name| ((*name).to_owned(), "fixture-value".to_owned()))
            .collect();
        headers.push((
            "content-type".into(),
            "application/x-www-form-urlencoded".into(),
        ));
        if !case.referer.is_empty() {
            headers.push(("referer".into(), case.referer));
        }
        let transport = HttpTransport::new(Some(&proxy))
            .unwrap()
            .with_pacing(interval);
        let result = async {
            let request = Request {
                method: case.method.parse().unwrap(),
                url: "http://source.invalid/start".into(),
                headers,
                parameters: vec![("value".into(), "fixture-value".into())],
                operation: "Artwork",
            };
            if post_form {
                transport.post_form(request).await
            } else {
                transport.send(request).await
            }
        }
        .await;
        stopped.store(true, Ordering::SeqCst);
        let (calls, spaced) = server.join().unwrap();
        assert_eq!(spaced, case.spaced, "{} pacing", case.name);
        assert_eq!(result.is_err(), case.failed, "{}", case.name);
        assert_eq!(
            result.as_ref().map(|response| response.status).unwrap_or(0),
            case.status,
            "{}",
            case.name
        );
        assert_eq!(calls, case.calls, "{}", case.name);
    }
}
