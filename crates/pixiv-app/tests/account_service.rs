use pixiv_app::{
    account_service::AccountService,
    database::{Database, PixivAccount},
    gate::Gate,
    lifecycle::Context,
    sessions::{ClientOpen, ClientSessions},
};
use pixiv_sdk::{
    Result,
    transport::{Request, Response, Transport},
};
use serde::Deserialize;
use serde_json::json;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Deserialize)]
struct Case {
    name: String,
    message: String,
    oauth_calls: usize,
    content_calls: usize,
    token: String,
    revision: i64,
    username: String,
    canceled: bool,
}

#[derive(Clone)]
struct Connection {
    name: String,
    database: Arc<Mutex<Database>>,
    oauth_calls: Arc<AtomicUsize>,
    content_calls: Arc<AtomicUsize>,
    context: Context,
}
impl Transport for Connection {
    async fn send(&self, request: Request) -> Result<Response> {
        let body = if request.operation == "Open" {
            self.oauth_calls.fetch_add(1, Ordering::SeqCst);
            if self.name == "refresh_canceled" {
                self.context.cancel();
                return std::future::pending().await;
            }
            if self.name == "revision_conflict" {
                self.database
                    .lock()
                    .unwrap()
                    .rotate_pixiv_credentials(42, 1, b"fixture-concurrent")
                    .unwrap();
            }
            json!({"access_token":"fixture-access","refresh_token":if self.name=="no_rotated_token" { "" } else { "fixture-rotated" },"expires_in":if self.name=="zero_expiry" {0} else {3600},"user":{"id":if self.name=="identity_mismatch" {43} else {42},"name":"oauth-name"}})
        } else {
            self.content_calls.fetch_add(1, Ordering::SeqCst);
            let stored = self.database.lock().unwrap().get_pixiv(42).unwrap();
            assert_eq!(stored.credential_revision, 2);
            assert!(
                request
                    .headers
                    .iter()
                    .any(|(name, value)| name.eq_ignore_ascii_case("authorization")
                        && value == "Bearer fixture-access")
            );
            json!({})
        };
        Ok(Response {
            status: 200,
            retry_after: None,
            body,
        })
    }
}

#[tokio::test]
async fn account_session_refreshes_checks_identity_and_persists_before_content() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/account-open.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 11);
    for case in cases {
        let directory = tempfile::tempdir().unwrap();
        let mut database = Database::open(directory.path()).unwrap();
        if case.name != "empty" {
            database
                .save_pixiv_credential(&PixivAccount::new(42, "stored-name", b"fixture-refresh"))
                .unwrap();
        }
        let database = Arc::new(Mutex::new(database));
        let default = match case.name.as_str() {
            "configured" => Some(42),
            "configured_missing" => Some(99),
            _ => None,
        };
        let service = Arc::new(AccountService {
            repository: database.clone(),
            defaults: Some(Arc::new(move || Ok(default))),
        });
        let context = Context::new();
        let connection = Connection {
            name: case.name.clone(),
            database: database.clone(),
            oauth_calls: Arc::new(AtomicUsize::new(0)),
            content_calls: Arc::new(AtomicUsize::new(0)),
            context: context.clone(),
        };
        let sent = connection.clone();
        let gate = Gate::new();
        let sessions = ClientSessions {
            accounts: Some(Arc::new(move |context, id, transport| {
                let service = service.clone();
                Box::pin(async move {
                    match service.open(&context, id, transport).await {
                        Ok(client) => ClientOpen {
                            client: Some(Arc::new(client)),
                            error: None,
                        },
                        Err(error) => ClientOpen {
                            client: None,
                            error: Some(error),
                        },
                    }
                })
            })),
            gate: Some(gate.clone()),
            close_client: Arc::new(|_| Ok(())),
        };
        let id = match case.name.as_str() {
            "fallback" | "configured" | "empty" | "configured_missing" => 0,
            "missing" => 99,
            _ => 42,
        };
        let result = sessions.open(Some(&context), id, connection).await;
        let message = result
            .as_ref()
            .err()
            .map(ToString::to_string)
            .unwrap_or_default();
        assert_eq!(message, case.message, "{}", case.name);
        assert_eq!(
            result
                .as_ref()
                .err()
                .is_some_and(|error| error.is_canceled()),
            case.canceled,
            "{}",
            case.name
        );
        if let Ok(lease) = result {
            let _ = lease.value().artwork(1).await;
            lease.close().unwrap();
        }
        let reusable = Context::new();
        let permit =
            tokio::time::timeout(std::time::Duration::from_secs(5), gate.acquire(&reusable))
                .await
                .unwrap()
                .unwrap();
        drop(permit);
        assert_eq!(
            sent.oauth_calls.load(Ordering::SeqCst),
            case.oauth_calls,
            "{}",
            case.name
        );
        assert_eq!(
            sent.content_calls.load(Ordering::SeqCst),
            case.content_calls,
            "{}",
            case.name
        );
        if let Ok(stored) = database.lock().unwrap().get_pixiv(42) {
            assert_eq!(
                stored.refresh_token_copy(),
                case.token.as_bytes(),
                "{}",
                case.name
            );
            assert_eq!(stored.credential_revision, case.revision, "{}", case.name);
            assert_eq!(stored.username, case.username, "{}", case.name);
        } else {
            assert_eq!(case.revision, 0);
        }
    }
}
