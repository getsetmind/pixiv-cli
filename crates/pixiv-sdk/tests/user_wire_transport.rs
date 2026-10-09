use pixiv_sdk::{
    Client, Reason,
    pixiv::{SearchUsersRequest, UserRequest},
    transport::{HttpTransport, JsonResponse, Request, Response, Transport},
};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{Arc, Mutex},
};

struct Legacy(Value);
impl Transport for Legacy {
    async fn send(&self, _: Request) -> pixiv_sdk::Result<Response> {
        Ok(Response {
            status: 200,
            retry_after: None,
            body: self.0.clone(),
        })
    }
}
#[tokio::test]
async fn existing_value_transports_remain_supported_and_raw_debug_hides_body() {
    let client = Client::with_transport(
        "fixture-access",
        Legacy(json!({"USER_PREVIEWS":[{"USER":{"ID":31,"NAME":"artist"}}]})),
    );
    let page = client
        .search_users(SearchUsersRequest {
            word: "artist".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(page.items[0].user.id, 31);
    assert_eq!(page.items[0].user.name, "artist");
    let response = JsonResponse {
        status: 200,
        retry_after: None,
        body: b"fixture-response-secret".to_vec(),
    };
    assert!(!format!("{response:?}").contains("fixture-response-secret"));
}

fn server(body: &str, status: &str) -> (String, std::thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let thread = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        let mut byte = [0];
        while !request.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        stream.write_all(response.as_bytes()).unwrap();
        String::from_utf8(request).unwrap()
    });
    (format!("http://{address}"), thread)
}
struct Local {
    address: String,
    http: HttpTransport,
}
impl Transport for Local {
    async fn send(&self, _: Request) -> pixiv_sdk::Result<Response> {
        panic!("raw user operation used legacy send")
    }
    async fn send_json(&self, mut request: Request) -> pixiv_sdk::Result<JsonResponse> {
        assert_eq!(request.url, "https://app-api.pixiv.net/v1/search/user");
        request.url = format!("{}/v1/search/user", self.address);
        self.http.send_json(request).await
    }
}
#[tokio::test]
async fn real_http_response_preserves_duplicate_order_and_ignored_large_numbers() {
    let body = r#"{"USER_PREVIEWS":[{"USER":{"ID":31,"name":"first","NAME":"last","profile_image_urls":{"medium":"https://i.pximg.net/profile.jpg"},"PROFILE_IMAGE_URLS":null},"user":{"ACCOUNT":"account"}}],"unknown":9e999}"#;
    let (address, thread) = server(body, "200 OK");
    let client = Client::with_transport(
        "fixture-access",
        Local {
            address,
            http: HttpTransport::new(None).unwrap(),
        },
    );
    let page = client
        .search_users(SearchUsersRequest {
            word: "artist".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(page.items[0].user.name, "last");
    assert_eq!(page.items[0].user.account, "account");
    assert_eq!(page.items[0].user.profile_image.variant, "medium");
    let request = thread.join().unwrap();
    assert!(request.starts_with("GET /v1/search/user?word=artist HTTP/1.1\r\n"));
    assert!(
        request
            .to_ascii_lowercase()
            .contains("authorization: bearer fixture-access\r\n")
    );
}

struct Retry {
    calls: Arc<Mutex<usize>>,
    statuses: Vec<u16>,
    retry: bool,
}
impl Transport for Retry {
    async fn send(&self, _: Request) -> pixiv_sdk::Result<Response> {
        panic!("raw user operation used legacy send")
    }
    async fn send_json(&self, request: Request) -> pixiv_sdk::Result<JsonResponse> {
        assert_eq!(request.operation, "User");
        let mut calls = self.calls.lock().unwrap();
        let status = self.statuses[*calls];
        *calls += 1;
        let body = if status == 200 {
            br#"{"USER":{"ID":31,"NAME":"artist"},"PROFILE":{},"PROFILE_PUBLICITY":{},"WORKSPACE":{}}"#.to_vec()
        } else {
            b"malformed fixture-response-secret".to_vec()
        };
        Ok(JsonResponse {
            status,
            retry_after: self.retry.then(|| chrono::TimeDelta::seconds(0)),
            body,
        })
    }
}
#[tokio::test(start_paused = true)]
async fn raw_user_status_and_retry_keep_existing_classification_and_decode_order() {
    for (statuses, retry, reason, expected_calls) in [
        (vec![429, 200], true, None, 2),
        (vec![429], false, Some(Reason::RateLimited), 1),
        (vec![401], false, Some(Reason::CredentialsExpired), 1),
        (vec![400], false, Some(Reason::InvalidArgument), 1),
        (vec![403], false, Some(Reason::Forbidden), 1),
        (vec![404], false, Some(Reason::NotFound), 1),
        (vec![410], false, Some(Reason::ContentUnavailable), 1),
        (vec![500], false, Some(Reason::UpstreamError), 1),
    ] {
        let calls = Arc::new(Mutex::new(0));
        let client = Client::with_transport(
            "fixture-access",
            Retry {
                calls: calls.clone(),
                statuses,
                retry,
            },
        );
        let result = client.user(UserRequest { user_id: 31 }).await;
        if let Some(reason) = reason {
            let error = result.unwrap_err();
            assert_eq!(error.code, reason);
            assert!(!format!("{error:?}").contains("fixture-response-secret"));
        } else {
            assert_eq!(result.unwrap().user.name, "artist");
        }
        assert_eq!(*calls.lock().unwrap(), expected_calls);
    }
}
