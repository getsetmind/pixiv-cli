use pixiv_sdk::{
    Client, Result,
    pixiv::*,
    transport::{JsonResponse, Request, Response, Transport},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
#[derive(Deserialize)]
struct Case {
    name: String,
    operation: String,
    id: i64,
    comment: String,
    parent_id: i64,
    stamp_id: i64,
    status: u16,
    body: String,
    comment_id: i64,
    error: Option<Value>,
    requests: Vec<String>,
}
struct Fixture {
    status: u16,
    body: String,
    seen: Arc<Mutex<Vec<String>>>,
}
impl Fixture {
    fn record(&self, mut request: Request) {
        assert_eq!(request.method, reqwest::Method::POST);
        assert!(
            request
                .headers
                .contains(&("Authorization".into(), "Bearer fixture-access".into()))
        );
        request.parameters.sort_by(|a, b| a.0.cmp(&b.0));
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
    }
}
impl Transport for Fixture {
    async fn send(&self, request: Request) -> Result<Response> {
        self.record(request);
        Ok(Response {
            status: self.status,
            retry_after: None,
            body: Value::Null,
        })
    }
    async fn send_json(&self, request: Request) -> Result<JsonResponse> {
        self.record(request);
        Ok(JsonResponse {
            status: self.status,
            retry_after: None,
            body: self.body.as_bytes().to_vec(),
        })
    }
}
#[tokio::test]
async fn comment_mutation_forms_validation_raw_response_and_errors_match_go() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/comment-mutations.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 136);
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
            "PostArtworkComment" => client
                .post_artwork_comment(PostArtworkCommentRequest {
                    artwork_id: case.id,
                    comment: case.comment,
                })
                .await
                .map(|result| result.comment_id),
            "ReplyArtworkComment" => client
                .reply_artwork_comment(ReplyArtworkCommentRequest {
                    artwork_id: case.id,
                    comment: case.comment,
                    parent_comment_id: case.parent_id,
                })
                .await
                .map(|result| result.comment_id),
            "StampArtworkComment" => client
                .stamp_artwork_comment(StampArtworkCommentRequest {
                    artwork_id: case.id,
                    comment: case.comment,
                    stamp_id: case.stamp_id,
                })
                .await
                .map(|result| result.comment_id),
            "DeleteArtworkComment" => client
                .delete_artwork_comment(DeleteArtworkCommentRequest {
                    comment_id: case.id,
                })
                .await
                .map(|()| 0),
            "PostNovelComment" => client
                .post_novel_comment(PostNovelCommentRequest {
                    novel_id: case.id,
                    comment: case.comment,
                })
                .await
                .map(|result| result.comment_id),
            "ReplyNovelComment" => client
                .reply_novel_comment(ReplyNovelCommentRequest {
                    novel_id: case.id,
                    comment: case.comment,
                    parent_comment_id: case.parent_id,
                })
                .await
                .map(|result| result.comment_id),
            "StampNovelComment" => client
                .stamp_novel_comment(StampNovelCommentRequest {
                    novel_id: case.id,
                    comment: case.comment,
                    stamp_id: case.stamp_id,
                })
                .await
                .map(|result| result.comment_id),
            "DeleteNovelComment" => client
                .delete_novel_comment(DeleteNovelCommentRequest {
                    comment_id: case.id,
                })
                .await
                .map(|()| 0),
            _ => panic!("unknown operation"),
        };
        let id = result.as_ref().copied().unwrap_or_default();
        let actual=result.err().map(|error|json!({"Reason":error.code.as_str(),"Product":error.product,"Operation":error.operation,"Detail":error.detail.unwrap_or_default(),"HTTPStatus":error.http_status.unwrap_or_default(),"Transport":error.transport.map(|kind|serde_json::to_value(kind).unwrap().as_str().unwrap().to_owned()).unwrap_or_default(),"Retry":{"Safe":error.retry.safe,"After":error.retry.after.map(|date|date.to_rfc3339()).unwrap_or_else(||"0001-01-01T00:00:00Z".into()),"HasAfter":error.retry.after.is_some()}}));
        assert_eq!(id, case.comment_id, "{} ID", case.name);
        assert_eq!(actual, case.error, "{} error", case.name);
        assert_eq!(
            *seen.lock().unwrap(),
            case.requests,
            "{} requests",
            case.name
        );
    }
}

struct HeaderFixture {
    expected_user: &'static str,
    expected_language: String,
    seen: Arc<Mutex<usize>>,
}
impl HeaderFixture {
    fn record(&self, request: &Request) {
        let value = |name: &str| {
            request
                .headers
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
                .unwrap_or_default()
        };
        assert_eq!(value("X-User-Id"), self.expected_user);
        assert_eq!(value("Accept-Language"), self.expected_language);
        *self.seen.lock().unwrap() += 1;
    }
}
impl Transport for HeaderFixture {
    async fn send(&self, request: Request) -> Result<Response> {
        if request.url == "https://oauth.secure.pixiv.net/auth/token" {
            return Ok(Response {
                status: 200,
                retry_after: None,
                body: json!({"access_token":"fixture-access","refresh_token":"fixture-refresh","expires_in":3600,"user":{"id":"31","name":"writer"}}),
            });
        }
        self.record(&request);
        if request.url.ends_with("/v1/stamps") {
            assert_eq!(request.method, reqwest::Method::GET);
            *self.seen.lock().unwrap() -= 1;
            return Ok(Response {
                status: 200,
                retry_after: None,
                body: json!({"stamps":[]}),
            });
        }
        assert_eq!(request.method, reqwest::Method::POST);
        Ok(Response {
            status: 429,
            retry_after: Some(chrono::TimeDelta::seconds(60)),
            body: Value::Null,
        })
    }
    async fn send_json(&self, request: Request) -> Result<JsonResponse> {
        self.record(&request);
        if request.url.ends_with("/v1/stamps") {
            assert_eq!(request.method, reqwest::Method::GET);
            *self.seen.lock().unwrap() -= 1;
            return Ok(JsonResponse {
                status: 200,
                retry_after: None,
                body: b"{\"stamps\":[]}".to_vec(),
            });
        }
        assert_eq!(request.method, reqwest::Method::POST);
        Ok(JsonResponse {
            status: 429,
            retry_after: Some(chrono::TimeDelta::seconds(60)),
            body: b"private sentinel".to_vec(),
        })
    }
}
#[tokio::test]
async fn comment_mutations_preserve_verified_headers_and_distinct_retry_advice_without_replay() {
    for verified in [false, true] {
        for language in ["", "  ", "  ja-JP  "] {
            for deletion in [false, true] {
                let seen = Arc::new(Mutex::new(0));
                let fixture = HeaderFixture {
                    expected_user: if verified { "31" } else { "" },
                    expected_language: language.trim().into(),
                    seen: seen.clone(),
                };
                let client = if verified {
                    let credentials = pixiv_sdk::oauth::refresh(&fixture, "fixture-refresh")
                        .await
                        .unwrap();
                    Client::from_credentials(&credentials, fixture)
                } else {
                    Client::with_transport("fixture-access", fixture)
                }
                .with_accept_language(language);
                client
                    .stamps(pixiv_sdk::StampsRequest::default())
                    .await
                    .unwrap();
                let before = chrono::Utc::now();
                let error = if deletion {
                    client
                        .delete_artwork_comment(DeleteArtworkCommentRequest { comment_id: 42 })
                        .await
                        .unwrap_err()
                } else {
                    client
                        .post_artwork_comment(PostArtworkCommentRequest {
                            artwork_id: 42,
                            comment: "body".into(),
                        })
                        .await
                        .unwrap_err()
                };
                assert_eq!(error.code, pixiv_sdk::Reason::RateLimited);
                assert_eq!(error.http_status, Some(429));
                assert_eq!(*seen.lock().unwrap(), 1);
                if deletion {
                    assert!(!error.retry.safe);
                    assert!(error.retry.after.is_none());
                } else {
                    assert!(error.retry.safe);
                    let after = error.retry.after.unwrap();
                    assert!(after >= before + chrono::TimeDelta::seconds(60));
                    assert!(after <= chrono::Utc::now() + chrono::TimeDelta::seconds(60));
                }
            }
        }
    }
}

#[tokio::test]
async fn form_json_http_transport_retains_duplicate_response_bytes_and_form_encoding() {
    use pixiv_sdk::transport::HttpTransport;
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let body = r#"{"comment_id":71,"COMMENT_ID":null,"comment":{"id":72}}"#;
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
        assert!(head.starts_with("POST /comment HTTP/1.1"));
        assert!(
            head.to_ascii_lowercase()
                .contains("content-type: application/x-www-form-urlencoded")
        );
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
        assert_eq!(
            url::form_urlencoded::parse(&form)
                .into_owned()
                .collect::<Vec<_>>(),
            vec![
                ("comment".into(), "日本語 &+\n".into()),
                ("stamp_id".into(), "9".into())
            ]
        );
        write!(stream,"HTTP/1.1 200 Fixture\r\nContent-Length: {}\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
    });
    let response = HttpTransport::new(None)
        .unwrap()
        .send_json(Request {
            method: reqwest::Method::POST,
            url: format!("http://{address}/comment"),
            headers: vec![],
            parameters: vec![
                ("comment".into(), "日本語 &+\n".into()),
                ("stamp_id".into(), "9".into()),
            ],
            operation: "StampArtworkComment",
        })
        .await
        .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.body, body.as_bytes());
    server.join().unwrap();
}
