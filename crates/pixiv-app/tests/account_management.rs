use pixiv_app::{
    account_management::AccountDefaultStore,
    account_service::{AccountRepository, AccountService},
    config::Store,
    database::{Database, PixivAccount},
    lifecycle::Context,
    scheduler::SchedulerError,
};
use std::sync::{Arc, Mutex};

struct Repository {
    calls: Arc<Mutex<Vec<&'static str>>>,
    failure: &'static str,
    remains: Mutex<bool>,
}
impl AccountRepository for Repository {
    fn get(&self, _: &Context, id: i64) -> Result<PixivAccount, SchedulerError> {
        self.calls.lock().unwrap().push("get");
        if self.failure == "get" {
            return Err(SchedulerError::Message("get failed".into()));
        }
        Ok(PixivAccount::new(id, "synthetic", b"synthetic-token"))
    }
    fn remove(&self, _: &Context, _: i64) -> Result<(), SchedulerError> {
        self.calls.lock().unwrap().push("remove");
        if self.failure == "remove" {
            return Err(SchedulerError::Message("remove failed".into()));
        }
        *self.remains.lock().unwrap() = false;
        Ok(())
    }
    fn list(&self, _: &Context) -> Result<Vec<PixivAccount>, SchedulerError> {
        panic!("management does not list")
    }
    fn rotate(&self, _: &Context, _: i64, _: i64, _: &[u8]) -> Result<(), SchedulerError> {
        panic!("management does not authenticate")
    }
}
struct Defaults {
    calls: Arc<Mutex<Vec<&'static str>>>,
    failure: &'static str,
    selected: Mutex<Option<i64>>,
}
impl AccountDefaultStore for Defaults {
    fn read(&self) -> Result<Option<i64>, SchedulerError> {
        self.calls.lock().unwrap().push("read");
        if self.failure == "read" {
            return Err(SchedulerError::Message("read failed".into()));
        }
        Ok(*self.selected.lock().unwrap())
    }
    fn set(&self, id: i64) -> Result<(), SchedulerError> {
        self.calls.lock().unwrap().push("set");
        if self.failure == "set" {
            return Err(SchedulerError::Message("set failed".into()));
        }
        *self.selected.lock().unwrap() = Some(id);
        Ok(())
    }
    fn clear(&self) -> Result<(), SchedulerError> {
        self.calls.lock().unwrap().push("clear");
        if self.failure == "clear" {
            return Err(SchedulerError::Message("clear failed".into()));
        }
        *self.selected.lock().unwrap() = None;
        Ok(())
    }
}

#[test]
fn management_preserves_frozen_go_call_order_errors_and_failed_delete_state() {
    struct Case<'a> {
        name: &'a str,
        use_account: bool,
        initial: Option<i64>,
        failure: &'static str,
        calls: &'a [&'a str],
        error: &'a str,
        selected: Option<i64>,
        remains: bool,
    }
    let cases = [
        Case {
            name: "use",
            use_account: true,
            initial: Some(7),
            failure: "",
            calls: &["get", "set"],
            error: "",
            selected: Some(42),
            remains: true,
        },
        Case {
            name: "use-get-failure",
            use_account: true,
            initial: Some(7),
            failure: "get",
            calls: &["get"],
            error: "get failed",
            selected: Some(7),
            remains: true,
        },
        Case {
            name: "use-set-failure",
            use_account: true,
            initial: Some(7),
            failure: "set",
            calls: &["get", "set"],
            error: "set failed",
            selected: Some(7),
            remains: true,
        },
        Case {
            name: "remove-explicit",
            use_account: false,
            initial: Some(42),
            failure: "",
            calls: &["read", "clear", "remove"],
            error: "",
            selected: None,
            remains: false,
        },
        Case {
            name: "remove-implicit",
            use_account: false,
            initial: None,
            failure: "",
            calls: &["read", "remove"],
            error: "",
            selected: None,
            remains: false,
        },
        Case {
            name: "remove-other",
            use_account: false,
            initial: Some(7),
            failure: "",
            calls: &["read", "remove"],
            error: "",
            selected: Some(7),
            remains: false,
        },
        Case {
            name: "remove-read-failure",
            use_account: false,
            initial: Some(42),
            failure: "read",
            calls: &["read"],
            error: "read pixiv default account: read failed",
            selected: Some(42),
            remains: true,
        },
        Case {
            name: "remove-clear-failure",
            use_account: false,
            initial: Some(42),
            failure: "clear",
            calls: &["read", "clear"],
            error: "clear pixiv default account: clear failed",
            selected: Some(42),
            remains: true,
        },
        Case {
            name: "remove-delete-failure-no-rollback",
            use_account: false,
            initial: Some(42),
            failure: "remove",
            calls: &["read", "clear", "remove"],
            error: "remove failed",
            selected: None,
            remains: true,
        },
    ];
    for Case {
        name,
        use_account,
        initial,
        failure,
        calls,
        error,
        selected,
        remains,
    } in cases
    {
        let observed = Arc::new(Mutex::new(vec![]));
        let repository = Arc::new(Repository {
            calls: observed.clone(),
            failure,
            remains: Mutex::new(true),
        });
        let defaults = Defaults {
            calls: observed.clone(),
            failure,
            selected: Mutex::new(initial),
        };
        let service = AccountService {
            repository: repository.clone(),
            defaults: None,
        };
        let management = service.management(&defaults);
        let result = if use_account {
            management.use_account(&Context::new(), 42)
        } else {
            management.remove_account(&Context::new(), 42)
        };
        assert_eq!(
            result
                .err()
                .map(|error| error.to_string())
                .unwrap_or_default(),
            error,
            "{name}"
        );
        assert_eq!(*observed.lock().unwrap(), calls, "{name}");
        assert_eq!(*defaults.selected.lock().unwrap(), selected, "{name}");
        assert_eq!(*repository.remains.lock().unwrap(), remains, "{name}");
    }
}

#[test]
fn saved_account_management_uses_real_config_database_and_retains_clear_on_delete_failure() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path().join("config.toml"));
    let mut database = Database::open(directory.path()).unwrap();
    for id in [42, 7] {
        database
            .save_pixiv_credential(&PixivAccount::new(id, "synthetic", b"synthetic-token"))
            .unwrap();
    }
    let sql = rusqlite::Connection::open(database.path()).unwrap();
    let database = Arc::new(Mutex::new(database));
    let reader = store.clone();
    let service = AccountService {
        repository: database.clone(),
        defaults: Some(Arc::new(move || {
            reader.read_pixiv_default_user_id().map_err(Into::into)
        })),
    };
    let management = service.management(&store);
    management.use_account(&Context::new(), 42).unwrap();
    assert_eq!(
        management
            .use_account(&Context::new(), 999)
            .unwrap_err()
            .to_string(),
        "pixiv account not found"
    );
    assert_eq!(store.read_pixiv_default_user_id().unwrap(), Some(42));
    assert_eq!(
        management
            .remove_account(&Context::new(), 999)
            .unwrap_err()
            .to_string(),
        "pixiv account not found"
    );
    assert_eq!(store.read_pixiv_default_user_id().unwrap(), Some(42));
    store.set_pixiv_default_user_id(999).unwrap();
    assert_eq!(
        management
            .remove_account(&Context::new(), 999)
            .unwrap_err()
            .to_string(),
        "pixiv account not found"
    );
    assert_eq!(store.read_pixiv_default_user_id().unwrap(), None);
    management.use_account(&Context::new(), 7).unwrap();
    assert_eq!(service.selected_user_id(&Context::new()).unwrap(), 7);
    management.remove_account(&Context::new(), 7).unwrap();
    assert_eq!(store.read_pixiv_default_user_id().unwrap(), None);
    assert_eq!(service.selected_user_id(&Context::new()).unwrap(), 42);
    management.use_account(&Context::new(), 42).unwrap();
    sql.execute_batch("CREATE TRIGGER reject_delete BEFORE DELETE ON pixiv_account BEGIN SELECT RAISE(ABORT,'synthetic rejection'); END;").unwrap();
    assert!(management.remove_account(&Context::new(), 42).is_err());
    assert_eq!(store.read_pixiv_default_user_id().unwrap(), None);
    assert_eq!(
        database
            .lock()
            .unwrap()
            .get_pixiv(42)
            .unwrap()
            .refresh_token_copy(),
        b"synthetic-token"
    );
    let canceled = Context::new();
    canceled.cancel();
    assert_eq!(
        management
            .use_account(&canceled, 42)
            .unwrap_err()
            .to_string(),
        "context canceled"
    );
    assert_eq!(store.read_pixiv_default_user_id().unwrap(), None);
    management.use_account(&Context::new(), 42).unwrap();
    assert_eq!(
        management
            .remove_account(&canceled, 42)
            .unwrap_err()
            .to_string(),
        "context canceled"
    );
    assert_eq!(store.read_pixiv_default_user_id().unwrap(), None);
    management.use_account(&Context::new(), 42).unwrap();
    management.use_auto().unwrap();
    assert_eq!(store.read_pixiv_default_user_id().unwrap(), None);
}
