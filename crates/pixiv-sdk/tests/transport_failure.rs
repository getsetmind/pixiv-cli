use pixiv_sdk::{
    Client, Result, oauth,
    pixiv::AddArtworkBookmarkRequest,
    transport::{HttpTransport, Request, Response, Transport},
};
use serde::Deserialize;
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener},
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::Duration,
};

#[derive(Deserialize)]
struct Case {
    name: String,
    operation: String,
    reason: String,
    transport: String,
    detail: String,
    message: String,
    status: u16,
    safe: bool,
    calls: usize,
}

struct RoutedTransport {
    http: HttpTransport,
    url: String,
    calls: AtomicUsize,
}

impl Transport for &RoutedTransport {
    async fn send(&self, mut request: Request) -> Result<Response> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        request.url.clone_from(&self.url);
        self.http.send(request).await
    }

    async fn post_form(&self, mut request: Request) -> Result<Response> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        request.url.clone_from(&self.url);
        self.http.post_form(request).await
    }
}

#[tokio::test]
async fn sdk_transport_failures_match_go_classification_and_consume_error_bodies() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/transport-failure.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 21);
    for case in cases {
        let reservation = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP)).unwrap();
        reservation
            .bind(&SocketAddr::from(([127, 0, 0, 1], 0)).into())
            .unwrap();
        let (url, server) = if case.name == "connection-refused" {
            let address = reservation.local_addr().unwrap().as_socket().unwrap();
            (format!("http://{address}/fixture"), None)
        } else {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let scheme = if case.name == "tls-record" {
                "https"
            } else {
                "http"
            };
            let url = format!("{scheme}://{}/fixture", listener.local_addr().unwrap());
            let name = case.name.clone();
            let server = thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                if name == "tls-record" {
                    let mut hello = [0; 4096];
                    assert!(socket.read(&mut hello).unwrap() > 0);
                    let _ = socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
                    return;
                }
                let mut head = Vec::new();
                while !head.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    socket.read_exact(&mut byte).unwrap();
                    head.push(byte[0]);
                    assert!(head.len() < 65536);
                }
                let head = String::from_utf8(head).unwrap();
                let size = head
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                    .unwrap_or_default();
                socket.read_exact(&mut vec![0; size]).unwrap();
                match name.as_str() {
                    "connection-reset" => {
                        let socket = Socket::from(socket);
                        socket.set_linger(Some(Duration::ZERO)).unwrap();
                    }
                    "unexpected-eof" => {}
                    "malformed-head" => {
                        socket.write_all(b"fixture-private-head\r\n\r\n").unwrap();
                    }
                    _ => {
                        let status = if name == "truncated-error" { 503 } else { 200 };
                        write!(socket, "HTTP/1.1 {status} Fixture\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{{}}")
                            .unwrap();
                    }
                }
            });
            (url, Some(server))
        };
        let transport = RoutedTransport {
            http: HttpTransport::new(None).unwrap(),
            url,
            calls: AtomicUsize::new(0),
        };
        let error = match case.operation.as_str() {
            "Artwork" => Client::with_transport("fixture-access", &transport)
                .artwork(42)
                .await
                .unwrap_err(),
            "Open" => oauth::refresh(&&transport, "fixture-refresh")
                .await
                .unwrap_err(),
            "AddArtworkBookmark" => Client::with_transport("fixture-access", &transport)
                .add_artwork_bookmark(AddArtworkBookmarkRequest {
                    artwork_id: 42,
                    ..Default::default()
                })
                .await
                .unwrap_err(),
            _ => unreachable!(),
        };
        if let Some(server) = server {
            server.join().unwrap();
        }
        let label = format!("{}/{}", case.operation, case.name);
        assert_eq!(error.code.as_str(), case.reason, "{label}");
        assert_eq!(
            serde_json::to_value(error.transport).unwrap(),
            serde_json::json!(case.transport),
            "{label}"
        );
        assert_eq!(
            error.detail.as_deref().unwrap_or_default(),
            case.detail,
            "{label}"
        );
        assert_eq!(error.to_string(), case.message, "{label}");
        assert_eq!(
            error.http_status.unwrap_or_default(),
            case.status,
            "{label}"
        );
        assert_eq!(error.retry.safe, case.safe, "{label}");
        assert_eq!(
            transport.calls.load(Ordering::SeqCst),
            case.calls,
            "{label}"
        );
        assert!(!format!("{error:?}").contains("fixture-private"));
    }
}
