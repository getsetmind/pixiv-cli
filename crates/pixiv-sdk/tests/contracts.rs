use pixiv_sdk::{
    Client, Reason, Result,
    oauth::{self, LoginSession},
    reference::artwork_id,
    transport::{Request, Response, Transport},
};
use serde_json::json;
use std::sync::Mutex;

struct Fixture {
    response: Mutex<Option<Response>>,
    requests: Mutex<Vec<Request>>,
}

#[test]
fn transport_response_debug_never_exposes_upstream_artifacts() {
    let response = Response {
        status: 403,
        retry_after_seconds: None,
        body: json!({"cookie":"fixture-upstream-cookie","token":"fixture-upstream-token"}),
    };
    let debug = format!("{response:?}");
    assert!(debug.contains("403"));
    assert!(!debug.contains("fixture-upstream-cookie"));
    assert!(!debug.contains("fixture-upstream-token"));
}

impl Fixture {
    fn new(response: Response) -> Self {
        Self {
            response: Mutex::new(Some(response)),
            requests: Mutex::new(Vec::new()),
        }
    }
}

impl Transport for Fixture {
    async fn send(&self, request: Request) -> Result<Response> {
        self.requests.lock().unwrap().push(request);
        Ok(self
            .response
            .lock()
            .unwrap()
            .take()
            .expect("unexpected request"))
    }
}

#[test]
fn artwork_references_reject_other_hosts_and_embedded_credentials() {
    for input in [
        "0",
        "-1",
        "https://pixiv.net.evil.invalid/artworks/42",
        "https://secret@www.pixiv.net/artworks/42",
        "https://www.pixiv.net:1234/artworks/42",
        "http://www.pixiv.net/artworks/42",
    ] {
        assert_eq!(artwork_id(input).unwrap_err().code, Reason::InvalidArgument);
    }
    assert_eq!(artwork_id("42").unwrap(), 42);
    assert_eq!(
        artwork_id("https://www.pixiv.net/en/artworks/42").unwrap(),
        42
    );
}

#[tokio::test]
async fn missing_credentials_fail_before_content_requests() {
    struct NoRequests;
    impl Transport for NoRequests {
        async fn send(&self, _: Request) -> Result<Response> {
            panic!("unauthenticated content must not issue a request");
        }
    }
    let client = Client::with_transport("", NoRequests);
    assert_eq!(
        client.artwork(42).await.unwrap_err().code,
        Reason::Unauthorized
    );
}

#[tokio::test]
async fn credential_debug_and_json_never_expose_tokens() {
    let transport = Fixture::new(Response {
        status: 200,
        retry_after_seconds: None,
        body: json!({
            "access_token": "fixture-access-secret",
            "refresh_token": "fixture-rotated-secret",
            "expires_in": 3600,
            "user": {"id": "42", "name": "fixture"}
        }),
    });
    let credentials = oauth::refresh(&transport, "fixture-input-secret")
        .await
        .unwrap();
    assert_eq!(credentials.refresh_token(), "fixture-rotated-secret");
    let metadata = serde_json::to_string(&credentials).unwrap();
    let debug = format!("{credentials:?}");
    let request = format!("{:?}", transport.requests.lock().unwrap()[0]);
    for output in [&metadata, &debug, &request] {
        for secret in [
            "fixture-access-secret",
            "fixture-rotated-secret",
            "fixture-input-secret",
        ] {
            assert!(!output.contains(secret));
        }
    }
}

#[tokio::test]
async fn upstream_errors_preserve_retry_advice_without_exposing_response_bodies() {
    let transport = Fixture::new(Response {
        status: 429,
        retry_after_seconds: Some(120),
        body: json!({"error": "fixture-upstream-secret"}),
    });
    let error = oauth::refresh(&transport, "fixture-refresh")
        .await
        .unwrap_err();
    assert_eq!(error.code, Reason::RateLimited);
    assert_eq!(error.retry_after_seconds_at(chrono::Utc::now()), Some(120));
    assert!(!error.to_string().contains("fixture-upstream-secret"));
    assert!(
        !serde_json::to_string(&error)
            .unwrap()
            .contains("fixture-upstream-secret")
    );
}

#[tokio::test]
async fn login_rejects_callbacks_from_other_hosts_before_exchanging_codes() {
    let transport = Fixture::new(Response {
        status: 200,
        retry_after_seconds: None,
        body: json!(null),
    });
    let session = LoginSession::begin().unwrap();
    let error = session
        .complete(
            &transport,
            "https://evil.invalid/callback?code=fixture-secret",
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, Reason::InvalidArgument);
    assert!(transport.requests.lock().unwrap().is_empty());
}
