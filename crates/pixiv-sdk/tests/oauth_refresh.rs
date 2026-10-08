use pixiv_sdk::{
    Result, oauth,
    transport::{Request, Response, Transport},
};
use serde::Deserialize;
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Deserialize)]
struct Case {
    name: String,
    input: String,
    body: String,
    calls: usize,
    message: String,
    access: String,
    refresh: String,
    uid: i64,
    username: String,
    expiry_seconds: i64,
}

struct Reply<'a> {
    case: &'a Case,
    calls: AtomicUsize,
}

impl Transport for Reply<'_> {
    async fn send(&self, request: Request) -> Result<Response> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(request.method, reqwest::Method::POST);
        assert_eq!(request.url, "https://oauth.secure.pixiv.net/auth/token");
        assert_eq!(request.operation, "Open");
        assert!(
            request
                .headers
                .iter()
                .any(|(name, value)| name.eq_ignore_ascii_case("content-type")
                    && value == "application/x-www-form-urlencoded")
        );
        assert!(
            !request
                .headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("authorization"))
        );
        assert_eq!(request.parameters.len(), 5);
        for (key, expected) in [
            ("refresh_token", self.case.input.trim()),
            ("grant_type", "refresh_token"),
            ("include_policy", "true"),
        ] {
            assert_eq!(
                request
                    .parameters
                    .iter()
                    .find(|(k, _)| k == key)
                    .map(|(_, v)| v.as_str()),
                Some(expected)
            );
        }
        Ok(Response {
            status: 200,
            retry_after: None,
            body: serde_json::from_str(&self.case.body).unwrap_or(Value::Null),
        })
    }
}

#[tokio::test]
async fn refresh_preserves_go_opaque_tokens_nested_fallback_and_zero_expiry() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/oauth-refresh.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 30);
    let mut differences = Vec::new();
    for case in cases {
        let reply = Reply {
            case: &case,
            calls: AtomicUsize::new(0),
        };
        let before = chrono::Utc::now();
        let result = oauth::refresh(&reply, &case.input).await;
        let after = chrono::Utc::now();
        let message = result
            .as_ref()
            .err()
            .map(ToString::to_string)
            .unwrap_or_default();
        if message != case.message || reply.calls.load(Ordering::SeqCst) != case.calls {
            differences.push(format!(
                "{}: expected {:?}/{} calls, received {:?}/{} calls",
                case.name,
                case.message,
                case.calls,
                message,
                reply.calls.load(Ordering::SeqCst)
            ));
            continue;
        }
        if let Ok(credentials) = result {
            assert_eq!(credentials.access_token(), case.access, "{}", case.name);
            assert_eq!(credentials.refresh_token(), case.refresh, "{}", case.name);
            assert_eq!(credentials.user_id, case.uid, "{}", case.name);
            assert_eq!(credentials.username, case.username, "{}", case.name);
            if case.expiry_seconds == 0 {
                assert_eq!(
                    credentials.expires_at,
                    chrono::DateTime::parse_from_rfc3339("0001-01-01T00:00:00Z").unwrap()
                );
            } else {
                let duration = chrono::TimeDelta::seconds(case.expiry_seconds);
                assert!(
                    credentials.expires_at >= before + duration
                        && credentials.expires_at <= after + duration,
                    "{}",
                    case.name
                );
            }
        }
    }
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}
