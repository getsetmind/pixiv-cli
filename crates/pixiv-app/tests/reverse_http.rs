use pixiv_app::reverse_search::{
    CallerContext,
    http::{HttpRequest, HttpTransport, NativeHttpTransport, ReqwestTransport},
};
use pixiv_sdk::{
    context::{Context, ContextError},
    fanbox::transport::{
        ExternalError, Headers, RawRequest, RawResponse, RawTransport, TransportFuture,
    },
};
use std::{
    io::{self, Cursor, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

fn request(url: String, context: CallerContext) -> HttpRequest {
    HttpRequest {
        method: "GET".into(),
        url,
        logical_host: None,
        headers: Headers::new(),
        body: None,
        content_length: 0,
        context,
    }
}
fn listener() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    (listener, url)
}
fn timed(stream: &TcpStream) {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
}
fn line(stream: &mut TcpStream) -> String {
    let mut result = Vec::new();
    loop {
        let mut byte = [0];
        stream.read_exact(&mut byte).unwrap();
        result.push(byte[0]);
        if result.ends_with(b"\r\n") {
            return String::from_utf8(result).unwrap();
        }
    }
}
fn read_headers(stream: &mut TcpStream) -> String {
    let mut result = line(stream);
    loop {
        let next = line(stream);
        if next == "\r\n" {
            return result;
        }
        result.push_str(&next);
    }
}
fn body(stream: &mut TcpStream, headers: &str, first: Option<mpsc::Sender<()>>) -> Vec<u8> {
    if headers
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        let mut output = Vec::new();
        let mut first = first;
        loop {
            let size_line = line(stream);
            let length =
                usize::from_str_radix(size_line.trim().split(';').next().unwrap(), 16).unwrap();
            if length == 0 {
                assert_eq!(line(stream), "\r\n");
                return output;
            }
            let start = output.len();
            output.resize(start + length, 0);
            stream.read_exact(&mut output[start..]).unwrap();
            assert_eq!(line(stream), "\r\n");
            if let Some(sender) = first.take() {
                sender.send(()).unwrap();
            }
        }
    }
    let length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    let mut output = vec![0; length];
    stream.read_exact(&mut output).unwrap();
    output
}
async fn join_server<T: Send + 'static>(server: thread::JoinHandle<T>) -> T {
    tokio::task::spawn_blocking(move || server.join().unwrap())
        .await
        .unwrap()
}
async fn read_body(mut response: RawResponse) -> Result<Vec<u8>, ExternalError> {
    let mut output = Vec::new();
    if let Some(body) = response.body.as_mut() {
        let mut buffer = [0; 8192];
        loop {
            let read = body.read(&mut buffer).await;
            assert!(read.count <= buffer.len());
            output.extend_from_slice(&buffer[..read.count]);
            if let Some(error) = read.error {
                let _ = body.close().await;
                return Err(error);
            }
            if read.eof {
                break;
            }
        }
        body.close().await?;
    }
    Ok(output)
}

#[tokio::test]
async fn ordinary_source_uses_raw_response_without_provider_headers_or_automatic_redirects() {
    let (listener, url) = listener();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        timed(&stream);
        let headers = read_headers(&mut stream);
        stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: /next\r\nX-Repeat: one\r\nX-Repeat: two\r\nContent-Length: 5\r\nConnection: close\r\n\r\nowned").unwrap();
        headers
    });
    let transport = ReqwestTransport::new("").unwrap();
    let response = RawTransport::send(
        &transport,
        RawRequest {
            method: "GET".into(),
            url: format!("{url}/source"),
            logical_host: None,
            headers: Headers::new(),
            body: None,
            content_length: 0,
            context: Arc::new(Context::background()),
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response.status, 302);
    assert_eq!(response.content_length, 5);
    assert_eq!(response.headers.get("Location").unwrap(), &["/next"]);
    assert_eq!(response.headers.get("X-Repeat").unwrap(), &["one", "two"]);
    assert_eq!(read_body(response).await.unwrap(), b"owned");
    let headers = join_server(server).await.to_ascii_lowercase();
    assert!(headers.starts_with("get /source http/1.1\r\n"));
    assert!(headers.contains("user-agent: go-http-client/1.1\r\n"));
    assert!(headers.contains("accept-encoding: gzip\r\n"));
    assert!(!headers.contains("referer:"));
    assert!(!headers.contains("pixiv"));
    assert!(!headers.contains("authorization:"));
}

struct StreamingInput {
    left: usize,
    first: bool,
    permit: mpsc::Receiver<()>,
    largest_read: Arc<AtomicUsize>,
}
impl Read for StreamingInput {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.largest_read.fetch_max(output.len(), Ordering::SeqCst);
        if !self.first {
            self.permit
                .recv_timeout(Duration::from_secs(5))
                .map_err(io::Error::other)?;
            self.first = true;
        }
        let length = self.left.min(output.len()).min(8192);
        output[..length].fill(b'x');
        self.left -= length;
        if self.left > 0 {
            self.first = true;
        }
        Ok(length)
    }
}
struct FirstChunkThenGate {
    input: StreamingInput,
    count: usize,
}
impl Read for FirstChunkThenGate {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if self.count == 1 {
            self.input.first = false;
        }
        self.count += 1;
        self.input.read(output)
    }
}

#[tokio::test]
async fn ordinary_upload_streams_bounded_chunks_and_has_no_ascii_image_size_limit() {
    let (listener, url) = listener();
    let (permit, receiver) = mpsc::channel();
    let size = 11 * 1024 * 1024 + 13;
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        timed(&stream);
        let headers = read_headers(&mut stream);
        let output = body(&mut stream, &headers, Some(permit));
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .unwrap();
        (headers, output)
    });
    let largest_read = Arc::new(AtomicUsize::new(0));
    let reader = FirstChunkThenGate {
        input: StreamingInput {
            left: size,
            first: true,
            permit: receiver,
            largest_read: largest_read.clone(),
        },
        count: 0,
    };
    let transport = ReqwestTransport::new("").unwrap();
    let mut upload = request(format!("{url}/upload"), Arc::new(Context::background()));
    upload.method = "POST".into();
    upload.headers.insert(
        "Content-Type".into(),
        vec!["application/octet-stream".into()],
    );
    upload.body = Some(Box::new(reader));
    upload.content_length = -1;
    let response = tokio::time::timeout(
        Duration::from_secs(10),
        HttpTransport::send(&transport, upload),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();
    assert_eq!(read_body(response).await.unwrap(), b"ok");
    let (headers, output) = join_server(server).await;
    assert!(
        headers
            .to_ascii_lowercase()
            .contains("transfer-encoding: chunked")
    );
    assert!(!headers.to_ascii_lowercase().contains("content-length:"));
    assert_eq!(output.len(), size);
    assert!(output.iter().all(|byte| *byte == b'x'));
    assert!(largest_read.load(Ordering::SeqCst) <= 64 * 1024);
}

#[tokio::test]
async fn ordinary_explicit_proxy_and_logical_host_are_preserved() {
    let (listener, proxy) = listener();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        timed(&stream);
        let headers = read_headers(&mut stream);
        let output = body(&mut stream, &headers, None);
        stream
            .write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .unwrap();
        (headers, output)
    });
    let transport = ReqwestTransport::new(&proxy).unwrap();
    let mut upload = request(
        "http://owned.invalid/upload?q=1".into(),
        Arc::new(Context::background()),
    );
    upload.method = "POST".into();
    upload.logical_host = Some("logical.invalid".into());
    upload
        .headers
        .insert("X-Repeat".into(), vec!["one".into(), "two".into()]);
    upload.body = Some(Box::new(Cursor::new(b"synthetic".to_vec())));
    upload.content_length = 9;
    let response = HttpTransport::send(&transport, upload)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status, 201);
    assert!(read_body(response).await.unwrap().is_empty());
    let (headers, output) = join_server(server).await;
    let headers = headers.to_ascii_lowercase();
    assert!(headers.starts_with("post http://owned.invalid/upload?q=1 http/1.1"));
    assert!(headers.contains("host: logical.invalid\r\n"));
    assert!(headers.contains("content-length: 9\r\n"));
    assert!(headers.contains("x-repeat: one\r\n"));
    assert!(headers.contains("x-repeat: two\r\n"));
    assert_eq!(output, b"synthetic");
}

#[tokio::test]
async fn ordinary_request_cancellation_drops_the_physical_request() {
    let (listener, url) = listener();
    let (accepted, receiver) = tokio::sync::oneshot::channel();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        timed(&stream);
        read_headers(&mut stream);
        accepted.send(()).unwrap();
        let mut byte = [0];
        stream.read(&mut byte).unwrap()
    });
    let transport = Arc::new(ReqwestTransport::new("").unwrap());
    let context = Arc::new(Context::new());
    let pending = tokio::spawn({
        let transport = transport.clone();
        let context = context.clone();
        async move { HttpTransport::send(transport.as_ref(), request(url, context)).await }
    });
    receiver.await.unwrap();
    context.cancel();
    let error = tokio::time::timeout(Duration::from_secs(2), pending)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(
        error.downcast_ref::<ContextError>(),
        Some(&ContextError::Canceled)
    );
    assert_eq!(join_server(server).await, 0);
}

#[tokio::test]
async fn ordinary_response_reads_observe_cancellation_and_close_is_idempotent() {
    let (listener, url) = listener();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        timed(&stream);
        read_headers(&mut stream);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\n\r\n")
            .unwrap();
        let mut byte = [0];
        stream.read(&mut byte).unwrap()
    });
    let transport = ReqwestTransport::new("").unwrap();
    let context = Arc::new(Context::new());
    let mut response = HttpTransport::send(&transport, request(url, context.clone()))
        .await
        .unwrap()
        .unwrap();
    context.cancel();
    let body = response.body.as_mut().unwrap();
    let mut buffer = [0; 4];
    let read = body.read(&mut buffer).await;
    assert_eq!(read.count, 0);
    assert!(!read.eof);
    assert_eq!(
        read.error.unwrap().downcast_ref::<ContextError>(),
        Some(&ContextError::Canceled)
    );
    body.close().await.unwrap();
    body.close().await.unwrap();
    assert!(body.read(&mut buffer).await.eof);
    assert_eq!(join_server(server).await, 0);
}

#[tokio::test]
async fn ordinary_truncated_response_reports_read_error_after_received_bytes() {
    let (listener, url) = listener();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        timed(&stream);
        read_headers(&mut stream);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\npart")
            .unwrap();
    });
    let transport = ReqwestTransport::new("").unwrap();
    let mut response =
        HttpTransport::send(&transport, request(url, Arc::new(Context::background())))
            .await
            .unwrap()
            .unwrap();
    let body = response.body.as_mut().unwrap();
    let mut output = Vec::new();
    let mut buffer = [0; 2];
    loop {
        let read = body.read(&mut buffer).await;
        output.extend_from_slice(&buffer[..read.count]);
        if read.error.is_some() {
            assert!(!read.eof);
            break;
        }
        assert!(!read.eof, "truncation must not become clean EOF");
    }
    assert_eq!(output, b"part");
    body.close().await.unwrap();
    join_server(server).await;
}

#[tokio::test]
async fn ordinary_idle_close_retires_the_owned_pool_and_later_requests_reopen_it() {
    let (listener, url) = listener();
    let server = thread::spawn(move || {
        let (mut first, _) = listener.accept().unwrap();
        timed(&first);
        for _ in 0..2 {
            read_headers(&mut first);
            first
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\nx")
                .unwrap();
        }
        let mut byte = [0];
        assert_eq!(first.read(&mut byte).unwrap(), 0);
        let (mut second, _) = listener.accept().unwrap();
        timed(&second);
        read_headers(&mut second);
        second
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\nConnection: close\r\n\r\ny")
            .unwrap();
    });
    let transport = ReqwestTransport::new("").unwrap();
    for _ in 0..2 {
        let response = HttpTransport::send(
            &transport,
            request(url.clone(), Arc::new(Context::background())),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(read_body(response).await.unwrap(), b"x");
    }
    HttpTransport::close_idle_connections(&transport);
    HttpTransport::close_idle_connections(&transport);
    let response = HttpTransport::send(&transport, request(url, Arc::new(Context::background())))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read_body(response).await.unwrap(), b"y");
    join_server(server).await;
}

#[derive(Default)]
struct NativePort {
    requests: Mutex<Vec<RawRequest>>,
    closes: AtomicUsize,
}
impl RawTransport for NativePort {
    fn send(
        &self,
        request: RawRequest,
    ) -> TransportFuture<'_, Result<Option<RawResponse>, ExternalError>> {
        self.requests.lock().unwrap().push(request);
        Box::pin(async {
            Ok(Some(RawResponse {
                status: 418,
                headers: Headers::new(),
                content_length: 0,
                body: None,
            }))
        })
    }
    fn close_idle_connections(&self) {
        self.closes.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn native_bridge_preserves_unknown_length_context_host_headers_and_response() {
    let port = Arc::new(NativePort::default());
    let transport = NativeHttpTransport::new(port.clone());
    let context: CallerContext = Arc::new(Context::background());
    let mut upload = request(
        "https://ascii2d.invalid/search/file".into(),
        context.clone(),
    );
    upload.method = "POST".into();
    upload.logical_host = Some("ascii2d.logical.invalid".into());
    upload.headers.insert(
        "Cookie".into(),
        vec!["owned=one".into(), "owned=two".into()],
    );
    upload.body = Some(Box::new(Cursor::new(
        b"complete-synthetic-multipart".to_vec(),
    )));
    upload.content_length = -1;
    let response = HttpTransport::send(&transport, upload)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status, 418);
    assert!(response.body.is_none());
    let requests = port.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.method, "POST");
    assert_eq!(request.content_length, -1);
    assert_eq!(
        request.logical_host.as_deref(),
        Some("ascii2d.logical.invalid")
    );
    assert_eq!(
        request.headers.get("Cookie").unwrap(),
        &["owned=one", "owned=two"]
    );
    assert_eq!(
        request.body.as_deref(),
        Some(b"complete-synthetic-multipart".as_slice())
    );
    assert!(Arc::ptr_eq(&request.context, &context));
    drop(requests);
    HttpTransport::close_idle_connections(&transport);
    assert_eq!(port.closes.load(Ordering::SeqCst), 1);
}

struct BrokenInput;
impl Read for BrokenInput {
    fn read(&mut self, _output: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "owned upload read failure",
        ))
    }
}
#[tokio::test]
async fn native_bridge_retains_reader_failure_and_does_not_manufacture_a_response() {
    let port = Arc::new(NativePort::default());
    let transport = NativeHttpTransport::new(port.clone());
    let mut upload = request(
        "https://ascii2d.invalid/search/file".into(),
        Arc::new(Context::background()),
    );
    upload.method = "POST".into();
    upload.body = Some(Box::new(BrokenInput));
    upload.content_length = -1;
    let error = HttpTransport::send(&transport, upload).await.unwrap_err();
    assert_eq!(
        error.downcast_ref::<io::Error>().unwrap().kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(error.to_string(), "owned upload read failure");
    assert!(port.requests.lock().unwrap().is_empty());
}

struct CancelingInput {
    context: Arc<Context>,
    reads: Arc<AtomicUsize>,
    largest_read: Arc<AtomicUsize>,
}
impl Read for CancelingInput {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.largest_read.fetch_max(output.len(), Ordering::SeqCst);
        let count = output.len().min(8192);
        output[..count].fill(b'x');
        if self.reads.fetch_add(1, Ordering::SeqCst) == 3 {
            self.context.cancel();
        }
        Ok(count)
    }
}

#[tokio::test]
async fn native_bridge_caller_cancellation_stops_unknown_length_buffering_before_native_send() {
    let port = Arc::new(NativePort::default());
    let transport = NativeHttpTransport::new(port.clone());
    let context = Arc::new(Context::new());
    let reads = Arc::new(AtomicUsize::new(0));
    let largest_read = Arc::new(AtomicUsize::new(0));
    let reader = CancelingInput {
        context: context.clone(),
        reads: reads.clone(),
        largest_read: largest_read.clone(),
    };
    let mut upload = request(
        "https://ascii2d.invalid/search/file".into(),
        context.clone(),
    );
    upload.method = "POST".into();
    upload.body = Some(Box::new(reader));
    upload.content_length = -1;
    let error = tokio::time::timeout(
        Duration::from_secs(2),
        HttpTransport::send(&transport, upload),
    )
    .await
    .unwrap()
    .unwrap_err();
    let cause = error.downcast_ref::<io::Error>().unwrap();
    assert_eq!(
        cause.get_ref().unwrap().downcast_ref::<ContextError>(),
        Some(&ContextError::Canceled)
    );
    assert_eq!(reads.load(Ordering::SeqCst), 4);
    assert!(port.requests.lock().unwrap().is_empty());
    assert!(largest_read.load(Ordering::SeqCst) <= 32 * 1024);
}

#[tokio::test]
async fn ordinary_source_streams_over_ascii_limit_into_the_owned_snapshot() {
    use pixiv_app::reverse_search::{Loader, SourceKind, SourceLoader, SourceLoaderOptions};
    let (listener, url) = listener();
    let size = 11 * 1024 * 1024 + 31;
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        timed(&stream);
        read_headers(&mut stream);
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {size}\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let chunk = [b's'; 8192];
        let mut left = size;
        while left > 0 {
            let count = left.min(chunk.len());
            stream.write_all(&chunk[..count]).unwrap();
            left -= count;
        }
    });
    let directory = tempfile::tempdir().unwrap();
    let loader = Loader::new(SourceLoaderOptions {
        temp_dir: directory.path().into(),
        http_transport: Some(Arc::new(ReqwestTransport::new("").unwrap())),
        ..SourceLoaderOptions::default()
    });
    let snapshot = loader
        .load(Arc::new(Context::background()), &url)
        .await
        .unwrap();
    assert_eq!(snapshot.kind(), SourceKind::Url);
    assert_eq!(snapshot.size(), size as i64);
    let mut reader = snapshot.open().unwrap();
    let mut buffer = [0; 8192];
    let mut received = 0;
    loop {
        let count = reader.read(&mut buffer).unwrap();
        if count == 0 {
            break;
        }
        assert!(buffer[..count].iter().all(|byte| *byte == b's'));
        received += count;
    }
    assert_eq!(received, size);
    drop(reader);
    snapshot.close().unwrap();
    assert_eq!(directory.path().read_dir().unwrap().count(), 0);
    join_server(server).await;
}

#[tokio::test]
async fn ordinary_source_automatically_decodes_gzip_without_exposing_encoded_length() {
    let (listener, url) = listener();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        timed(&stream);
        let headers = read_headers(&mut stream);
        let compressed = [
            31, 139, 8, 0, 0, 0, 0, 0, 2, 3, 75, 206, 207, 45, 200, 73, 45, 73, 213, 45, 174, 204,
            43, 201, 72, 45, 201, 76, 214, 45, 206, 47, 45, 74, 78, 5, 0, 200, 64, 245, 192, 25, 0,
            0, 0,
        ];
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", compressed.len()).unwrap();
        stream.write_all(&compressed).unwrap();
        headers
    });
    let transport = ReqwestTransport::new("").unwrap();
    let response = HttpTransport::send(&transport, request(url, Arc::new(Context::background())))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.content_length, -1);
    assert!(!response.headers.contains_key("Content-Encoding"));
    assert!(!response.headers.contains_key("Content-Length"));
    assert_eq!(
        read_body(response).await.unwrap(),
        b"complete-synthetic-source"
    );
    let headers = join_server(server).await.to_ascii_lowercase();
    assert!(headers.contains("accept-encoding: gzip\r\n"));
}

#[tokio::test]
async fn ordinary_go_zero_length_reader_remains_an_unknown_length_stream() {
    let (listener, url) = listener();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        timed(&stream);
        let headers = read_headers(&mut stream);
        let output = body(&mut stream, &headers, None);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .unwrap();
        (headers, output)
    });
    let transport = ReqwestTransport::new("").unwrap();
    let mut upload = request(url, Arc::new(Context::background()));
    upload.method = "POST".into();
    upload.body = Some(Box::new(Cursor::new(b"complete-synthetic-body".to_vec())));
    upload.content_length = 0;
    let response = HttpTransport::send(&transport, upload)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read_body(response).await.unwrap(), b"ok");
    let (headers, output) = join_server(server).await;
    assert!(!headers.to_ascii_lowercase().contains("content-length:"));
    assert!(
        headers
            .to_ascii_lowercase()
            .contains("transfer-encoding: chunked")
    );
    assert_eq!(output, b"complete-synthetic-body");
}

#[tokio::test]
async fn native_go_zero_length_reader_maps_to_raw_unknown_length() {
    let port = Arc::new(NativePort::default());
    let transport = NativeHttpTransport::new(port.clone());
    let mut upload = request(
        "https://ascii2d.invalid/search/file".into(),
        Arc::new(Context::background()),
    );
    upload.method = "POST".into();
    upload.body = Some(Box::new(Cursor::new(b"complete-synthetic-body".to_vec())));
    upload.content_length = 0;
    let response = HttpTransport::send(&transport, upload)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status, 418);
    let requests = port.requests.lock().unwrap();
    assert_eq!(requests[0].content_length, -1);
    assert_eq!(
        requests[0].body.as_deref(),
        Some(b"complete-synthetic-body".as_slice())
    );
}
