use chrono::{DateTime, Utc};
use pixiv_sdk::error::{
    Cause, Error, Reason, RetryAdvice, TransportKind, is_canceled, is_deadline_exceeded, is_reason,
    matches_reason, reason_of,
};
use serde::Deserialize;
use std::error::Error as StdError;

#[derive(Deserialize)]
struct Case {
    name: String,
    product: String,
    operation: String,
    reason: Reason,
    detail: String,
    cause: String,
    http_status: u16,
    transport: String,
    retry_safe: bool,
    retry_after: Option<DateTime<Utc>>,
    message: String,
    canceled: bool,
    deadline: bool,
    matches: Vec<Reason>,
    retry_seconds: Option<u64>,
}
#[derive(Deserialize)]
struct Contract {
    now: DateTime<Utc>,
    cases: Vec<Case>,
}

#[test]
fn classified_errors_match_go_messages_reasons_causes_and_retry_metadata() {
    let contract: Contract = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/errors.json"
    ))
    .unwrap();
    for case in contract.cases {
        assert!(Reason::ALL.contains(&case.reason), "{}", case.name);
        let product = match case.product.as_str() {
            "pixiv" => "pixiv",
            "fanbox" => "fanbox",
            "" => "",
            _ => panic!("unexpected product"),
        };
        let operation = match case.operation.as_str() {
            "Artwork" => "Artwork",
            "Post" => "Post",
            "ParseResourceRef" => "ParseResourceRef",
            _ => panic!("unexpected operation"),
        };
        let mut error = Error::with_product(product, case.reason, operation)
            .with_http_status(case.http_status)
            .with_retry(RetryAdvice {
                safe: case.retry_safe,
                after: case.retry_after,
            });
        if !case.detail.is_empty() {
            assert_eq!(case.detail, "status 502");
            error = error.with_detail("status 502");
        }
        if !case.transport.is_empty() {
            error = error.with_transport(
                serde_json::from_value::<TransportKind>(serde_json::json!(case.transport)).unwrap(),
            );
        }
        let cause = match case.cause.as_str() {
            "" => None,
            "redacted" => Some(Cause::Redacted("redacted local classification".into())),
            "empty" => Some(Cause::Redacted("".into())),
            "canceled" => Some(Cause::Canceled),
            "deadline" => Some(Cause::DeadlineExceeded),
            "nested" => Some(Cause::Classified(Box::new(Error::with_product(
                "fanbox",
                Reason::Forbidden,
                "Post",
            )))),
            _ => panic!("unknown cause"),
        };
        if let Some(cause) = cause {
            error = error.with_cause(cause);
        }
        assert_eq!(error.to_string(), case.message, "{}", case.name);
        assert_eq!(error.product, case.product, "{}", case.name);
        assert_eq!(error.operation, case.operation, "{}", case.name);
        assert_eq!(
            error.http_status,
            (case.http_status != 0).then_some(case.http_status),
            "{}",
            case.name
        );
        assert_eq!(
            error
                .transport
                .map(|kind| serde_json::to_value(kind)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned())
                .unwrap_or_default(),
            case.transport,
            "{}",
            case.name
        );
        assert_eq!(error.retry.safe, case.retry_safe, "{}", case.name);
        assert_eq!(error.retry.after, case.retry_after, "{}", case.name);
        assert_eq!(reason_of(&error), Some(case.reason), "{}", case.name);
        assert_eq!(is_canceled(&error), case.canceled, "{}", case.name);
        assert_eq!(is_deadline_exceeded(&error), case.deadline, "{}", case.name);
        assert_eq!(
            error.retry_after_seconds_at(contract.now),
            case.retry_seconds,
            "{}",
            case.name
        );
        for reason in Reason::ALL {
            assert_eq!(
                matches_reason(&error, *reason),
                case.matches.contains(reason),
                "{}: {reason}",
                case.name
            );
            assert_eq!(
                is_reason(&error, *reason),
                case.reason == *reason,
                "{}: {reason}",
                case.name
            );
        }
        assert_eq!(
            error.source().is_some(),
            !case.cause.is_empty(),
            "{}",
            case.name
        );
    }
}

#[test]
fn unrelated_errors_have_no_sdk_classification_or_cancellation() {
    let error = std::io::Error::other("fixture-secret");
    assert_eq!(reason_of(&error), None);
    assert!(!is_reason(&error, Reason::LocalStateError));
    assert!(!is_canceled(&error));
    assert!(!is_deadline_exceeded(&error));
}

#[test]
fn retry_advice_keeps_commit_safety_independent_from_backoff() {
    let now = Utc::now();
    let error = Error::new(Reason::RateLimited, "Artwork").with_retry(RetryAdvice {
        safe: false,
        after: Some(now),
    });
    assert!(!error.retry.safe);
    assert_eq!(error.retry_after_seconds_at(now), Some(0));
}

#[derive(Debug)]
struct WrappedError(Error);

impl std::fmt::Display for WrappedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "wrapped: {}", self.0)
    }
}

impl StdError for WrappedError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(&self.0)
    }
}

#[test]
fn classification_and_cancellation_survive_external_error_wrappers() {
    let error = WrappedError(Error::new(Reason::UpstreamError, "Artwork").with_cause(
        Cause::Classified(Box::new(
            Error::new(Reason::Forbidden, "Artwork").with_cause(Cause::Canceled),
        )),
    ));
    assert_eq!(reason_of(&error), Some(Reason::UpstreamError));
    assert!(is_reason(&error, Reason::UpstreamError));
    assert!(!is_reason(&error, Reason::Forbidden));
    assert!(matches_reason(&error, Reason::Forbidden));
    assert!(is_canceled(&error));
    assert!(!is_deadline_exceeded(&error));
}
