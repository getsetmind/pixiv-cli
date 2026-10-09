use pixiv_app::{
    account_service::{AccountRepository, AccountService, AccountSummary},
    database::{Database, PixivAccount, PoolStatus},
    lifecycle::Context,
    scheduler::SchedulerError,
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

fn snapshot(accounts: Vec<AccountSummary>) -> Value {
    Value::Array(accounts.into_iter().map(|account| json!({
        "UserID":account.user_id,"Username":account.username,"Default":account.default,
        "Premium":account.premium,"Schedulable":account.schedulable,
        "PoolFrozenUntil":account.pool_frozen_until,"PoolLastSelected":account.pool_last_selected,
        "Eligible":account.eligible,"PoolStatusKnown":account.pool_status_known
    })).collect())
}

#[test]
fn local_account_views_match_go_default_metadata_membership_and_expiry() {
    let cases: Vec<Value> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/auth-account-views.json"
    ))
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let mut db = Database::open(directory.path()).unwrap();
    for id in [42, 7] {
        db.save_pixiv_credential(&PixivAccount::new(id, "synthetic", b"synthetic-token"))
            .unwrap();
    }
    let connection = rusqlite::Connection::open(db.path()).unwrap();
    connection.execute("UPDATE pixiv_account SET premium_status=1,premium_checked_at=123,pool_frozen_until=4102444800,pool_last_selected=1 WHERE user_id=7",[]).unwrap();
    let default = Arc::new(Mutex::new(None));
    let reader = default.clone();
    let service = AccountService {
        repository: Arc::new(Mutex::new(db)),
        defaults: Some(Arc::new(move || Ok(*reader.lock().unwrap()))),
    };
    let context = Context::new();
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let result = match name {
            "explicit_default" => {
                *default.lock().unwrap() = Some(7);
                None
            }
            "missing_explicit_default" => {
                *default.lock().unwrap() = Some(999);
                None
            }
            "empty" => Some(service.set_pool_schedulable(&context, &[], false)),
            "zero" => Some(service.set_pool_schedulable(&context, &[0], false)),
            "duplicate" => Some(service.set_pool_schedulable(&context, &[42, 42], false)),
            "unknown" => Some(service.set_pool_schedulable(&context, &[42, 999], false)),
            "disabled_frozen" => {
                service.set_pool_schedulable(&context, &[7], false).unwrap();
                None
            }
            "all_disabled" => {
                service.set_all_pool_schedulable(&context, false).unwrap();
                None
            }
            "all_enabled" => {
                service.set_all_pool_schedulable(&context, true).unwrap();
                None
            }
            "expired_freeze" => {
                connection.execute("UPDATE pixiv_account SET pool_frozen_until=1,updated_at=22 WHERE user_id=7",[]).unwrap();
                None
            }
            "canceled" => {
                context.cancel();
                Some(service.set_all_pool_schedulable(&context, false))
            }
            "implicit_default" => None,
            name => panic!("unknown case {name}"),
        };
        if let Some(result) = result {
            assert_eq!(
                result.unwrap_err().to_string(),
                case["error"].as_str().unwrap(),
                "{name}"
            );
        } else {
            assert_eq!(
                snapshot(service.list_accounts(&context).unwrap()),
                case["accounts"],
                "{name}"
            );
        }
    }
    assert_eq!(
        service.list_accounts(&context).unwrap_err().to_string(),
        "context canceled"
    );
    assert_eq!(
        service
            .set_pool_schedulable(&context, &[0], false)
            .unwrap_err()
            .to_string(),
        "database: account pool user id must be positive"
    );
    let updated: i64 = connection
        .query_row(
            "SELECT updated_at FROM pixiv_account WHERE user_id=7",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(updated, 22);
}

struct ViewRepository {
    calls: Mutex<Vec<&'static str>>,
    pool_error: bool,
    list_error: bool,
    empty: bool,
    two: bool,
}
impl AccountRepository for ViewRepository {
    fn get(&self, _: &Context, _: i64) -> Result<PixivAccount, SchedulerError> {
        panic!("summary must not inspect credentials")
    }
    fn rotate(&self, _: &Context, _: i64, _: i64, _: &[u8]) -> Result<(), SchedulerError> {
        panic!("summary must not rotate")
    }
    fn list(&self, _: &Context) -> Result<Vec<PixivAccount>, SchedulerError> {
        self.calls.lock().unwrap().push("list");
        if self.list_error {
            return Err(SchedulerError::Message("list failed".into()));
        }
        Ok(if self.empty {
            vec![]
        } else if self.two {
            vec![
                PixivAccount::new(42, "synthetic", b""),
                PixivAccount::new(7, "second", b""),
            ]
        } else {
            vec![PixivAccount::new(42, "synthetic", b"")]
        })
    }
    fn pool_status(&self, _: &Context, _: i64) -> Result<PoolStatus, SchedulerError> {
        self.calls.lock().unwrap().push("pool");
        if self.pool_error {
            return Err(SchedulerError::Message("pool failed".into()));
        }
        Ok(PoolStatus {
            accounts: vec![],
            earliest_frozen_until: None,
        })
    }
}
#[test]
fn views_read_pool_first_and_resolve_defaults_per_row_only() {
    for (pool_error, list_error, empty, expected) in [
        (true, true, false, "pool failed"),
        (false, true, false, "list failed"),
        (
            false,
            false,
            false,
            "pixiv default account store is not configured",
        ),
        (false, false, true, ""),
    ] {
        let repository = Arc::new(ViewRepository {
            calls: Mutex::new(vec![]),
            pool_error,
            list_error,
            empty,
            two: false,
        });
        let service = AccountService {
            repository: repository.clone(),
            defaults: None,
        };
        let result = service.list_accounts(&Context::new());
        if expected.is_empty() {
            assert!(result.unwrap().is_empty());
        } else {
            assert_eq!(result.unwrap_err().to_string(), expected);
        }
        assert_eq!(
            *repository.calls.lock().unwrap(),
            if pool_error {
                vec!["pool"]
            } else {
                vec!["pool", "list"]
            }
        );
    }
    let repository = Arc::new(ViewRepository {
        calls: Mutex::new(vec![]),
        pool_error: false,
        list_error: false,
        empty: false,
        two: false,
    });
    let service = AccountService {
        repository: repository.clone(),
        defaults: Some(Arc::new(|| Ok(None))),
    };
    let rows = service.list_accounts(&Context::new()).unwrap();
    assert!(rows[0].default && rows[0].pool_status_known);
    assert!(!rows[0].schedulable && !rows[0].eligible);
    assert_eq!(
        *repository.calls.lock().unwrap(),
        vec!["pool", "list", "list"]
    );
}

#[test]
fn membership_failure_rolls_back_selected_and_all_without_touching_metadata() {
    let directory = tempfile::tempdir().unwrap();
    let mut database = Database::open(directory.path()).unwrap();
    for id in [42, 7] {
        database
            .save_pixiv_credential(&PixivAccount::new(id, "synthetic", b"synthetic-token"))
            .unwrap();
    }
    let connection = rusqlite::Connection::open(database.path()).unwrap();
    connection.execute("UPDATE pixiv_account SET premium_status=1,premium_checked_at=123,pool_frozen_until=4102444800,pool_last_selected=1,created_at=11,updated_at=22 WHERE user_id=7",[]).unwrap();
    connection.execute_batch("CREATE TRIGGER reject_membership BEFORE UPDATE OF schedulable ON pixiv_account WHEN NEW.user_id=7 BEGIN SELECT RAISE(ABORT,'synthetic rejection'); END;").unwrap();
    let database = Arc::new(Mutex::new(database));
    let service = AccountService {
        repository: database.clone(),
        defaults: Some(Arc::new(|| Ok(None))),
    };
    for all in [false, true] {
        let result = if all {
            service.set_all_pool_schedulable(&Context::new(), false)
        } else {
            service.set_pool_schedulable(&Context::new(), &[42, 7], false)
        };
        assert!(result.is_err());
        let rows = database.lock().unwrap().list_pixiv().unwrap();
        for row in &rows {
            assert!(row.schedulable);
            assert_eq!(row.credential_revision, 1);
            assert_eq!(row.refresh_token_copy(), b"synthetic-token");
        }
        let second = &rows[1];
        assert_eq!(
            (
                second.premium_status,
                second.premium_checked_at,
                second.pool_frozen_until,
                second.pool_last_selected,
                second.created_at,
                second.updated_at
            ),
            (Some(true), Some(123), Some(4102444800), true, 11, 22)
        );
    }
}

#[test]
fn defaults_are_read_per_summary_and_implicit_fallback_requeries_accounts() {
    for explicit in [false, true] {
        let repository = Arc::new(ViewRepository {
            calls: Mutex::new(vec![]),
            pool_error: false,
            list_error: false,
            empty: false,
            two: true,
        });
        let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let reader_reads = reads.clone();
        let service = AccountService {
            repository: repository.clone(),
            defaults: Some(Arc::new(move || {
                reader_reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(if explicit { Some(7) } else { None })
            })),
        };
        let rows = service.list_accounts(&Context::new()).unwrap();
        assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(
            *repository.calls.lock().unwrap(),
            if explicit {
                vec!["pool", "list"]
            } else {
                vec!["pool", "list", "list", "list"]
            }
        );
        assert_eq!((rows[0].default, rows[1].default), (!explicit, explicit));
    }
}
