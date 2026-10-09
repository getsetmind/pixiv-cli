use pixiv_sdk::transport::{HttpTransport, Request, Transport};
use std::{
    io::{Read, Write},
    net::TcpListener,
    time::Duration,
};

pub async fn capture_target(path: &str, parameters: Vec<(String, String)>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let thread = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut request = Vec::new();
        let mut byte = [0];
        while !request.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
            .unwrap();
        let request = String::from_utf8(request).unwrap();
        request
            .lines()
            .next()
            .unwrap()
            .split(' ')
            .nth(1)
            .unwrap()
            .to_owned()
    });
    let response = HttpTransport::new(None)
        .unwrap()
        .send(Request {
            method: reqwest::Method::GET,
            url: format!("http://{address}{path}"),
            headers: vec![],
            parameters,
            operation: "QueryEncoding",
        })
        .await
        .unwrap();
    assert_eq!(response.status, 200);
    thread.join().unwrap()
}
