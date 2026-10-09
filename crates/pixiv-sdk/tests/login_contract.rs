use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{TimeDelta, Utc};
use pixiv_sdk::{
    Error, Reason, Result,
    oauth::{LoginSession, is_official_oauth_callback_url, is_official_oauth_start_url},
    transport::{Request, Response, Transport},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

struct Fixture {
    body: Value,
    status: u16,
    requests: Mutex<Vec<Request>>,
}
impl Fixture {
    fn new(body: Value, status: u16) -> Self {
        Self {
            body,
            status,
            requests: Mutex::new(vec![]),
        }
    }
}
impl Transport for Fixture {
    async fn send(&self, request: Request) -> Result<Response> {
        self.requests.lock().unwrap().push(request);
        tokio::task::yield_now().await;
        Ok(Response {
            body: self.body.clone(),
            status: self.status,
            retry_after: Some(TimeDelta::seconds(120)),
        })
    }
}
fn success() -> Value {
    json!({"access_token":"fixture-access","refresh_token":"fixture-refresh","expires_in":3600,"user":{"id":"42","name":"fixture"}})
}
fn state(session: &LoginSession) -> String {
    url::Url::parse(session.authorization_url())
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1
        .into_owned()
}

#[tokio::test]
async fn callback_contract_matches_go_first_query_value_and_state_policy() {
    let probe = LoginSession::begin().unwrap();
    let cases = [
        ("pixiv://account/login?code=x".to_owned(), Some("x"), true),
        (
            "https://app-api.pixiv.net/web/v1/users/auth/pixiv/callback?code=x".to_owned(),
            Some("x"),
            true,
        ),
        (
            "https://example.invalid/callback?code=x&state=SESSION".to_owned(),
            Some("x"),
            true,
        ),
        (
            "custom:opaque?code=x&state=SESSION".to_owned(),
            Some("x"),
            true,
        ),
        (
            "pixiv://account/login?code=x&code=y".to_owned(),
            Some("x"),
            true,
        ),
        (
            "pixiv://account/login?code=%20x%20&state=%20SESSION%20".to_owned(),
            Some("x"),
            true,
        ),
        (
            "pixiv://user@account/login?code=x&state=SESSION".to_owned(),
            Some("x"),
            true,
        ),
        (
            "pixiv://account:443/login?code=x&state=SESSION".to_owned(),
            Some("x"),
            true,
        ),
        (
            "custom://%C3%A9/cb?code=x&state=SESSION".to_owned(),
            Some("x"),
            true,
        ),
        (
            "custom://[fe80::1%25eth0]/cb?code=x&state=SESSION".to_owned(),
            Some("x"),
            true,
        ),
        (
            "custom://%61/cb?code=x&state=SESSION".to_owned(),
            None,
            false,
        ),
        (
            "custom://[fe80::1%25é]/cb?code=x&state=SESSION".to_owned(),
            Some("x"),
            true,
        ),
        (
            "custom://[fe80::1%25%C3%A9]/cb?code=x&state=SESSION".to_owned(),
            None,
            false,
        ),
        (
            "custom://[fe80::1%25]/cb?code=x&state=SESSION".to_owned(),
            None,
            false,
        ),
        (
            "custom://[bad]/cb?code=x&state=SESSION".to_owned(),
            None,
            false,
        ),
        (
            "https://<@app-api.pixiv.net/web/v1/users/auth/pixiv/callback?code=x".to_owned(),
            None,
            false,
        ),
        (
            "custom://bad\\host/cb?code=x&state=SESSION".to_owned(),
            None,
            false,
        ),
        (
            "https://<@app-api.pixiv.net/web/v1/users/auth/pixiv/callback".to_owned(),
            Some("https://<@app-api.pixiv.net/web/v1/users/auth/pixiv/callback"),
            false,
        ),
        (
            "custom://bad\\host/cb".to_owned(),
            Some("custom://bad\\host/cb"),
            false,
        ),
        ("pixiv://account/%6cogin?code=x".to_owned(), Some("x"), true),
        (" bare-code ".to_owned(), Some("bare-code"), false),
        ("%zz".to_owned(), Some("%zz"), false),
        ("https://[bad".to_owned(), Some("https://[bad"), false),
        ("".to_owned(), None, false),
        ("relative?code=x".to_owned(), None, false),
        (
            "pixiv://account/login?code=x&state=wrong".to_owned(),
            None,
            false,
        ),
        (
            "https://example.invalid/callback?code=x".to_owned(),
            None,
            false,
        ),
        ("pixiv://account/login?code=%zz".to_owned(), None, false),
        ("pixiv://account/login?code=x;y".to_owned(), None, false),
        ("pixiv://account/login?code=&code=x".to_owned(), None, false),
    ];
    for (input, code, accepts) in cases {
        let session = LoginSession::begin().unwrap();
        let input = input.replace("SESSION", &state(&session));
        assert_eq!(session.accepts_callback_url(&input), accepts, "{input}");
        assert_eq!(session.accepts_callback_url(&input), accepts);
        let fixture = Fixture::new(success(), 200);
        let result = session.complete(&fixture, &input).await;
        if let Some(code) = code {
            assert_eq!(result.unwrap().user_id, 42, "{input}");
            let requests = fixture.requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            assert_eq!(
                requests[0]
                    .parameters
                    .iter()
                    .find(|(k, _)| k == "code")
                    .unwrap()
                    .1,
                code
            );
        } else {
            assert_eq!(result.unwrap_err().code, Reason::InvalidArgument);
            assert!(fixture.requests.lock().unwrap().is_empty());
        }
    }
    assert!(!probe.accepts_callback_url("bare-code"));
}

#[tokio::test]
async fn pkce_request_invalid_retry_shared_gate_and_redaction_match_go() {
    let session = LoginSession::begin().unwrap();
    let fixture = Fixture::new(success(), 200);
    let url = url::Url::parse(session.authorization_url()).unwrap();
    assert_eq!(
        url.origin().ascii_serialization(),
        "https://app-api.pixiv.net"
    );
    assert_eq!(url.path(), "/web/v1/login");
    let query = url
        .query_pairs()
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(query.len(), 4);
    assert_eq!(query["client"], "pixiv-android");
    assert_eq!(query["code_challenge_method"], "S256");
    assert_eq!(query["state"].len(), 43);
    assert_eq!(
        session
            .complete(&fixture, "bad?input")
            .await
            .unwrap_err()
            .code,
        Reason::InvalidArgument
    );
    let copy = session.clone();
    let before = Utc::now();
    let credentials = copy.complete(&fixture, " bare-code ").await.unwrap();
    assert_eq!(credentials.user_id, 42);
    assert_eq!(credentials.username, "fixture");
    assert!((credentials.expires_at - before - TimeDelta::seconds(3600)).num_milliseconds() < 1000);
    assert_eq!(
        session
            .complete(&fixture, "bare-code")
            .await
            .unwrap_err()
            .detail
            .as_deref(),
        Some("login session was already used")
    );
    assert!(session.accepts_callback_url("pixiv://account/login?code=x"));
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.method, reqwest::Method::POST);
    assert_eq!(request.url, "https://oauth.secure.pixiv.net/auth/token");
    assert_eq!(request.operation, "Complete");
    assert_eq!(request.parameters.len(), 7);
    let form = request
        .parameters
        .iter()
        .cloned()
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(form["grant_type"], "authorization_code");
    assert_eq!(form["include_policy"], "true");
    assert_eq!(
        form["redirect_uri"],
        "https://app-api.pixiv.net/web/v1/users/auth/pixiv/callback"
    );
    assert_eq!(form["code_verifier"].len(), 86);
    assert_eq!(
        URL_SAFE_NO_PAD.encode(Sha256::digest(form["code_verifier"].as_bytes())),
        query["code_challenge"]
    );
    assert!(
        !request
            .headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("authorization"))
    );
    for output in [
        format!("{session:?}"),
        session.to_string(),
        format!("{request:?}"),
        format!("{credentials:?}"),
        serde_json::to_string(&credentials).unwrap(),
    ] {
        for secret in [
            &form["code_verifier"],
            query["state"].as_ref(),
            "bare-code",
            "fixture-access",
            "fixture-refresh",
        ] {
            assert!(!output.contains(secret));
        }
    }
}

#[tokio::test]
async fn zero_handle_has_nil_contract() {
    let session = LoginSession::default();
    let fixture = Fixture::new(success(), 200);
    assert_eq!(session.authorization_url(), "");
    assert!(!session.accepts_callback_url("pixiv://account/login?code=x"));
    assert_eq!(
        session
            .complete(&fixture, "x")
            .await
            .unwrap_err()
            .detail
            .as_deref(),
        Some("login session is nil")
    );
    assert!(fixture.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn response_contract_accepts_missing_access_expiry_and_root_fallback() {
    let cases = [
        (json!({"refresh_token":"r","user":{"id":42}}), 200, None),
        (
            json!({"refresh_token":"r","expires_in":-1,"user":{"id":42}}),
            200,
            None,
        ),
        (
            json!({"refresh_token":"r","user":{"id":42},"response":{}}),
            200,
            None,
        ),
        (
            json!({"refresh_token":"r","user":{"id":42},"response":{"user":{"id":7}}}),
            200,
            Some(Reason::MalformedUpstreamResponse),
        ),
        (
            json!({"refresh_token":" ","user":{"id":42}}),
            200,
            Some(Reason::MalformedUpstreamResponse),
        ),
        (Value::Null, 200, Some(Reason::MalformedUpstreamResponse)),
        (json!({}), 200, Some(Reason::MalformedUpstreamResponse)),
        (
            json!({"secret":"fixture-response-secret"}),
            400,
            Some(Reason::CredentialsExpired),
        ),
        (json!({}), 401, Some(Reason::CredentialsExpired)),
        (json!({}), 403, Some(Reason::Forbidden)),
        (json!({}), 429, Some(Reason::RateLimited)),
        (json!({}), 503, Some(Reason::UpstreamError)),
    ];
    for (body, status, reason) in cases {
        let session = LoginSession::begin().unwrap();
        let fixture = Fixture::new(body, status);
        let result = session.complete(&fixture, "fixture-code-secret").await;
        if let Some(reason) = reason {
            let error = result.unwrap_err();
            assert_eq!(error.code, reason);
            assert!(!error.retry.safe);
            assert!(!format!("{error:?}").contains("fixture-response-secret"));
            assert!(!error.to_string().contains("fixture-code-secret"));
        } else {
            let credentials = result.unwrap();
            assert_eq!(credentials.access_token(), "");
            assert_eq!(credentials.expires_at.timestamp(), -62_135_596_800);
        }
        assert_eq!(
            session.complete(&fixture, "x").await.unwrap_err().code,
            Reason::InvalidArgument
        );
        assert_eq!(fixture.requests.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn concurrent_copies_send_exactly_one_request() {
    let session = LoginSession::begin().unwrap();
    let fixture = Fixture::new(success(), 200);
    let copy = session.clone();
    let (a, b) = tokio::join!(
        session.complete(&fixture, "x"),
        copy.complete(&fixture, "x")
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert_eq!(fixture.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn valid_input_consumes_session_on_transport_failure_and_aborted_exchange() {
    struct Failing;
    impl Transport for Failing {
        async fn send(&self, _: Request) -> Result<Response> {
            Err(Error::new(Reason::UpstreamUnavailable, "Complete"))
        }
    }
    let session = LoginSession::begin().unwrap();
    assert_eq!(
        session.complete(&Failing, "x").await.unwrap_err().code,
        Reason::UpstreamUnavailable
    );
    assert_eq!(
        session.complete(&Failing, "x").await.unwrap_err().code,
        Reason::InvalidArgument
    );
    struct Pending(AtomicUsize);
    impl Transport for Pending {
        async fn send(&self, _: Request) -> Result<Response> {
            self.0.fetch_add(1, Ordering::SeqCst);
            std::future::pending().await
        }
    }
    let session = LoginSession::begin().unwrap();
    let pending = Pending(AtomicUsize::new(0));
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(1),
            session.complete(&pending, "x")
        )
        .await
        .is_err()
    );
    assert_eq!(pending.0.load(Ordering::SeqCst), 1);
    assert_eq!(
        session.complete(&pending, "x").await.unwrap_err().code,
        Reason::InvalidArgument
    );
}

#[test]
fn official_url_predicates_are_distinct_from_session_callback_acceptance() {
    for (input, expected) in [
        (
            "https://app-api.pixiv.net/web/v1/users/auth/pixiv/callback",
            true,
        ),
        (
            "HTTPS://APP-API.PIXIV.NET/web/v1/users/auth/pixiv/callback",
            true,
        ),
        (
            "https://user@App-Api.Pixiv.Net/web/v1/users/auth/pixiv/callback",
            true,
        ),
        (
            "https://app-api.pixiv.net/web/v1/users/auth/pixiv/%63allback",
            true,
        ),
        (
            "https://app-api.pixiv.net:443/web/v1/users/auth/pixiv/callback",
            false,
        ),
        (
            "https://app-api.pixiv.net/web/v1/users/auth/pixiv/callback/",
            false,
        ),
        (
            "http://app-api.pixiv.net/web/v1/users/auth/pixiv/callback",
            false,
        ),
        (
            "https://<@app-api.pixiv.net/web/v1/users/auth/pixiv/callback?code=x",
            false,
        ),
        ("https://bad\\host/web/v1/users/auth/pixiv/callback", false),
        (
            "https://example.invalid/web/v1/users/auth/pixiv/callback",
            false,
        ),
    ] {
        assert_eq!(is_official_oauth_callback_url(input), expected, "{input}");
        assert_eq!(
            is_official_oauth_start_url(
                &input
                    .replace("callback", "start")
                    .replace("%63allback", "%73tart")
            ),
            expected
        );
    }
}
