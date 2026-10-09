use pixiv_app::{
    account_login::{LoginCompleteRequest, LoginService, LoginStart},
    account_management::AccountDefaultStore,
    account_service::{AccountRepository, AccountService},
    database::{Database, PixivAccount},
    lifecycle::Context,
    scheduler::SchedulerError,
};
use pixiv_sdk::{
    Result,
    transport::{Request, Response, Transport},
};
use serde_json::json;
use std::sync::{Arc, Mutex};

type Calls = Arc<Mutex<Vec<&'static str>>>;
struct Defaults {
    calls: Calls,
    selected: Mutex<Option<i64>>,
    failure: &'static str,
    reads: Mutex<usize>,
}
impl AccountDefaultStore for Defaults {
    fn read(&self) -> std::result::Result<Option<i64>, SchedulerError> {
        self.calls.lock().unwrap().push("read");
        let mut reads = self.reads.lock().unwrap();
        *reads += 1;
        if self.failure == "read2" && *reads == 2 {
            return Err(SchedulerError::Message("summary default failed".into()));
        }
        if self.failure == "read" {
            return Err(SchedulerError::Message("read failed".into()));
        }
        Ok(*self.selected.lock().unwrap())
    }
    fn set(&self, id: i64) -> std::result::Result<(), SchedulerError> {
        self.calls.lock().unwrap().push("set");
        if self.failure == "set" {
            return Err(SchedulerError::Message("set failed".into()));
        }
        *self.selected.lock().unwrap() = Some(id);
        Ok(())
    }
    fn clear(&self) -> std::result::Result<(), SchedulerError> {
        *self.selected.lock().unwrap() = None;
        Ok(())
    }
}
struct Repository {
    database: Arc<Mutex<Database>>,
    calls: Calls,
    failure: &'static str,
}
impl AccountRepository for Repository {
    fn get(&self, context: &Context, id: i64) -> std::result::Result<PixivAccount, SchedulerError> {
        self.calls.lock().unwrap().push("get");
        if self.failure == "get" {
            return Err(SchedulerError::Message("get failed".into()));
        }
        self.database.get(context, id)
    }
    fn list(&self, context: &Context) -> std::result::Result<Vec<PixivAccount>, SchedulerError> {
        self.calls.lock().unwrap().push("list");
        self.database.list(context)
    }
    fn save_credential(
        &self,
        context: &Context,
        account: &PixivAccount,
    ) -> std::result::Result<(), SchedulerError> {
        self.calls.lock().unwrap().push("save");
        if self.failure == "save" {
            return Err(SchedulerError::Message("save failed".into()));
        }
        let result = self.database.save_credential(context, account);
        if self.failure == "cancel" {
            context.cancel();
        }
        result
    }
    fn save_credentials(
        &self,
        context: &Context,
        accounts: &[PixivAccount],
    ) -> std::result::Result<(), SchedulerError> {
        self.calls.lock().unwrap().push("batch");
        self.database.save_credentials(context, accounts)
    }
    fn rotate(
        &self,
        _: &Context,
        _: i64,
        _: i64,
        _: &[u8],
    ) -> std::result::Result<(), SchedulerError> {
        panic!("transfer does not perform CAS rotation")
    }
}
struct Connection {
    calls: Calls,
    cancel: Option<Context>,
}
impl Transport for Connection {
    async fn send(&self, request: Request) -> Result<Response> {
        self.calls.lock().unwrap().push("oauth");
        assert_eq!(request.operation, "Complete");
        assert_eq!(request.method.as_str(), "POST");
        assert_eq!(request.url, "https://oauth.secure.pixiv.net/auth/token");
        assert!(
            request
                .parameters
                .iter()
                .any(|(key, value)| key == "grant_type" && value == "authorization_code")
        );
        assert!(
            request
                .parameters
                .iter()
                .any(|(key, value)| key == "code" && value == "synthetic-code")
        );
        if let Some(context) = &self.cancel {
            context.cancel();
            return std::future::pending().await;
        }
        Ok(Response {
            status: 200,
            retry_after: None,
            body: json!({"access_token":"synthetic-access","refresh_token":" synthetic-refresh ","user":{"id":"42","name":"fresh"}}),
        })
    }
}
fn request() -> LoginCompleteRequest {
    LoginCompleteRequest {
        callback_or_code: "synthetic-code".into(),
        use_after_login: false,
    }
}
#[tokio::test]
async fn completion_matches_go_save_default_summary_order_and_partial_commits() {
    for (failure, selected, use_after_login, expected, want) in [
        (
            "",
            None,
            false,
            vec!["oauth", "save", "read", "set", "get", "read"],
            "",
        ),
        (
            "",
            Some(7),
            false,
            vec!["oauth", "save", "read", "get", "read"],
            "",
        ),
        (
            "",
            Some(7),
            true,
            vec!["oauth", "save", "read", "set", "get", "read"],
            "",
        ),
        (
            "save",
            None,
            false,
            vec!["oauth", "save"],
            "save pixiv account: save failed",
        ),
        (
            "read",
            None,
            true,
            vec!["oauth", "save", "read"],
            "read pixiv default account: read failed",
        ),
        (
            "set",
            None,
            false,
            vec!["oauth", "save", "read", "set"],
            "set pixiv default account: set failed",
        ),
        (
            "read2",
            None,
            false,
            vec!["oauth", "save", "read", "set", "get", "read"],
            "summary default failed",
        ),
        (
            "cancel",
            None,
            false,
            vec!["oauth", "save", "read", "set", "get"],
            "context canceled",
        ),
        (
            "get",
            None,
            false,
            vec!["oauth", "save", "read", "set", "get"],
            "get failed",
        ),
        (
            "nil",
            None,
            false,
            vec!["oauth", "save"],
            "read pixiv default account: pixiv default account store is not configured",
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let database = Arc::new(Mutex::new(Database::open(temp.path()).unwrap()));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let service = AccountService {
            repository: Arc::new(Repository {
                database: database.clone(),
                calls: calls.clone(),
                failure,
            }),
            defaults: None,
        };
        let defaults = Defaults {
            calls: calls.clone(),
            selected: Mutex::new(selected),
            failure,
            reads: Mutex::new(0),
        };
        let login = LoginService {
            pixiv: Some(&service),
            defaults: if failure == "nil" {
                None
            } else {
                Some(&defaults)
            },
        };
        let start = login.start().unwrap();
        let connection = Connection {
            calls: calls.clone(),
            cancel: None,
        };
        let result = login
            .complete(
                &Context::new(),
                &start,
                &LoginCompleteRequest {
                    use_after_login,
                    ..request()
                },
                &connection,
            )
            .await;
        if want.is_empty() {
            let summary = result.unwrap();
            assert_eq!(summary.username, "fresh");
            assert_eq!(summary.default, selected.is_none() || use_after_login);
            assert!(summary.schedulable && summary.eligible && summary.pool_status_known);
            assert!(!format!("{summary:?}").contains("synthetic-refresh"));
        } else {
            assert_eq!(result.unwrap_err().to_string(), want);
        }
        assert_eq!(*calls.lock().unwrap(), expected);
        if failure == "read2" || failure == "get" || failure == "cancel" {
            assert_eq!(*defaults.selected.lock().unwrap(), Some(42));
        }
        if failure != "save" {
            let account = database.lock().unwrap().get_pixiv(42).unwrap();
            assert_eq!(account.refresh_token_copy(), b" synthetic-refresh ");
            assert_eq!(account.credential_revision, 1);
        }
    }
}
#[tokio::test]
async fn missing_service_is_checked_after_oauth_and_shared_session_is_consumed() {
    let login = LoginService::default();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let connection = Connection {
        calls: calls.clone(),
        cancel: None,
    };
    let context = Context::new();
    assert_eq!(
        login
            .complete(&context, &LoginStart::default(), &request(), &connection)
            .await
            .unwrap_err()
            .to_string(),
        "login session is not initialized"
    );
    assert!(!LoginStart::default().accepts_callback_url("pixiv://account/login?code=x"));
    let start = login.start().unwrap();
    let copied = start.clone();
    let invalid = LoginCompleteRequest {
        callback_or_code: "https://example.com/?code=synthetic-code".into(),
        use_after_login: false,
    };
    let error = login
        .complete(&context, &start, &invalid, &connection)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("login callback is invalid"));
    assert!(calls.lock().unwrap().is_empty());
    let state = url::Url::parse(&start.authorization_url)
        .unwrap()
        .query_pairs()
        .find(|(key, _)| key == "state")
        .unwrap()
        .1
        .into_owned();
    assert!(!start.accepts_callback_url("pixiv://account/login?code=synthetic-code&state=foreign"));
    assert!(start.accepts_callback_url(&format!(
        "pixiv://account/login?code=synthetic-code&state={state}"
    )));
    assert_eq!(
        login
            .complete(&context, &start, &request(), &connection)
            .await
            .unwrap_err()
            .to_string(),
        "pixiv account service is not configured"
    );
    let error = login
        .complete(&context, &copied, &request(), &connection)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("login session was already used"));
    assert_eq!(*calls.lock().unwrap(), ["oauth"]);
    assert!(!format!("{start:?}").contains("synthetic-refresh"));
}
#[tokio::test]
async fn cancellation_consumes_session_without_persisting_and_precedes_missing_service() {
    for before in [false, true] {
        let context = Context::new();
        if before {
            context.cancel();
        }
        let login = LoginService::default();
        let start = login.start().unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let connection = Connection {
            calls: calls.clone(),
            cancel: Some(context.clone()),
        };
        let error = login
            .complete(&context, &start, &request(), &connection)
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "pixiv:Complete: upstream_unavailable: pixiv upstream transport failed"
        );
        assert!(pixiv_sdk::error::is_canceled(&error));
        assert!(!error.to_string().contains("service is not configured"));
        let error = login
            .complete(&Context::new(), &start, &request(), &connection)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("already used"));
        assert_eq!(calls.lock().unwrap().len(), usize::from(!before));
    }
}

#[tokio::test]
async fn incomplete_credentials_precede_storage_and_canceled_save_leaves_database_empty() {
    let temp = tempfile::tempdir().unwrap();
    let database = Arc::new(Mutex::new(Database::open(temp.path()).unwrap()));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let service = AccountService {
        repository: Arc::new(Repository {
            database: database.clone(),
            calls: calls.clone(),
            failure: "",
        }),
        defaults: None,
    };
    let session = pixiv_sdk::oauth::LoginSession::begin().unwrap();
    let connection = Connection {
        calls: calls.clone(),
        cancel: None,
    };
    let mut credentials = session
        .complete(&connection, "synthetic-code")
        .await
        .unwrap();
    calls.lock().unwrap().clear();
    credentials.user_id = 0;
    assert_eq!(
        service
            .complete_login(&Context::new(), &credentials, false, None)
            .unwrap_err()
            .to_string(),
        "login credentials are incomplete"
    );
    assert!(calls.lock().unwrap().is_empty());
    credentials.user_id = 42;
    let context = Context::new();
    context.cancel();
    assert_eq!(
        service
            .complete_login(&context, &credentials, false, None)
            .unwrap_err()
            .to_string(),
        "save pixiv account: context canceled"
    );
    assert_eq!(*calls.lock().unwrap(), ["save"]);
    assert!(database.lock().unwrap().list_pixiv().unwrap().is_empty());
}
