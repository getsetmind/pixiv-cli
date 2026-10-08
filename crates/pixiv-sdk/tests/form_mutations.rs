use pixiv_sdk::{
    Client, Result,
    pixiv::*,
    transport::{Request, Response, Transport},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

#[derive(Deserialize)]
struct Case {
    operation: String,
    id: i64,
    restrict: String,
    tags: Option<Vec<String>>,
    status: u16,
    body: String,
    error: Option<Value>,
    requests: Vec<String>,
}
struct Fixture {
    status: u16,
    body: String,
    seen: Arc<Mutex<Vec<String>>>,
}
impl Transport for Fixture {
    async fn send(&self, mut request: Request) -> Result<Response> {
        assert_eq!(request.method, reqwest::Method::POST);
        assert!(
            request
                .headers
                .contains(&("Authorization".into(), "Bearer fixture-access".into()))
        );
        request
            .parameters
            .sort_by(|left, right| left.0.cmp(&right.0));
        let form = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(&request.parameters)
            .finish();
        self.seen.lock().unwrap().push(format!(
            "POST {} {form}",
            request
                .url
                .strip_prefix("https://app-api.pixiv.net")
                .unwrap()
        ));
        Ok(Response {
            status: self.status,
            retry_after: Some(chrono::TimeDelta::zero()),
            body: Value::String(self.body.clone()),
        })
    }
}
#[tokio::test]
async fn mutation_validation_forms_aliases_and_statuses_match_go_without_replay() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/form-mutations.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 178);
    for case in cases {
        let seen = Arc::new(Mutex::new(vec![]));
        let client = Client::with_transport(
            "fixture-access",
            Fixture {
                status: case.status,
                body: case.body,
                seen: seen.clone(),
            },
        );
        let result = match case.operation.as_str() {
            "AddArtworkBookmark" => {
                client
                    .add_artwork_bookmark(AddArtworkBookmarkRequest {
                        artwork_id: case.id,
                        restrict: case.restrict,
                        tags: case.tags.unwrap_or_default(),
                    })
                    .await
            }
            "AddBookmark" => {
                client
                    .add_bookmark(AddBookmarkRequest {
                        artwork_id: case.id,
                        restrict: case.restrict,
                        tags: case.tags.unwrap_or_default(),
                    })
                    .await
            }
            "RemoveArtworkBookmark" => {
                client
                    .remove_artwork_bookmark(RemoveArtworkBookmarkRequest {
                        artwork_id: case.id,
                    })
                    .await
            }
            "RemoveBookmark" => {
                client
                    .remove_bookmark(RemoveBookmarkRequest {
                        artwork_id: case.id,
                    })
                    .await
            }
            "AddNovelBookmark" => {
                client
                    .add_novel_bookmark(AddNovelBookmarkRequest {
                        novel_id: case.id,
                        restrict: case.restrict,
                        tags: case.tags.unwrap_or_default(),
                    })
                    .await
            }
            "RemoveNovelBookmark" => {
                client
                    .remove_novel_bookmark(RemoveNovelBookmarkRequest { novel_id: case.id })
                    .await
            }
            "FollowUser" => {
                client
                    .follow_user(FollowUserRequest {
                        user_id: case.id,
                        restrict: case.restrict,
                    })
                    .await
            }
            "UnfollowUser" => {
                client
                    .unfollow_user(UnfollowUserRequest { user_id: case.id })
                    .await
            }
            "SetAIArtworkVisibility" => {
                client
                    .set_ai_artwork_visibility(SetAiArtworkVisibilityRequest {
                        visible: case.id != 0,
                    })
                    .await
            }
            _ => panic!("unknown operation"),
        };
        let actual=result.err().map(|error|json!({"Reason":error.code.as_str(),"Product":error.product,"Operation":error.operation,"Detail":error.detail.unwrap_or_default(),"HTTPStatus":error.http_status.unwrap_or_default(),"Transport":error.transport.map(|kind|serde_json::to_value(kind).unwrap().as_str().unwrap().to_owned()).unwrap_or_default(),"Retry":{"Safe":error.retry.safe,"After":error.retry.after.map(|date|date.to_rfc3339()).unwrap_or_else(||"0001-01-01T00:00:00Z".into()),"HasAfter":error.retry.after.is_some()}}));
        assert_eq!(actual, case.error, "{} id={}", case.operation, case.id);
        assert_eq!(*seen.lock().unwrap(), case.requests, "{}", case.operation);
    }
}

#[tokio::test]
async fn form_http_transport_accepts_non_json_success_and_encodes_repeated_tags() {
    use pixiv_sdk::transport::HttpTransport;
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };
    for (status, body) in [
        (200, ""),
        (200, "not JSON"),
        (200, "{broken"),
        (200, "{\"error\":true}"),
        (204, ""),
        (429, "fixture private body"),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let body = body.to_owned();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut head = vec![];
            while !head.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                head.push(byte[0]);
                assert!(head.len() < 65536);
            }
            let head = String::from_utf8(head).unwrap();
            let length: usize = head
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .map(str::to_owned)
                })
                .unwrap()
                .parse()
                .unwrap();
            let mut form = vec![0; length];
            stream.read_exact(&mut form).unwrap();
            assert!(head.starts_with("POST /mutation HTTP/1.1"));
            assert!(
                head.to_ascii_lowercase()
                    .contains("content-type: application/x-www-form-urlencoded")
            );
            assert_eq!(
                url::form_urlencoded::parse(&form)
                    .into_owned()
                    .collect::<Vec<_>>(),
                vec![
                    ("tags[]".into(), "日本語".into()),
                    ("tags[]".into(), "".into())
                ]
            );
            write!(stream,"HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nContent-Type: text/plain\r\nRetry-After: 0\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        });
        let response = HttpTransport::new(None)
            .unwrap()
            .post_form(Request {
                method: reqwest::Method::POST,
                url: format!("http://{address}/mutation"),
                headers: vec![],
                parameters: vec![
                    ("tags[]".into(), "日本語".into()),
                    ("tags[]".into(), "".into()),
                ],
                operation: "AddArtworkBookmark",
            })
            .await
            .unwrap();
        assert_eq!(response.status, status);
        assert!(response.body.is_null());
        assert!(response.retry_after.is_none());
        server.join().unwrap();
    }
}

#[tokio::test(start_paused = true)]
async fn form_mutations_use_configured_pacing_between_requests() {
    let client = Client::with_transport(
        "fixture-access",
        Fixture {
            status: 200,
            body: String::new(),
            seen: Arc::new(Mutex::new(vec![])),
        },
    )
    .with_pacing(std::time::Duration::from_secs(2));
    let start = tokio::time::Instant::now();
    client
        .add_artwork_bookmark(AddArtworkBookmarkRequest {
            artwork_id: 42,
            ..Default::default()
        })
        .await
        .unwrap();
    client
        .remove_artwork_bookmark(RemoveArtworkBookmarkRequest { artwork_id: 42 })
        .await
        .unwrap();
    assert_eq!(start.elapsed(), std::time::Duration::from_secs(2));
}
