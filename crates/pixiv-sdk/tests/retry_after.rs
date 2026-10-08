use chrono::{DateTime, TimeDelta, Utc};
use pixiv_sdk::{
    Client, Reason, Result,
    transport::{HttpTransport, Request, Response, Transport},
};
use reqwest::Method;
use serde::Deserialize;
use serde_json::json;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::Mutex,
    thread,
};

#[derive(Deserialize)]
struct Case {
    header: String,
    present: bool,
    nanos: i64,
}
#[derive(Deserialize)]
struct Contract {
    now: DateTime<Utc>,
    cases: Vec<Case>,
}

#[tokio::test]
async fn http_retry_headers_match_go_through_the_real_transport() {
    let contract: Contract = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/retry-after.json"
    ))
    .unwrap();
    for case in contract.cases {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/fixture", listener.local_addr().unwrap());
        let header = case.header.clone();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut received = Vec::new();
            let mut buffer = [0; 1024];
            while !received.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let count = stream.read(&mut buffer).unwrap();
                assert!(count > 0);
                received.extend_from_slice(&buffer[..count]);
            }
            assert!(received.starts_with(b"GET /fixture HTTP/1.1\r\n"));
            write!(stream,"HTTP/1.1 429 Too Many Requests\r\nRetry-After: {header}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        });
        let before = Utc::now();
        let response = HttpTransport::new(None)
            .unwrap()
            .send(Request {
                method: Method::GET,
                url,
                headers: vec![],
                parameters: vec![],
                operation: "Fixture",
            })
            .await
            .unwrap();
        let after = Utc::now();
        server.join().unwrap();
        assert_eq!(
            response.retry_after.is_some(),
            case.present,
            "{}",
            case.header
        );
        if let Some(delay) = response.retry_after {
            let expected = if case.header.contains(':') && case.nanos != i64::MAX {
                let target = contract.now + TimeDelta::nanoseconds(case.nanos);
                let earliest = (target - after).max(TimeDelta::zero());
                let latest = (target - before).max(TimeDelta::zero());
                assert!(
                    delay >= earliest && delay <= latest,
                    "{}: {delay}",
                    case.header
                );
                continue;
            } else {
                TimeDelta::nanoseconds(case.nanos)
            };
            assert_eq!(delay, expected, "{}", case.header);
        }
    }
}

struct Sequence {
    replies: Mutex<Vec<Response>>,
    requests: Mutex<Vec<Request>>,
}
impl Transport for Sequence {
    async fn send(&self, request: Request) -> Result<Response> {
        self.requests.lock().unwrap().push(request);
        Ok(self.replies.lock().unwrap().remove(0))
    }
}

#[tokio::test(start_paused = true)]
async fn content_reads_replay_once_only_for_a_valid_rate_limit_delay() {
    for (status, delay, expected_calls) in [
        (429, Some(TimeDelta::seconds(3)), 2),
        (429, None, 1),
        (503, Some(TimeDelta::seconds(3)), 1),
        (401, Some(TimeDelta::seconds(3)), 1),
    ] {
        let transport = Sequence {
            replies: Mutex::new(
                (0..2)
                    .map(|_| Response {
                        status,
                        retry_after: delay,
                        body: json!(null),
                    })
                    .collect(),
            ),
            requests: Mutex::new(vec![]),
        };
        let client = Client::with_transport("fixture-access", &transport);
        let started = tokio::time::Instant::now();
        let error = client.artwork(42).await.unwrap_err();
        assert_eq!(
            error.code,
            match status {
                429 => Reason::RateLimited,
                401 => Reason::CredentialsExpired,
                _ => Reason::UpstreamError,
            }
        );
        let requests = transport.requests.lock().unwrap();
        assert_eq!(requests.len(), expected_calls);
        if expected_calls == 2 {
            assert!(tokio::time::Instant::now() - started >= std::time::Duration::from_secs(3));
            assert_eq!(requests[0].url, requests[1].url);
            assert_eq!(requests[0].method, requests[1].method);
            assert_eq!(requests[0].headers, requests[1].headers);
            assert_eq!(requests[0].parameters, requests[1].parameters);
        }
    }
}

impl Transport for &Sequence {
    async fn send(&self, request: Request) -> Result<Response> {
        (*self).send(request).await
    }
}

#[tokio::test(start_paused = true)]
async fn immediate_rate_limit_retries_still_honor_the_client_pacing_interval() {
    let transport = Sequence {
        replies: Mutex::new(
            (0..2)
                .map(|_| Response {
                    status: 429,
                    retry_after: Some(TimeDelta::zero()),
                    body: json!(null),
                })
                .collect(),
        ),
        requests: Mutex::new(vec![]),
    };
    let client = Client::with_transport("fixture-access", &transport)
        .with_pacing(std::time::Duration::from_secs(3));
    let started = tokio::time::Instant::now();
    assert_eq!(
        client.artwork(42).await.unwrap_err().code,
        Reason::RateLimited
    );
    assert_eq!(transport.requests.lock().unwrap().len(), 2);
    assert!(tokio::time::Instant::now() - started >= std::time::Duration::from_secs(3));
}

#[tokio::test(start_paused = true)]
async fn retry_success_returns_the_second_response_and_dropped_waits_never_replay() {
    let transport = Sequence {
        replies: Mutex::new(vec![
            Response {
                status: 429,
                retry_after: Some(TimeDelta::seconds(2)),
                body: json!(null),
            },
            Response {
                status: 200,
                retry_after: None,
                body: json!({"illust":{
                    "id":42,"title":"fixture","caption":"","type":"illust","tags":[],
                    "user":{"id":7,"name":"fixture","account":"fixture"},
                    "create_date":"2026-01-01T00:00:00Z","total_bookmarks":0,"total_view":0,
                    "width":10,"height":10,"page_count":1,"x_restrict":0
                }}),
            },
        ]),
        requests: Mutex::new(vec![]),
    };
    assert_eq!(
        Client::with_transport("fixture-access", &transport)
            .artwork(42)
            .await
            .unwrap()
            .id,
        42
    );
    assert_eq!(transport.requests.lock().unwrap().len(), 2);

    let transport = Sequence {
        replies: Mutex::new(vec![Response {
            status: 429,
            retry_after: Some(TimeDelta::seconds(60)),
            body: json!(null),
        }]),
        requests: Mutex::new(vec![]),
    };
    let client = Client::with_transport("fixture-access", &transport);
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(1), client.artwork(42))
            .await
            .is_err()
    );
    tokio::time::advance(std::time::Duration::from_secs(120)).await;
    assert_eq!(transport.requests.lock().unwrap().len(), 1);
}
