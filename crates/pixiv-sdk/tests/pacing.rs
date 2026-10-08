use pixiv_sdk::{
    resource::ResourceHeaders,
    transport::{HttpTransport, Request, ResourceReadRequest, ResourceTransport, Transport},
};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::{Duration, Instant},
};
use tokio::io::AsyncReadExt;

#[tokio::test(start_paused = true)]
async fn http_transport_matches_go_spacing_across_oauth_content_redirects_and_mutations() {
    let rows: Vec<Value> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/pacing.json"
    ))
    .unwrap();
    for expected in rows {
        let interval = Duration::from_millis(expected["interval_ms"].as_u64().unwrap());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let runtime = tokio::runtime::Handle::current();
        let server = thread::spawn(move || {
            let _runtime = runtime.enter();
            listener.set_nonblocking(true).unwrap();
            let mut calls = Vec::new();
            let mut last = None;
            let mut spaced = true;
            for _ in 0..5 {
                let deadline = Instant::now() + Duration::from_secs(5);
                let (mut socket, _) = loop {
                    match listener.accept() {
                        Ok(socket) => break socket,
                        Err(error)
                            if error.kind() == std::io::ErrorKind::WouldBlock
                                && Instant::now() < deadline =>
                        {
                            thread::sleep(Duration::from_millis(1))
                        }
                        Err(error) => panic!("missing expected paced request: {error}"),
                    }
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
                let now = tokio::time::Instant::now();
                if let Some(previous) = last
                    && now.duration_since(previous) + Duration::from_millis(5) < interval
                {
                    spaced = false;
                }
                last = Some(now);
                let head = String::from_utf8(head).unwrap();
                let mut request = head.lines().next().unwrap().split_whitespace();
                let method = request.next().unwrap();
                let path = request.next().unwrap();
                calls.push(format!("{method} {path}"));
                let response = if path == "/media-start" {
                    "HTTP/1.1 302 Found\r\nLocation: /media-final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                } else {
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}"
                };
                socket.write_all(response.as_bytes()).unwrap();
            }
            json!({"interval_ms": interval.as_millis() as u64, "calls": calls, "spaced": spaced})
        });
        let transport = HttpTransport::new(None).unwrap().with_pacing(interval);
        let clone = transport.clone();
        let request = |method, path: &str, operation| Request {
            method,
            url: format!("http://{address}{path}"),
            headers: Vec::new(),
            parameters: Vec::new(),
            operation,
        };
        transport
            .send(request("POST".parse().unwrap(), "/auth/token", "Open"))
            .await
            .unwrap();
        clone
            .send(request(
                "GET".parse().unwrap(),
                "/v1/illust/detail",
                "Artwork",
            ))
            .await
            .unwrap();
        let mut response = transport
            .open_resource(ResourceReadRequest {
                url: format!("http://{address}/media-start"),
                method: "GET".into(),
                headers: ResourceHeaders::new(),
                operation: "OpenResource",
                validate: None,
            })
            .await
            .unwrap();
        let mut body = Vec::new();
        response.body.read_to_end(&mut body).await.unwrap();
        assert_eq!(body, b"{}");
        clone
            .post_form(request(
                "POST".parse().unwrap(),
                "/v2/illust/bookmark/add",
                "BookmarkArtwork",
            ))
            .await
            .unwrap();
        assert_eq!(server.join().unwrap(), expected);
    }
}

#[tokio::test]
async fn default_http_transport_keeps_a_pending_response_after_the_prototype_sixty_second_limit() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            socket.read_exact(&mut byte).unwrap();
            head.push(byte[0]);
        }
        started_tx.send(()).unwrap();
        resume_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let _ = socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}");
    });
    let task = tokio::spawn(async move {
        HttpTransport::new(None)
            .unwrap()
            .send(Request {
                method: "GET".parse().unwrap(),
                url: format!("http://{address}/pending"),
                headers: Vec::new(),
                parameters: Vec::new(),
                operation: "Artwork",
            })
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), started_rx)
        .await
        .unwrap()
        .unwrap();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(61)).await;
    tokio::task::yield_now().await;
    let completed_early = task.is_finished();
    resume_tx.send(()).unwrap();
    tokio::time::resume();
    let result = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    server.join().unwrap();
    assert!(
        !completed_early,
        "prototype total timeout still terminates a pending response"
    );
    assert_eq!(result.unwrap().status, 200);
}

#[tokio::test]
async fn aborting_a_resource_pacing_wait_releases_the_queue_without_consuming_a_request_start() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let mut paths = Vec::new();
        listener.set_nonblocking(true).unwrap();
        for _ in 0..2 {
            let deadline = Instant::now() + Duration::from_secs(5);
            let (mut socket, _) = loop {
                match listener.accept() {
                    Ok(socket) => break socket,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        thread::sleep(Duration::from_millis(1))
                    }
                    Err(error) => panic!("pacing queue failed to resume: {error}"),
                }
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
            }
            let head = String::from_utf8(head).unwrap();
            paths.push(
                head.lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap()
                    .to_owned(),
            );
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
                .unwrap();
        }
        paths
    });
    let transport = HttpTransport::new(None)
        .unwrap()
        .with_pacing(Duration::from_secs(100));
    transport
        .send(Request {
            method: "GET".parse().unwrap(),
            url: format!("http://{address}/first"),
            headers: Vec::new(),
            parameters: Vec::new(),
            operation: "Artwork",
        })
        .await
        .unwrap();
    tokio::time::pause();
    let waiting = transport.clone();
    let task = tokio::spawn(async move {
        waiting
            .open_resource(ResourceReadRequest {
                url: format!("http://{address}/canceled"),
                method: "GET".into(),
                headers: ResourceHeaders::new(),
                operation: "OpenResource",
                validate: None,
            })
            .await
    });
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(50)).await;
    assert!(!task.is_finished());
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let next = transport.clone();
    let task = tokio::spawn(async move {
        next.post_form(Request {
            method: "POST".parse().unwrap(),
            url: format!("http://{address}/after-cancel"),
            headers: Vec::new(),
            parameters: Vec::new(),
            operation: "BookmarkArtwork",
        })
        .await
    });
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(49)).await;
    assert!(!task.is_finished());
    tokio::time::advance(Duration::from_secs(1)).await;
    tokio::time::resume();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(server.join().unwrap(), ["/first", "/after-cancel"]);
}
