use pixiv_app::{
    account_management::AccountDefaultStore,
    account_service::{AccountRepository, AccountService, AccountSummary},
    account_transfer::{AccountWithToken, RestoreAccountInput},
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
}
impl AccountDefaultStore for Defaults {
    fn read(&self) -> std::result::Result<Option<i64>, SchedulerError> {
        self.calls.lock().unwrap().push("read");
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
        self.database.save_credential(context, account)
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
    dropped: Arc<Mutex<bool>>,
    failure: bool,
    cancel: Option<Context>,
}
impl Drop for Connection {
    fn drop(&mut self) {
        *self.dropped.lock().unwrap() = true;
    }
}
impl Transport for Connection {
    async fn send(&self, request: Request) -> Result<Response> {
        self.calls.lock().unwrap().push("oauth");
        assert_eq!(request.method.as_str(), "POST");
        assert_eq!(request.url, "https://oauth.secure.pixiv.net/auth/token");
        assert_eq!(request.operation, "Open");
        assert!(
            request
                .parameters
                .iter()
                .any(|(k, v)| k == "refresh_token" && v == "synthetic-input")
        );
        assert!(
            request
                .parameters
                .iter()
                .any(|(k, v)| k == "grant_type" && v == "refresh_token")
        );
        if let Some(context) = &self.cancel {
            context.cancel();
            return std::future::pending().await;
        }
        Ok(Response {
            status: if self.failure { 401 } else { 200 },
            retry_after: None,
            body: json!({"access_token":"synthetic-access","refresh_token":"synthetic-rotated","user":{"id":"42","name":"fresh"}}),
        })
    }
}
fn summary(id: i64) -> AccountSummary {
    AccountSummary {
        user_id: id,
        username: format!("name-{id}"),
        default: false,
        premium: None,
        schedulable: false,
        pool_frozen_until: None,
        pool_last_selected: false,
        eligible: false,
        pool_status_known: false,
    }
}
fn input(id: i64, default: bool) -> RestoreAccountInput {
    RestoreAccountInput {
        account: summary(id),
        refresh_token: format!(" synthetic-{id} "),
        is_bundle_default: default,
    }
}

#[tokio::test]
async fn import_matches_go_oauth_save_default_and_fresh_summary_failure_order() {
    for (failure, expected, want) in [
        ("", vec!["oauth", "save", "read", "set", "get", "read"], ""),
        (
            "save",
            vec!["oauth", "save"],
            "save pixiv account: save failed",
        ),
        (
            "read",
            vec!["oauth", "save", "read"],
            "read pixiv default account: read failed",
        ),
        (
            "set",
            vec!["oauth", "save", "read", "set"],
            "set pixiv default account: set failed",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let database = Arc::new(Mutex::new(Database::open(dir.path()).unwrap()));
        let calls = Calls::default();
        let dropped = Arc::new(Mutex::new(false));
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
            selected: Mutex::new(None),
            failure,
        };
        let result = service
            .transfer(&defaults)
            .import_account_with(
                &Context::new(),
                "  synthetic-input  ",
                false,
                Connection {
                    calls: calls.clone(),
                    dropped: dropped.clone(),
                    failure: false,
                    cancel: None,
                },
            )
            .await;
        if want.is_empty() {
            let out = result.unwrap();
            assert_eq!(out.user_id, 42);
            assert_eq!(out.username, "fresh");
            assert!(out.default);
            assert!(out.schedulable);
            assert!(out.eligible);
            assert!(out.pool_status_known)
        } else {
            assert_eq!(result.unwrap_err().to_string(), want)
        }
        assert_eq!(*calls.lock().unwrap(), expected, "{failure}");
        assert!(*dropped.lock().unwrap());
        if failure != "save" {
            assert_eq!(
                database
                    .lock()
                    .unwrap()
                    .get_pixiv(42)
                    .unwrap()
                    .refresh_token_copy(),
                b"synthetic-rotated"
            )
        }
    }
}

#[tokio::test]
async fn rejected_or_cancelled_oauth_never_saves_and_owned_transport_is_released() {
    for cancel in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let database = Arc::new(Mutex::new(Database::open(dir.path()).unwrap()));
        let calls = Calls::default();
        let dropped = Arc::new(Mutex::new(false));
        let context = Context::new();
        let service = AccountService {
            repository: database.clone(),
            defaults: None,
        };
        let defaults = Defaults {
            calls: calls.clone(),
            selected: Mutex::new(None),
            failure: "",
        };
        let result = service
            .transfer(&defaults)
            .import_account_with(
                &context,
                "synthetic-input",
                false,
                Connection {
                    calls: calls.clone(),
                    dropped: dropped.clone(),
                    failure: !cancel,
                    cancel: cancel.then(|| context.clone()),
                },
            )
            .await;
        assert!(result.is_err());
        assert!(database.lock().unwrap().list_pixiv().unwrap().is_empty());
        assert_eq!(*calls.lock().unwrap(), vec!["oauth"]);
        assert!(*dropped.lock().unwrap());
    }
}

#[test]
fn restore_matches_go_atomic_database_merge_order_metadata_and_default_commit_boundaries() {
    for mode in [
        "implicit",
        "explicit",
        "duplicate",
        "trigger",
        "set-failure",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let database = Arc::new(Mutex::new(Database::open(dir.path()).unwrap()));
        database
            .lock()
            .unwrap()
            .save_pixiv_credential(&PixivAccount::new(7, "old", b"synthetic-old"))
            .unwrap();
        let sql = rusqlite::Connection::open(database.lock().unwrap().path()).unwrap();
        sql.execute("UPDATE pixiv_account SET credential_revision=4,schedulable=0,premium_status=1,premium_checked_at=123,pool_frozen_until=4102444800,pool_last_selected=1 WHERE user_id=7",[]).unwrap();
        if mode == "trigger" {
            sql.execute_batch("CREATE TRIGGER fail_restore BEFORE INSERT ON pixiv_account WHEN NEW.user_id=42 BEGIN SELECT RAISE(ABORT,'synthetic failure'); END").unwrap()
        }
        let service = AccountService {
            repository: database.clone(),
            defaults: None,
        };
        let defaults = Defaults {
            calls: Calls::default(),
            selected: Mutex::new((mode == "explicit").then_some(999)),
            failure: if mode == "set-failure" { "set" } else { "" },
        };
        let mut inputs = vec![input(7, false), input(42, true), input(9, true)];
        if mode == "duplicate" {
            inputs[2].account.user_id = 7
        }
        let result = service
            .transfer(&defaults)
            .restore_accounts(&Context::new(), &inputs);
        let rows = database.lock().unwrap().list_pixiv().unwrap();
        if ["duplicate", "trigger"].contains(&mode) {
            assert!(result.is_err());
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].refresh_token_copy(), b"synthetic-old");
            assert_eq!(*defaults.selected.lock().unwrap(), None);
            continue;
        }
        assert_eq!(
            rows.iter().map(|a| a.user_id).collect::<Vec<_>>(),
            vec![7, 42, 9]
        );
        assert_eq!(rows[0].credential_revision, 5);
        assert_eq!(rows[0].refresh_token_copy(), b"synthetic-7");
        assert!(!rows[0].schedulable);
        assert_eq!(rows[0].premium_status, Some(true));
        assert_eq!(rows[0].premium_checked_at, Some(123));
        assert_eq!(rows[0].pool_frozen_until, Some(4102444800));
        assert!(rows[0].pool_last_selected);
        assert!(rows[1].schedulable);
        if mode == "set-failure" {
            assert_eq!(
                result.unwrap_err().to_string(),
                "set pixiv default account: set failed"
            );
            assert_eq!(*defaults.selected.lock().unwrap(), None);
            continue;
        }
        let out = result.unwrap();
        assert_eq!(
            out.resulting_default,
            if mode == "explicit" { 999 } else { 42 }
        );
        assert!(out.accounts[0].is_replacement);
        assert!(!out.accounts[1].is_replacement);
        assert_eq!(
            out.accounts
                .iter()
                .map(|a| a.account.user_id)
                .collect::<Vec<_>>(),
            vec![7, 42, 9]
        );
        assert!(!out.accounts[1].account.default);
    }
}

#[test]
fn export_matches_go_order_and_repeated_fresh_default_reads_without_pool_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let database = Arc::new(Mutex::new(Database::open(dir.path()).unwrap()));
    for id in [42, 7] {
        database
            .lock()
            .unwrap()
            .save_pixiv_credential(&PixivAccount::new(
                id,
                "synthetic",
                format!("synthetic-{id}").as_bytes(),
            ))
            .unwrap()
    }
    let sql = rusqlite::Connection::open(database.lock().unwrap().path()).unwrap();
    sql.execute("UPDATE pixiv_account SET pool_frozen_until=1", [])
        .unwrap();
    let calls = Calls::default();
    let service = AccountService {
        repository: Arc::new(Repository {
            database: database.clone(),
            calls: calls.clone(),
            failure: "",
        }),
        defaults: None,
    };
    let defaults = Defaults {
        calls: calls.clone(),
        selected: Mutex::new(None),
        failure: "",
    };
    let out = service
        .transfer(&defaults)
        .accounts_with_tokens(&Context::new())
        .unwrap();
    assert_eq!(
        *calls.lock().unwrap(),
        vec!["list", "read", "list", "read", "list"]
    );
    assert_eq!(
        out.iter().map(|a| a.user_id).collect::<Vec<_>>(),
        vec![42, 7]
    );
    assert!(out[0].default);
    assert!(!out[1].default);
    assert_eq!(out[1].refresh_token(), "synthetic-7");
    assert_eq!(
        database
            .lock()
            .unwrap()
            .get_pixiv(7)
            .unwrap()
            .pool_frozen_until,
        Some(1)
    );
    assert!(!format!("{:?}", out[1]).contains("synthetic-7"));
    assert_eq!(
        serde_json::to_value(&out[1]).unwrap(),
        json!({"user_id":7,"username":"synthetic","default":false})
    );
}

#[test]
fn transfer_validation_precedes_any_read_or_write_and_tokens_are_opaque() {
    let dir = tempfile::tempdir().unwrap();
    let database = Arc::new(Mutex::new(Database::open(dir.path()).unwrap()));
    let calls = Calls::default();
    let service = AccountService {
        repository: Arc::new(Repository {
            database,
            calls: calls.clone(),
            failure: "",
        }),
        defaults: None,
    };
    let defaults = Defaults {
        calls: calls.clone(),
        selected: Mutex::new(None),
        failure: "",
    };
    let transfer = service.transfer(&defaults);
    assert_eq!(
        transfer
            .restore_accounts(&Context::new(), &[])
            .unwrap_err()
            .to_string(),
        "pixiv restore bundle has no accounts"
    );
    assert_eq!(
        transfer
            .restore_accounts(&Context::new(), &[input(0, false)])
            .unwrap_err()
            .to_string(),
        "pixiv refresh token is required"
    );
    let mut blank = input(42, false);
    blank.refresh_token = "  ".into();
    assert_eq!(
        transfer
            .restore_accounts(&Context::new(), &[blank])
            .unwrap_err()
            .to_string(),
        "pixiv refresh token is required"
    );
    assert!(calls.lock().unwrap().is_empty());
    assert!(!format!("{:?}", input(42, false)).contains("synthetic-42"));
    assert_eq!(
        AccountWithToken::new(42, "name", true, "synthetic-secret").refresh_token(),
        "synthetic-secret"
    );
}

struct ChangingDefaults {
    calls: Calls,
    values: Vec<Option<i64>>,
    reads: Mutex<usize>,
}
impl AccountDefaultStore for ChangingDefaults {
    fn read(&self) -> std::result::Result<Option<i64>, SchedulerError> {
        self.calls.lock().unwrap().push("read");
        let mut reads = self.reads.lock().unwrap();
        let value = self.values[*reads];
        *reads += 1;
        Ok(value)
    }
    fn set(&self, _: i64) -> std::result::Result<(), SchedulerError> {
        self.calls.lock().unwrap().push("set");
        Ok(())
    }
    fn clear(&self) -> std::result::Result<(), SchedulerError> {
        Ok(())
    }
}
#[test]
fn export_and_current_user_observe_freshly_changing_default() {
    let dir = tempfile::tempdir().unwrap();
    let database = Arc::new(Mutex::new(Database::open(dir.path()).unwrap()));
    for id in [42, 7] {
        database
            .lock()
            .unwrap()
            .save_pixiv_credential(&PixivAccount::new(id, "synthetic", b"synthetic-token"))
            .unwrap()
    }
    let calls = Calls::default();
    let service = AccountService {
        repository: Arc::new(Repository {
            database,
            calls: calls.clone(),
            failure: "",
        }),
        defaults: None,
    };
    let defaults = ChangingDefaults {
        calls: calls.clone(),
        values: vec![Some(42), Some(7), Some(7)],
        reads: Mutex::new(0),
    };
    let transfer = service.transfer(&defaults);
    let out = transfer.accounts_with_tokens(&Context::new()).unwrap();
    assert!(out[0].default);
    assert!(out[1].default);
    assert_eq!(transfer.current_user(&Context::new()).unwrap().user_id, 7);
    assert_eq!(
        *calls.lock().unwrap(),
        vec!["list", "read", "read", "read", "get", "get"]
    );
}
#[tokio::test]
async fn import_preserves_existing_default_and_metadata_and_normalizes_expired_freeze_only_in_summary()
 {
    let dir = tempfile::tempdir().unwrap();
    let database = Arc::new(Mutex::new(Database::open(dir.path()).unwrap()));
    database
        .lock()
        .unwrap()
        .save_pixiv_credential(&PixivAccount::new(42, "old", b"synthetic-old"))
        .unwrap();
    let sql = rusqlite::Connection::open(database.lock().unwrap().path()).unwrap();
    sql.execute("UPDATE pixiv_account SET pool_frozen_until=1,premium_status=1,premium_checked_at=123,schedulable=0 WHERE user_id=42",[]).unwrap();
    let calls = Calls::default();
    let service = AccountService {
        repository: database.clone(),
        defaults: None,
    };
    let defaults = Defaults {
        calls: calls.clone(),
        selected: Mutex::new(Some(7)),
        failure: "",
    };
    let dropped = Arc::new(Mutex::new(false));
    let out = service
        .transfer(&defaults)
        .import_account_with(
            &Context::new(),
            "synthetic-input",
            false,
            Connection {
                calls: calls.clone(),
                dropped: dropped.clone(),
                failure: false,
                cancel: None,
            },
        )
        .await
        .unwrap();
    assert!(!out.default);
    assert_eq!(out.pool_frozen_until, None);
    assert_eq!(out.premium, Some(true));
    assert!(!out.eligible);
    let stored = database.lock().unwrap().get_pixiv(42).unwrap();
    assert_eq!(stored.pool_frozen_until, Some(1));
    assert_eq!(stored.premium_checked_at, Some(123));
    assert_eq!(*defaults.selected.lock().unwrap(), Some(7));
    assert_eq!(*calls.lock().unwrap(), vec!["oauth", "read", "read"]);
    assert!(*dropped.lock().unwrap());
}

#[test]
fn raw_export_retains_non_utf8_credential_bytes_while_bundle_text_matches_go_json() {
    let dir = tempfile::tempdir().unwrap();
    let database = Arc::new(Mutex::new(Database::open(dir.path()).unwrap()));
    database
        .lock()
        .unwrap()
        .save_pixiv_credential(&PixivAccount::new(42, "synthetic", b"synthetic-\xff-token"))
        .unwrap();
    let service = AccountService {
        repository: database,
        defaults: None,
    };
    let defaults = Defaults {
        calls: Calls::default(),
        selected: Mutex::new(Some(42)),
        failure: "",
    };
    let rows = service
        .transfer(&defaults)
        .accounts_with_tokens(&Context::new())
        .unwrap();
    assert_eq!(rows[0].refresh_token_bytes(), b"synthetic-\xff-token");
    assert_eq!(rows[0].refresh_token(), "synthetic-\u{fffd}-token");
    assert!(!format!("{:?}", rows[0]).contains("token"));
    assert_eq!(
        serde_json::to_value(&rows[0]).unwrap(),
        json!({"user_id":42,"username":"synthetic","default":true})
    );
}

#[test]
fn bundle_token_text_replaces_each_invalid_byte_like_go_json_encoder() {
    let account =
        AccountWithToken::from_token_bytes(42, "synthetic", true, b"synthetic-\xe2\x82-token");
    assert_eq!(account.refresh_token_bytes(), b"synthetic-\xe2\x82-token");
    assert_eq!(account.refresh_token(), "synthetic-\u{fffd}\u{fffd}-token");
}
