use pixiv_sdk::{
    Client, Result, oauth,
    transport::{Request, Response, Transport},
};
use serde::Deserialize;
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Deserialize)]
struct Case {
    oauth: bool,
    status: u16,
    retry_after: bool,
    reason: String,
    safe: bool,
    has_after: bool,
    message: String,
}

struct StatusTransport<'a> {
    case: &'a Case,
    calls: &'a AtomicUsize,
}

impl Transport for StatusTransport<'_> {
    async fn send(&self, request: Request) -> Result<Response> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(
            request.operation,
            if self.case.oauth { "Open" } else { "Artwork" }
        );
        Ok(Response {
            status: self.case.status,
            retry_after: self
                .case
                .retry_after
                .then_some(chrono::TimeDelta::seconds(120)),
            body: json!({"error":"fixture-http-secret"}),
        })
    }
}

#[tokio::test(start_paused = true)]
async fn http_status_and_retry_advice_match_go_for_content_and_oauth() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/http-status.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 64);
    for case in cases {
        let calls = AtomicUsize::new(0);
        let transport = StatusTransport {
            case: &case,
            calls: &calls,
        };
        let before = chrono::Utc::now();
        let error = if case.oauth {
            oauth::refresh(&transport, "fixture-refresh")
                .await
                .unwrap_err()
        } else {
            Client::with_transport("fixture-access", transport)
                .artwork(42)
                .await
                .unwrap_err()
        };
        let after = chrono::Utc::now();
        let label = format!(
            "oauth={} status={} retry={}",
            case.oauth, case.status, case.retry_after
        );
        assert_eq!(error.code.as_str(), case.reason, "{label}");
        assert_eq!(error.http_status, Some(case.status), "{label}");
        assert_eq!(error.retry.safe, case.safe, "{label}");
        assert_eq!(error.retry.after.is_some(), case.has_after, "{label}");
        if let Some(deadline) = error.retry.after {
            assert!(
                deadline >= before + chrono::TimeDelta::seconds(120),
                "{label}"
            );
            assert!(
                deadline <= after + chrono::TimeDelta::seconds(120),
                "{label}"
            );
        }
        assert_eq!(error.to_string(), case.message, "{label}");
        assert_eq!(error.product, "pixiv", "{label}");
        assert!(
            error.detail.is_none() && error.transport.is_none(),
            "{label}"
        );
        assert!(!format!("{error:?}").contains("fixture-http-secret"));
        let expected_calls = if !case.oauth && case.status == 429 && case.retry_after {
            2
        } else {
            1
        };
        assert_eq!(calls.load(Ordering::SeqCst), expected_calls, "{label}");
    }
}
