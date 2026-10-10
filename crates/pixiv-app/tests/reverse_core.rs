use pixiv_app::reverse_search::{Loader, SourceKind, SourceLoader, SourceLoaderOptions};
use pixiv_sdk::context::Context;
use std::{io::Read, sync::Arc};

#[tokio::test]
async fn source_snapshot_preserves_owned_bytes_and_independent_readers() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("owned-source.bin");
    let snapshots = root.path().join("snapshots");
    std::fs::create_dir(&snapshots).unwrap();
    std::fs::write(&source, [0, 255, 254, 13, 10, 34]).unwrap();
    let loader = Loader::new(SourceLoaderOptions {
        temp_dir: snapshots.clone(),
        ..SourceLoaderOptions::default()
    });
    let snapshot = loader
        .load(Arc::new(Context::background()), source.to_str().unwrap())
        .await
        .unwrap();
    std::fs::write(&source, b"changed after snapshot").unwrap();
    assert_eq!(snapshot.kind(), SourceKind::File);
    assert_eq!(snapshot.size(), 6);
    let mut first = snapshot.open().unwrap();
    let mut second = snapshot.open().unwrap();
    let mut first_byte = [0];
    first.read_exact(&mut first_byte).unwrap();
    let mut independent = Vec::new();
    second.read_to_end(&mut independent).unwrap();
    assert_eq!(first_byte, [0]);
    assert_eq!(independent, [0, 255, 254, 13, 10, 34]);
    drop(first);
    drop(second);
    snapshot.close().unwrap();
    assert!(snapshot.open().is_err());
    snapshot.close().unwrap();
    assert_eq!(std::fs::read_dir(snapshots).unwrap().count(), 0);
}

#[cfg(target_os = "linux")]
mod reverse_core_support;

#[test]
fn sealed_core_fixture_preserves_all_ninety_nine_contract_rows() {
    let value: serde_json::Value = serde_json::from_str(include_str!(
        "../../pixiv-cli/tests/fixtures/reverse-search-core.json"
    ))
    .unwrap();
    assert_eq!(
        value["reference"],
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    assert_eq!(value["environment"], "linux/amd64");
    let cases = value["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 99);
    let mut counts = std::collections::BTreeMap::new();
    for row in cases {
        *counts
            .entry(row["operation"].as_str().unwrap())
            .or_insert(0) += 1;
    }
    assert_eq!(
        counts,
        std::collections::BTreeMap::from([
            ("facade_aggregate", 43),
            ("facade_lifecycle", 16),
            ("snapshot_lifecycle", 2),
            ("source_load", 33),
            ("source_redirect", 5)
        ])
    );
    assert!(
        value["boundaries"]["representation"]
            .as_str()
            .unwrap()
            .contains("nil context")
    );
    let unavailable: Vec<_> = cases
        .iter()
        .filter(|row| {
            row["input"]["context"] == "nil"
                || row["input"]["nil_facade"] == true
                || matches!(
                    row["input"]["state"].as_str(),
                    Some("final-userinfo" | "final-unsupported" | "request-nil")
                )
        })
        .map(|row| {
            format!(
                "{}:{}",
                row["operation"].as_str().unwrap(),
                row["name"].as_str().unwrap()
            )
        })
        .collect();
    assert_eq!(
        unavailable,
        vec![
            "facade_aggregate:nil-context",
            "facade_lifecycle:nil-context-before-ports",
            "facade_lifecycle:nil-facade",
            "facade_lifecycle:nil-facade-nil-context",
            "source_load:file-nil-context",
            "source_load:url-final-request-nil",
            "source_load:url-final-unsupported",
            "source_load:url-final-userinfo"
        ]
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn source_runtime_matches_twenty_nine_representable_frozen_rows() {
    let fixture = reverse_core_support::fixture();
    assert_eq!(
        fixture.reference,
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    assert_eq!(fixture.environment, "linux/amd64");
    assert!(
        fixture.boundaries["scope"]
            .as_str()
            .unwrap()
            .contains("synthetic")
    );
    let mut observed = 0;
    for row in &fixture.cases {
        if row.operation == "source_load"
            && reverse_core_support::string(&row.input, "context") != "nil"
            && !matches!(
                reverse_core_support::string(&row.input, "state"),
                "final-userinfo" | "final-unsupported" | "request-nil"
            )
        {
            reverse_core_support::source::load(row).await;
            observed += 1;
        }
    }
    assert_eq!(observed, 29);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn redirects_preserve_all_five_policy_body_and_hook_contracts() {
    let fixture = reverse_core_support::fixture();
    let mut observed = 0;
    for row in &fixture.cases {
        if row.operation == "source_redirect" {
            reverse_core_support::source::redirect(row).await;
            observed += 1;
        }
    }
    assert_eq!(observed, 5);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn aggregate_runtime_matches_forty_two_non_null_context_contracts() {
    let fixture = reverse_core_support::fixture();
    let mut observed = 0;
    for row in &fixture.cases {
        if row.operation == "facade_aggregate"
            && reverse_core_support::string(&row.input, "context") != "nil"
        {
            reverse_core_support::aggregate::run(row).await;
            observed += 1;
        }
    }
    assert_eq!(observed, 42);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn facade_runtime_matches_thirteen_non_null_owner_contracts() {
    let fixture = reverse_core_support::fixture();
    let mut observed = 0;
    for row in &fixture.cases {
        if row.operation == "facade_lifecycle"
            && reverse_core_support::string(&row.input, "context") != "nil"
            && !reverse_core_support::boolean(&row.input, "nil_facade")
        {
            reverse_core_support::facade::run(row).await;
            observed += 1;
        }
    }
    assert_eq!(observed, 13);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn snapshot_runtime_preserves_both_linux_lifecycle_contracts() {
    let fixture = reverse_core_support::fixture();
    let mut observed = 0;
    for row in &fixture.cases {
        if row.operation == "snapshot_lifecycle" {
            reverse_core_support::source::lifecycle(row).await;
            observed += 1;
        }
    }
    assert_eq!(observed, 2);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn cancelled_and_concurrent_close_waiters_share_one_reverse_order_owner() {
    reverse_core_support::aggregate::suspended_close_is_owned_once().await;
}

#[test]
fn domain_json_retains_invalid_embedded_identity_kinds_and_exact_i64_ids() {
    use pixiv_app::reverse_search::{Evidence, PixivRef, PixivRefType, Provider};
    let identity = PixivRef {
        kind: PixivRefType::Other("invalid-owned-kind".to_owned()),
        id: i64::MAX,
    };
    let value = serde_json::to_value(&identity).unwrap();
    assert_eq!(value["type"], "invalid-owned-kind");
    assert_eq!(value["id"].as_i64(), Some(i64::MAX));
    assert_eq!(serde_json::from_value::<PixivRef>(value).unwrap(), identity);
    let mut evidence = Evidence {
        provider: Provider::SauceNao,
        rank: 1,
        similarity: 90.0,
        index_id: 0,
        index_name: String::new(),
        title: String::new(),
        author: String::new(),
        external_urls: Vec::new(),
    };
    assert_eq!(
        serde_json::to_value(&evidence).unwrap()["similarity"].as_i64(),
        Some(90)
    );
    evidence.similarity = -0.0;
    assert!(
        serde_json::to_value(&evidence).unwrap()["similarity"]
            .as_f64()
            .unwrap()
            .is_sign_negative()
    );
    evidence.similarity = f64::INFINITY;
    assert!(serde_json::to_value(&evidence).is_err());
}

#[test]
fn joined_errors_keep_order_identity_and_both_context_memberships() {
    use pixiv_app::reverse_search::{Error, ErrorCode};
    use pixiv_sdk::context::ContextError;
    let first = Error::new(
        ErrorCode::SourceReadFailed,
        "owned safe source failure",
        Some(Error::wrap(
            "owned private cancellation path",
            ContextError::Canceled.into(),
        )),
    );
    let second = Error::new(
        ErrorCode::SnapshotFailed,
        "owned safe snapshot failure",
        Some(ContextError::DeadlineExceeded.into()),
    );
    let wrapped = Error::external(first.clone());
    assert!(wrapped.contains(&first));
    let joined = Error::join([wrapped, second.clone()]).unwrap();
    assert!(joined.contains(&first));
    assert!(joined.contains(&second));
    assert!(joined.contains_context(ContextError::Canceled));
    assert!(joined.contains_context(ContextError::DeadlineExceeded));
    assert_eq!(joined.context_error(), Some(ContextError::Canceled));
    assert_eq!(joined.code(), ErrorCode::SourceReadFailed);
    assert_eq!(
        joined.to_string(),
        "owned safe source failure\nowned safe snapshot failure"
    );
    assert_eq!(joined.joined().unwrap().len(), 2);
}

#[test]
fn embedded_provider_summary_preserves_signed_go_counts() {
    use pixiv_app::reverse_search::{Provider, ProviderStatus, ProviderSummary};
    let summary = ProviderSummary {
        name: Provider::SauceNao,
        status: ProviderStatus::Success,
        result_count: -1,
        quota: None,
    };
    let value = serde_json::to_value(&summary).unwrap();
    assert_eq!(value["result_count"].as_i64(), Some(-1));
    assert_eq!(
        serde_json::from_value::<ProviderSummary>(value).unwrap(),
        summary
    );
}

#[tokio::test]
async fn local_source_copy_yields_for_cancellation_and_removes_its_partial_snapshot() {
    use pixiv_sdk::context::ContextError;
    use std::{
        future::Future,
        task::{Context as TaskContext, Poll},
    };

    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("owned-large-source.bin");
    let snapshots = root.path().join("snapshots");
    std::fs::create_dir(&snapshots).unwrap();
    let original = vec![0xa5; 1024 * 1024];
    std::fs::write(&source, &original).unwrap();
    let loader = Loader::new(SourceLoaderOptions {
        temp_dir: snapshots.clone(),
        ..SourceLoaderOptions::default()
    });
    let context = Arc::new(Context::new());
    let mut load = loader.load(context.clone(), source.to_str().unwrap());
    let waker = futures_util::task::noop_waker();
    let mut task = TaskContext::from_waker(&waker);

    assert!(
        matches!(Future::poll(load.as_mut(), &mut task), Poll::Pending),
        "local source copying must let the same-task MCP reader receive cancellation notifications"
    );
    assert_eq!(std::fs::read_dir(&snapshots).unwrap().count(), 1);
    context.cancel();
    let error = load.await.unwrap_err();
    assert!(error.contains_context(ContextError::Canceled));
    assert_eq!(error.to_string(), "context canceled");
    assert_eq!(std::fs::read_dir(&snapshots).unwrap().count(), 0);
    assert_eq!(std::fs::read(&source).unwrap(), original);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn buffered_url_source_copy_yields_for_cancellation_and_closes_its_body() {
    reverse_core_support::source::buffered_copy_yields_and_cancels().await;
}
