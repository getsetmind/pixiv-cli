use super::{FIXTURE, context};
use pixiv_app::lifecycle::{Context, ContextError};
use pixiv_cli_rs::dictionary::service::{
    self, Client, ErrorCode, HttpError, HttpTransport, SearchRequest, Transport,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    error::Error as StdError,
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

struct Server {
    origin: String,
    requests: Arc<Mutex<Vec<Value>>>,
    stopped: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn start(row: Value, other_origin: Option<String>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&requests);
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&stopped);
        let worker = thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut socket, _)) => {
                        socket
                            .set_read_timeout(Some(Duration::from_secs(3)))
                            .unwrap();
                        let mut raw = Vec::new();
                        let mut bytes = [0; 1024];
                        while !raw.windows(4).any(|value| value == b"\r\n\r\n") {
                            match socket.read(&mut bytes) {
                                Ok(0) => break,
                                Ok(count) => raw.extend_from_slice(&bytes[..count]),
                                Err(_) => break,
                            }
                        }
                        if raw.is_empty() {
                            continue;
                        }
                        let text = String::from_utf8(raw).unwrap();
                        let mut lines = text.split("\r\n");
                        let mut first = lines.next().unwrap().split(' ');
                        let method = first.next().unwrap();
                        let target = first.next().unwrap();
                        let mut headers: BTreeMap<String, Vec<String>> = BTreeMap::new();
                        let mut host = String::new();
                        for line in lines.take_while(|line| !line.is_empty()) {
                            let (name, value) = line.split_once(':').unwrap();
                            let value = value.trim().to_owned();
                            if name.eq_ignore_ascii_case("host") {
                                host = value;
                                continue;
                            }
                            let name = name
                                .split('-')
                                .map(|part| {
                                    let mut chars = part.chars();
                                    format!(
                                        "{}{}",
                                        chars.next().unwrap().to_ascii_uppercase(),
                                        chars.as_str().to_ascii_lowercase()
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join("-");
                            headers.entry(name).or_default().push(value);
                        }
                        observed.lock().unwrap().push(json!({"method":method,"url":target,"request_uri":target,"host":host,"headers":headers}));
                        let status = row["response"]["status"].as_u64().unwrap();
                        let body = row["response"]["body"].as_str().unwrap().as_bytes();
                        let (status, extra, sent_body, length) = if target.starts_with("/redirect")
                        {
                            let location = other_origin
                                .as_ref()
                                .map(|origin| format!("{origin}/final?step=2"))
                                .unwrap_or_else(|| "/final?step=2".to_owned());
                            (
                                row["redirect_status"].as_u64().unwrap(),
                                format!("Location: {location}\r\n"),
                                Vec::new(),
                                0,
                            )
                        } else if target.starts_with("/loop") {
                            let hop = target.split("hop=").nth(1).unwrap().parse::<u64>().unwrap();
                            (
                                302,
                                format!("Location: /loop?hop={}\r\n", hop + 1),
                                Vec::new(),
                                0,
                            )
                        } else if target.starts_with("/truncated") {
                            (status, String::new(), body.to_vec(), body.len() + 20)
                        } else if target.starts_with("/gzip") {
                            let gzip = vec![
                                31, 139, 8, 0, 0, 0, 0, 0, 2, 3, 75, 173, 40, 72, 204, 75, 73, 77,
                                81, 72, 201, 76, 46, 201, 204, 207, 75, 44, 170, 84, 120, 52, 103,
                                50, 0, 230, 152, 33, 104, 23, 0, 0, 0,
                            ];
                            let length = gzip.len();
                            (
                                status,
                                "Content-Encoding: gzip\r\n".to_owned(),
                                gzip,
                                length,
                            )
                        } else {
                            (status, String::new(), body.to_vec(), body.len())
                        };
                        let header = format!(
                            "HTTP/1.1 {status} Fixture\r\n{extra}Content-Length: {length}\r\nConnection: close\r\n\r\n"
                        );
                        let _ = socket.write_all(header.as_bytes());
                        let _ = socket.write_all(&sent_body);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2))
                    }
                    Err(error) => panic!("owned dictionary listener failed: {error}"),
                }
            }
        });
        Self {
            origin,
            requests,
            stopped,
            worker: Some(worker),
        }
    }
    fn requests(&self, origin: &str, other_origin: Option<&str>) -> Vec<Value> {
        let mut requests = self.requests.lock().unwrap().clone();
        for request in &mut requests {
            let host = request["host"].as_str().unwrap();
            request["host"] = json!(if host == origin.strip_prefix("http://").unwrap() {
                "<HOST>"
            } else if other_origin.is_some_and(|other| Some(host) == other.strip_prefix("http://"))
            {
                "<OTHER_HOST>"
            } else {
                panic!("unowned host {host}")
            });
            if let Some(values) = request["headers"]
                .get_mut("Referer")
                .and_then(Value::as_array_mut)
            {
                for value in values {
                    *value = json!(value.as_str().unwrap().replace(origin, "<ORIGIN>"));
                }
            }
        }
        requests
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.worker.take().unwrap().join().unwrap();
    }
}
fn wire_request(value: &Value) -> Value {
    json!({"method":value["method"],"url":value["url"],"request_uri":value["request_uri"],"host":value["host"],"headers":value["headers"]})
}
fn source_contains(error: &(dyn StdError + 'static), expected: ContextError) -> bool {
    let mut current = Some(error);
    while let Some(error) = current {
        if error.downcast_ref::<ContextError>() == Some(&expected) {
            return true;
        }
        current = error.source();
    }
    false
}

#[tokio::test]
async fn native_anonymous_get_headers_gzip_status_redirects_and_context_match_owned_go_http() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let rows = fixture["transports"].as_array().unwrap();
    assert_eq!(rows.len(), 42);
    let mut compared = 0;
    for row in rows
        .iter()
        .filter(|row| row["mode"] == "loopback" && row["accept"] != "")
    {
        let name = row["name"].as_str().unwrap();
        let other = row["cross_host"].as_bool().unwrap().then(|| {
            let mut server = Server::start(row.clone(), None);
            server.origin = server.origin.replace("127.0.0.1", "localhost");
            server
        });
        let server = Server::start(
            row.clone(),
            other.as_ref().map(|server| server.origin.clone()),
        );
        let transport = HttpTransport::new()
            .unwrap()
            .with_user_agent(row["user_agent"].as_str().unwrap());
        let context = context(row["context"].as_str().unwrap());
        let url = format!("{}{}", server.origin, row["raw_url"].as_str().unwrap());
        let result = transport
            .get(context.as_ref(), &url, row["accept"].as_str().unwrap())
            .await;
        let expected = &row["results"][0];
        match result {
            Ok(response) => {
                assert_eq!(expected["error"], Value::Null, "{name}");
                assert_eq!(
                    response.status,
                    expected["status"].as_u64().unwrap() as u16,
                    "{name}"
                );
                assert_eq!(
                    String::from_utf8(response.body).unwrap(),
                    expected["body"].as_str().unwrap(),
                    "{name}"
                );
            }
            Err(error) => {
                assert!(!expected["error"].is_null(), "{name}: {error}");
                assert_eq!(
                    service::code_of(Some(error.as_ref())),
                    ErrorCode::Unknown,
                    "{name}"
                );
                assert_eq!(
                    error.to_string().replace(&server.origin, "<ORIGIN>"),
                    expected["error"]["message"].as_str().unwrap(),
                    "{name}"
                );
                assert_eq!(
                    error
                        .downcast_ref::<HttpError>()
                        .map(HttpError::status_code)
                        .unwrap_or(0),
                    expected["status"].as_u64().unwrap() as u16,
                    "{name}"
                );
                assert_eq!(
                    source_contains(error.as_ref(), ContextError::Canceled),
                    expected["error"]["is_canceled"].as_bool().unwrap(),
                    "{name}"
                );
                assert_eq!(
                    source_contains(error.as_ref(), ContextError::DeadlineExceeded),
                    expected["error"]["is_deadline"].as_bool().unwrap(),
                    "{name}"
                );
                if name.starts_with("loopback-truncated") {
                    assert_eq!(expected["error"]["cause"], "");
                    assert!(
                        error.source().is_some(),
                        "reqwest retains its native body-error cause; Go immediate-cause parity is open"
                    );
                } else {
                    assert_eq!(
                        error.source().map(ToString::to_string).unwrap_or_default(),
                        expected["error"]["cause"].as_str().unwrap(),
                        "{name}"
                    );
                }
            }
        }
        let mut actual = server.requests(
            &server.origin,
            other.as_ref().map(|server| server.origin.as_str()),
        );
        if let Some(other) = &other {
            actual.extend(other.requests(&server.origin, Some(&other.origin)));
        }
        let expected: Vec<_> = row["requests"]
            .as_array()
            .unwrap()
            .iter()
            .map(wire_request)
            .collect();
        assert_eq!(actual, expected, "{name} full native wire");
        compared += 1;
    }
    assert_eq!(compared, 21);
}

#[tokio::test]
async fn direct_empty_accept_is_an_explicit_unresolved_native_wire_difference() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let row = fixture["transports"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "loopback-default-headers-")
        .unwrap();
    let server = Server::start(row.clone(), None);
    let transport = HttpTransport::new().unwrap();
    let response = transport
        .get(
            Some(&Context::new()),
            &format!("{}{}", server.origin, row["raw_url"].as_str().unwrap()),
            "",
        )
        .await
        .unwrap();
    assert_eq!(
        response.body,
        row["response"]["body"].as_str().unwrap().as_bytes()
    );
    let requests = server.requests(&server.origin, None);
    assert!(row["requests"][0]["headers"].get("Accept").is_none());
    assert_eq!(requests[0]["headers"]["Accept"], json!([""]));
    assert_ne!(requests[0]["headers"], row["requests"][0]["headers"]);
}

#[tokio::test]
async fn direct_nil_context_has_the_frozen_request_creation_error_and_cause() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let row = fixture["transports"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "synthetic-nil-context-before-roundtrip")
        .unwrap();
    let error = HttpTransport::new()
        .unwrap()
        .get(
            None,
            row["raw_url"].as_str().unwrap(),
            row["accept"].as_str().unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        row["results"][0]["error"]["message"].as_str().unwrap()
    );
    assert_eq!(
        error.source().unwrap().to_string(),
        row["results"][0]["error"]["cause"].as_str().unwrap()
    );
    assert_eq!(service::code_of(Some(error.as_ref())), ErrorCode::Unknown);
}

struct OwnedOriginTransport {
    native: HttpTransport,
    origin: String,
}
impl Transport for OwnedOriginTransport {
    async fn get(
        &self,
        context: Option<&Context>,
        raw_url: &str,
        accept: &str,
    ) -> Result<service::Response, service::TransportError> {
        assert!(raw_url.starts_with("https://dic.pixiv.net/"));
        self.native
            .get(
                context,
                &raw_url.replacen("https://dic.pixiv.net", &self.origin, 1),
                accept,
            )
            .await
    }
}

#[tokio::test]
async fn failed_body_read_precedes_search_404_empty_result_interpretation() {
    let row = json!({"response":{"status":404,"body":"prefix"},"redirect_status":0});
    let server = Server::start(row, None);
    let transport = OwnedOriginTransport {
        native: HttpTransport::new().unwrap(),
        origin: format!("{}/truncated", server.origin),
    };
    let result = Client::new(Some(transport))
        .search(
            Some(&Context::new()),
            SearchRequest {
                query: "private query".to_owned(),
                page: 1,
            },
        )
        .await;
    let error = result.unwrap_err();
    assert_eq!(error.code(), ErrorCode::Transport);
    assert_eq!(error.to_string(), "dic search request failed");
    let cause = error.source().unwrap();
    assert_eq!(cause.to_string(), "unexpected EOF");
    assert_eq!(
        cause.downcast_ref::<HttpError>().unwrap().status_code(),
        404
    );
    assert_eq!(server.requests.lock().unwrap().len(), 1);
}
