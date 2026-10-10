use pixiv_sdk::fanbox::{
    Client, FlareSolverrOptions, Options,
    transport::{ExternalError, RawRequest, RawResponse, RawTransport, TransportFuture},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    error::Error as _,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

#[derive(Default)]
struct ConstructorTransport {
    requests: AtomicUsize,
    idle_closes: AtomicUsize,
}
impl RawTransport for ConstructorTransport {
    fn send(
        &self,
        _request: RawRequest,
    ) -> TransportFuture<'_, std::result::Result<Option<RawResponse>, ExternalError>> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { panic!("raw credential constructors must not issue requests") })
    }
    fn close_idle_connections(&self) {
        self.idle_closes.fetch_add(1, Ordering::SeqCst);
    }
}
fn decode_hex(input: &str) -> Vec<u8> {
    assert_eq!(input.len() % 2, 0);
    assert!(
        input
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    input
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

#[test]
fn raw_session_byte_constructor_matches_all_40_frozen_go_rows() {
    let encoded = include_bytes!("fixtures/fanbox-opaque-credentials.json");
    assert_eq!(
        format!("{:x}", Sha256::digest(encoded)),
        "120ac3eb7d0fd51118e5fe08ee63fad7622ed9c56b441926096b74c03565dac5"
    );
    let fixture: Value = serde_json::from_slice(encoded).unwrap();
    assert_eq!(
        fixture["source_commit"],
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    assert_eq!(fixture["go_version"], "go1.27.1");
    assert_eq!(fixture["frozen_go_production_guard"]["path_count"], 434);
    assert_eq!(
        fixture["frozen_go_production_guard"]["byte_equal_to_frozen_commit"],
        true
    );
    let rows = fixture["cases"].as_array().unwrap();
    assert_eq!(rows.len(), 40);
    let mut names = BTreeSet::new();
    for row in rows {
        let name = row["name"].as_str().unwrap();
        assert!(names.insert(name));
        let raw = decode_hex(row["session_hex"].as_str().unwrap());
        let transport = Arc::new(ConstructorTransport::default());
        let options = Options {
            http_client: Some(transport.clone()),
            proxy_url: row["proxy_url"].as_str().unwrap().into(),
            user_agent: row["user_agent"].as_str().unwrap().into(),
            flare_solverr: (!row["solver_url"].as_str().unwrap().is_empty()
                || !row["solver_proxy_url"].as_str().unwrap().is_empty())
            .then(|| FlareSolverrOptions {
                url: row["solver_url"].as_str().unwrap().into(),
                proxy_url: row["solver_proxy_url"].as_str().unwrap().into(),
            }),
        };
        let result = Client::open_session_bytes_with(&raw, options);
        match result {
            Ok(client) => {
                assert!(
                    row["error"].is_null(),
                    "{name}: expected constructor failure"
                );
                for _ in 0..row["close_calls"].as_u64().unwrap() {
                    client.close_idle_connections();
                }
            }
            Err(error) => {
                assert!(!row["error"].is_null(), "{name}: unexpected {error}");
                assert_eq!(error.code.as_str(), row["error"]["reason"], "{name}");
                assert_eq!(error.to_string(), row["error"]["message"], "{name}");
                assert_eq!(
                    error.source().map(ToString::to_string).unwrap_or_default(),
                    row["error"]["cause"],
                    "{name}"
                );
            }
        }
        assert_eq!(row["requests"], json!([]), "{name}");
        assert_eq!(transport.requests.load(Ordering::SeqCst), 0, "{name}");
        assert_eq!(
            transport.idle_closes.load(Ordering::SeqCst) as u64,
            row["idle_closes"].as_u64().unwrap(),
            "{name}"
        );
        assert_eq!(row["go_only_injected_client_unchanged"], true, "{name}");
        assert_eq!(row["go_only_default_client_requests"], 0, "{name}");
        assert_eq!(row["go_only_default_client_idle_closes"], 0, "{name}");
    }
    for required in [
        "credential/invalid-ff-plus-lf",
        "credential/duplicate-precedes-later-invalid-byte",
        "credential/earlier-malformed-precedes-duplicate",
        "options/invalid-utf8/agent-before-proxy-before-solver",
        "ownership/default-client-explicitly-injected",
    ] {
        assert!(
            names.contains(required),
            "missing concrete precedence boundary {required}"
        );
    }
}
