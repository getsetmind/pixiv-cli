use pixiv_sdk::{
    Client, Result,
    pixiv::AddArtworkBookmarkRequest,
    transport::{Request, Response, Transport},
};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

#[derive(Clone, Default)]
struct CallerTransport {
    closes: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<Request>>>,
}

impl Transport for CallerTransport {
    async fn send(&self, request: Request) -> Result<Response> {
        self.requests.lock().unwrap().push(request);
        Ok(Response {
            status: 200,
            retry_after: None,
            body: json!({}),
        })
    }

    fn close_idle_connections(&self) {
        self.closes.fetch_add(1, Ordering::SeqCst);
    }
}

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/client-ownership.json")).unwrap()
}

#[test]
fn validated_constructors_reject_blank_tokens_before_native_transport_configuration() {
    for row in fixture()["constructors"].as_array().unwrap() {
        let Some(expected) = row["error"].as_object() else {
            continue;
        };
        let input = row["input"].as_str().unwrap();
        let error = Client::new_owned(input, Some("://invalid-proxy")).unwrap_err();
        assert_eq!(error.code.as_str(), expected["reason"].as_str().unwrap());
        assert_eq!(error.operation, expected["operation"].as_str().unwrap());
        assert_eq!(error.detail.as_deref(), expected["detail"].as_str());
        let transport = CallerTransport::default();
        let error = Client::try_with_transport(input, transport.clone()).unwrap_err();
        assert_eq!(error.operation, "New");
        assert_eq!(transport.closes.load(Ordering::SeqCst), 0);
        assert!(transport.requests.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn injected_clients_share_the_caller_port_and_never_close_it_even_when_paced() {
    let expected = fixture();
    for row in expected["constructors"].as_array().unwrap() {
        if row["mode"] != "custom" || !row["error"].is_null() {
            continue;
        }
        let transport = CallerTransport::default();
        let client = Client::try_with_transport(row["input"].as_str().unwrap(), transport.clone())
            .unwrap()
            .with_pacing(Duration::from_millis(
                row["pacing_ms"].as_i64().unwrap().max(0) as u64,
            ));
        let second = Client::try_with_transport("fixture-access", transport.clone()).unwrap();
        client.close_idle_connections();
        client.close_idle_connections();
        second.close_idle_connections();
        assert_eq!(
            transport.closes.load(Ordering::SeqCst) as u64,
            row["injected_close_calls"].as_u64().unwrap(),
            "{}",
            row["name"]
        );
        client
            .add_artwork_bookmark(AddArtworkBookmarkRequest {
                artwork_id: 73,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(transport.requests.lock().unwrap().len(), 1);
        assert_eq!(client.user_id(), row["identity"].as_i64().unwrap());
    }
}

#[tokio::test]
async fn explicit_ownership_transfer_forwards_every_close_without_terminating_requests() {
    let transport = CallerTransport::default();
    let client = Client::with_owned_transport(" fixture-access ", transport.clone()).unwrap();
    client.close_idle_connections();
    client.close_idle_connections();
    assert_eq!(transport.closes.load(Ordering::SeqCst), 2);
    client
        .add_artwork_bookmark(AddArtworkBookmarkRequest {
            artwork_id: 73,
            ..Default::default()
        })
        .await
        .unwrap();
    let requests = transport.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("authorization") && value == "Bearer fixture-access"
    }));
}

#[test]
fn legacy_anonymous_resource_constructor_remains_available() {
    let client = Client::new("", None).unwrap();
    client.close_idle_connections();
    assert_eq!(client.user_id(), 0);
}

#[tokio::test]
async fn validated_saved_credentials_keep_the_verified_identity_and_caller_ownership() {
    struct RefreshTransport(CallerTransport);
    impl Transport for RefreshTransport {
        async fn send(&self, request: Request) -> Result<Response> {
            let oauth = request.operation == "Open";
            let mut response = self.0.send(request).await?;
            if oauth {
                response.body = json!({
                    "access_token": "fixture-access",
                    "refresh_token": "fixture-rotated",
                    "expires_in": 3600,
                    "user": {"id": 73, "name": "owned fixture identity"}
                });
            }
            Ok(response)
        }

        fn close_idle_connections(&self) {
            self.0.close_idle_connections();
        }
    }
    let transport = CallerTransport::default();
    let credentials =
        pixiv_sdk::oauth::refresh(&RefreshTransport(transport.clone()), "fixture-refresh")
            .await
            .unwrap();
    let client = Client::try_from_credentials(&credentials, transport.clone()).unwrap();
    assert_eq!(client.user_id(), 73);
    assert_eq!(client.username(), "owned fixture identity");
    client.close_idle_connections();
    client.close_idle_connections();
    client
        .add_artwork_bookmark(AddArtworkBookmarkRequest {
            artwork_id: 73,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(transport.closes.load(Ordering::SeqCst), 0);
    let requests = transport.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1]
            .headers
            .iter()
            .any(|(name, value)| { name.eq_ignore_ascii_case("x-user-id") && value == "73" })
    );
}
