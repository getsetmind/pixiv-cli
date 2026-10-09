use pixiv_app::{
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
struct Repository {
    database: Arc<Mutex<Database>>,
    calls: Calls,
    failure: &'static str,
    gets: Mutex<usize>,
}
impl AccountRepository for Repository {
    fn get(&self, context: &Context, id: i64) -> std::result::Result<PixivAccount, SchedulerError> {
        self.calls.lock().unwrap().push("get");
        let mut gets = self.gets.lock().unwrap();
        *gets += 1;
        if self.failure == format!("get{}", *gets) {
            return Err(SchedulerError::Message("get failed".into()));
        }
        self.database
            .get(context, if self.failure == "record" { 7 } else { id })
    }
    fn list(&self, context: &Context) -> std::result::Result<Vec<PixivAccount>, SchedulerError> {
        self.calls.lock().unwrap().push("list");
        self.database.list(context)
    }
    fn rotate(
        &self,
        context: &Context,
        id: i64,
        revision: i64,
        token: &[u8],
    ) -> std::result::Result<(), SchedulerError> {
        self.calls.lock().unwrap().push("rotate");
        if self.failure == "rotate" {
            return Err(SchedulerError::Account(
                pixiv_app::database::AccountError::CredentialConflict,
            ));
        }
        self.database.rotate(context, id, revision, token)
    }
    fn update_metadata(
        &self,
        context: &Context,
        id: i64,
        name: &str,
        premium: Option<bool>,
        checked: Option<i64>,
    ) -> std::result::Result<(), SchedulerError> {
        self.calls.lock().unwrap().push("metadata");
        if self.failure == "metadata" {
            return Err(SchedulerError::Message("metadata failed".into()));
        }
        self.database
            .update_metadata(context, id, name, premium, checked)
    }
}
struct Defaults {
    selected: Option<i64>,
    failure: bool,
}
impl AccountDefaultStore for Defaults {
    fn read(&self) -> std::result::Result<Option<i64>, SchedulerError> {
        if self.failure {
            return Err(SchedulerError::Message("default failed".into()));
        }
        Ok(self.selected)
    }
    fn set(&self, _: i64) -> std::result::Result<(), SchedulerError> {
        panic!("validation does not set default")
    }
    fn clear(&self) -> std::result::Result<(), SchedulerError> {
        panic!("validation does not clear default")
    }
}
struct Connection {
    database: Arc<Mutex<Database>>,
    calls: Calls,
    failure: &'static str,
    context: Context,
    dropped: Arc<Mutex<bool>>,
}
impl Drop for Connection {
    fn drop(&mut self) {
        *self.dropped.lock().unwrap() = true;
    }
}
impl Transport for Connection {
    async fn send(&self, request: Request) -> Result<Response> {
        let oauth = request.operation == "Open";
        self.calls
            .lock()
            .unwrap()
            .push(if oauth { "oauth" } else { "profile" });
        if self.failure
            == if oauth {
                "oauth_cancel"
            } else {
                "profile_cancel"
            }
        {
            self.context.cancel();
            return std::future::pending().await;
        }
        let body = if oauth {
            assert_eq!(request.method.as_str(), "POST");
            assert_eq!(request.url, "https://oauth.secure.pixiv.net/auth/token");
            assert!(
                request
                    .parameters
                    .iter()
                    .any(|(k, v)| k == "refresh_token" && v == "synthetic-input")
            );
            json!({"access_token":"synthetic-access","refresh_token":"synthetic-rotated","expires_in":3600,"user":{"id":if self.failure=="identity"{43}else if self.failure=="record"{7}else{42},"name":"oauth-name"}})
        } else {
            assert_eq!(request.operation, "CurrentUser");
            assert_eq!(request.method.as_str(), "GET");
            assert_eq!(request.url, "https://app-api.pixiv.net/v1/user/detail");
            assert!(
                request
                    .parameters
                    .iter()
                    .any(|(k, v)| k == "filter" && v == "for_android")
            );
            assert!(
                request
                    .parameters
                    .iter()
                    .any(|(k, v)| k == "user_id" && v == "42")
            );
            assert!(
                request
                    .headers
                    .iter()
                    .any(|(k, v)| k.eq_ignore_ascii_case("authorization")
                        && v == "Bearer synthetic-access")
            );
            assert_eq!(
                self.database
                    .lock()
                    .unwrap()
                    .get_pixiv(42)
                    .unwrap()
                    .credential_revision,
                2
            );
            if self.failure == "profile" {
                json!({})
            } else {
                json!({"user":{"id":42,"name":"profile-name"},"profile":{"is_premium":true},"profile_publicity":{},"workspace":{}})
            }
        };
        Ok(Response {
            status: 200,
            retry_after: None,
            body,
        })
    }
}

#[tokio::test]
async fn validation_matches_go_rotation_profile_metadata_and_summary_order() {
    for (refresh, failure, expected, committed, metadata) in [
        (false, "", vec!["get", "oauth", "rotate"], true, false),
        (false, "identity", vec!["get", "oauth"], false, false),
        (
            false,
            "rotate",
            vec!["get", "oauth", "rotate"],
            false,
            false,
        ),
        (false, "get1", vec!["get"], false, false),
        (
            true,
            "",
            vec![
                "get", "oauth", "rotate", "profile", "get", "metadata", "get",
            ],
            true,
            true,
        ),
        (true, "identity", vec!["get", "oauth"], false, false),
        (true, "rotate", vec!["get", "oauth", "rotate"], false, false),
        (
            true,
            "profile",
            vec!["get", "oauth", "rotate", "profile"],
            true,
            false,
        ),
        (
            true,
            "get2",
            vec!["get", "oauth", "rotate", "profile", "get"],
            true,
            false,
        ),
        (
            true,
            "metadata",
            vec!["get", "oauth", "rotate", "profile", "get", "metadata"],
            true,
            false,
        ),
        (
            true,
            "get3",
            vec![
                "get", "oauth", "rotate", "profile", "get", "metadata", "get",
            ],
            true,
            true,
        ),
        (
            true,
            "default",
            vec![
                "get", "oauth", "rotate", "profile", "get", "metadata", "get",
            ],
            true,
            true,
        ),
        (true, "get1", vec!["get"], false, false),
        (false, "oauth_cancel", vec!["get", "oauth"], false, false),
        (true, "oauth_cancel", vec!["get", "oauth"], false, false),
        (
            true,
            "profile_cancel",
            vec!["get", "oauth", "rotate", "profile"],
            true,
            false,
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let mut database = Database::open(dir.path()).unwrap();
        database
            .save_pixiv_credential(&PixivAccount::new(42, "stored", b"synthetic-input"))
            .unwrap();
        let database = Arc::new(Mutex::new(database));
        let calls = Calls::default();
        let context = Context::new();
        let service = AccountService {
            repository: Arc::new(Repository {
                database: database.clone(),
                calls: calls.clone(),
                failure,
                gets: Mutex::new(0),
            }),
            defaults: None,
        };
        let defaults = Defaults {
            selected: Some(42),
            failure: failure == "default",
        };
        let dropped = Arc::new(Mutex::new(false));
        let transport = Connection {
            database: database.clone(),
            calls: calls.clone(),
            failure,
            context: context.clone(),
            dropped: dropped.clone(),
        };
        let before = chrono::Utc::now().timestamp();
        let result = if refresh {
            service
                .validation(&defaults)
                .refresh_account_with(&context, 42, transport)
                .await
        } else {
            service.check_account_with(&context, 42, transport).await
        };
        assert_eq!(
            result.is_err(),
            !failure.is_empty(),
            "{refresh} {failure}: {result:?}"
        );
        if let Err(error) = &result {
            let expected_error = match failure {
                "identity" => {
                    "persist rotated pixiv credentials: pixiv:OpenAccountClient: local_state_error: credential identity does not match selected account"
                }
                "rotate" => {
                    "persist rotated pixiv credentials: pixiv account credential revision conflict"
                }
                "profile" => "pixiv:CurrentUser: malformed_upstream_response",
                "get1" if refresh => "select pixiv account: get failed",
                "get1" | "get2" | "get3" => "get failed",
                "metadata" => "metadata failed",
                "default" => "default failed",
                "oauth_cancel" => {
                    "pixiv:Open: upstream_unavailable: pixiv upstream transport failed"
                }
                "profile_cancel" => {
                    "pixiv:CurrentUser: upstream_unavailable: pixiv upstream transport failed"
                }
                other => panic!("unexpected failure {other}"),
            };
            assert_eq!(error.to_string(), expected_error);
        }
        assert_eq!(*calls.lock().unwrap(), expected, "{refresh} {failure}");
        assert!(
            *dropped.lock().unwrap(),
            "transport must be dropped on every exit"
        );
        let stored = database.lock().unwrap().get_pixiv(42).unwrap();
        assert_eq!(stored.credential_revision, if committed { 2 } else { 1 });
        assert_eq!(
            stored.refresh_token_copy(),
            if committed {
                b"synthetic-rotated".to_vec()
            } else {
                b"synthetic-input".to_vec()
            }
        );
        assert_eq!(stored.username, "stored");
        assert_eq!(
            stored.premium_status,
            if metadata { Some(true) } else { None }
        );
        if metadata {
            assert!(
                (before..=chrono::Utc::now().timestamp())
                    .contains(&stored.premium_checked_at.unwrap())
            );
        } else {
            assert!(stored.premium_checked_at.is_none());
        }
        match result {
            Ok(summary) if refresh => {
                assert_eq!(summary.username, "stored");
                assert!(summary.default);
                assert_eq!(summary.premium, Some(true));
                assert!(summary.pool_status_known);
                assert!(summary.eligible);
            }
            Ok(summary) => {
                assert_eq!(summary.user_id, 42);
                assert_eq!(summary.username, "oauth-name");
                assert!(!summary.default);
                assert!(summary.premium.is_none());
                assert!(!summary.pool_status_known);
                assert!(!summary.schedulable);
                assert!(!summary.eligible);
            }
            Err(error) if failure.ends_with("cancel") => assert!(error.is_canceled(), "{error}"),
            Err(error) if failure == "get1" => assert_eq!(
                error.to_string(),
                if refresh {
                    "select pixiv account: get failed"
                } else {
                    "get failed"
                }
            ),
            Err(_) => {}
        }
    }
}

struct FreshDefaults(Mutex<Option<i64>>);
impl AccountDefaultStore for FreshDefaults {
    fn read(&self) -> std::result::Result<Option<i64>, SchedulerError> {
        Ok(*self.0.lock().unwrap())
    }
    fn set(&self, _: i64) -> std::result::Result<(), SchedulerError> {
        panic!("validation does not change default")
    }
    fn clear(&self) -> std::result::Result<(), SchedulerError> {
        panic!("validation does not change default")
    }
}
struct FreshProfile {
    database: Arc<Mutex<Database>>,
    defaults: Arc<FreshDefaults>,
    premium: bool,
}
impl Transport for FreshProfile {
    async fn send(&self, request: Request) -> Result<Response> {
        let body = if request.operation == "Open" {
            json!({"access_token":"synthetic-access","refresh_token":"synthetic-rotated","expires_in":3600,"user":{"id":42,"name":"oauth-name"}})
        } else {
            *self.defaults.0.lock().unwrap() = Some(7);
            self.database
                .lock()
                .unwrap()
                .update_pixiv_metadata(42, "fresh-stored", None, None)
                .unwrap();
            json!({"user":{"id":42,"name":"profile-name"},"profile":{"is_premium":self.premium},"profile_publicity":{},"workspace":{}})
        };
        Ok(Response {
            status: 200,
            retry_after: None,
            body,
        })
    }
}
#[tokio::test]
async fn refresh_preserves_scheduling_and_reads_final_metadata_and_default() {
    for premium in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let database = Arc::new(Mutex::new(Database::open(dir.path()).unwrap()));
        for id in [42, 7] {
            database
                .lock()
                .unwrap()
                .save_pixiv_credential(&PixivAccount::new(id, "stored", b"synthetic-input"))
                .unwrap();
        }
        let sql = rusqlite::Connection::open(database.lock().unwrap().path()).unwrap();
        sql.execute("UPDATE pixiv_account SET schedulable=0,pool_frozen_until=1,pool_last_selected=1,premium_status=1,premium_checked_at=123 WHERE user_id=42",[]).unwrap();
        let defaults = Arc::new(FreshDefaults(Mutex::new(None)));
        let service = AccountService {
            repository: database.clone(),
            defaults: None,
        };
        let summary = service
            .validation(defaults.as_ref())
            .refresh_account_with(
                &Context::new(),
                42,
                FreshProfile {
                    database: database.clone(),
                    defaults: defaults.clone(),
                    premium,
                },
            )
            .await
            .unwrap();
        assert!(!summary.default);
        assert_eq!(summary.username, "fresh-stored");
        assert_eq!(summary.premium, Some(premium));
        assert!(!summary.schedulable);
        assert!(!summary.eligible);
        assert!(summary.pool_frozen_until.is_none());
        assert!(summary.pool_last_selected);
        assert!(summary.pool_status_known);
        let stored = database.lock().unwrap().get_pixiv(42).unwrap();
        assert_eq!(stored.credential_revision, 2);
        assert!(!stored.schedulable);
        assert_eq!(stored.pool_frozen_until, Some(1));
        assert!(stored.pool_last_selected);
        assert_eq!(stored.username, "fresh-stored");
        assert_eq!(stored.premium_status, Some(premium));
    }
}

#[tokio::test]
async fn check_uses_returned_identity_while_open_preserves_requested_identity_boundary() {
    for check in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let database = Arc::new(Mutex::new(Database::open(dir.path()).unwrap()));
        for id in [42, 7] {
            database
                .lock()
                .unwrap()
                .save_pixiv_credential(&PixivAccount::new(id, "stored", b"synthetic-input"))
                .unwrap();
        }
        let calls = Calls::default();
        let context = Context::new();
        let service = AccountService {
            repository: Arc::new(Repository {
                database: database.clone(),
                calls: calls.clone(),
                failure: "record",
                gets: Mutex::new(0),
            }),
            defaults: None,
        };
        let transport = Connection {
            database: database.clone(),
            calls,
            failure: "record",
            context: context.clone(),
            dropped: Arc::new(Mutex::new(false)),
        };
        if check {
            let summary = service
                .check_account_with(&context, 42, transport)
                .await
                .unwrap();
            assert_eq!(summary.user_id, 7);
        } else {
            let result = service.open_account(&context, 42, transport).await;
            assert_eq!(
                result.err().unwrap().to_string(),
                "persist rotated pixiv credentials: pixiv:OpenAccountClient: local_state_error: credential identity does not match selected account"
            );
        }
        assert_eq!(
            database
                .lock()
                .unwrap()
                .get_pixiv(42)
                .unwrap()
                .credential_revision,
            1
        );
        assert_eq!(
            database
                .lock()
                .unwrap()
                .get_pixiv(7)
                .unwrap()
                .credential_revision,
            if check { 2 } else { 1 }
        );
    }
}
