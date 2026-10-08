use chrono::Utc;
use pixiv_app::database::{Database, PixivAccount, PoolError, PoolSnapshot, choose_pool_account};
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
struct Operation {
    action: String,
    ids: Option<Vec<i64>>,
    enabled: bool,
    now: i64,
    until: i64,
    strategy: String,
    random: i64,
    error: String,
    kind: String,
    earliest: Option<i64>,
    snapshot: Value,
    status: Value,
    selected: Value,
    rows: Value,
    random_sizes: Option<Vec<usize>>,
}

fn candidate(value: &pixiv_app::database::PoolCandidate) -> Value {
    json!({"UserID":value.user_id,"SortOrder":value.sort_order,"Schedulable":value.schedulable,
        "PoolFrozenUntil":value.pool_frozen_until,"PoolLastSelected":value.pool_last_selected,"Eligible":value.eligible})
}

fn snapshot(value: &PoolSnapshot) -> Value {
    json!({"Candidates":value.candidates.iter().map(candidate).collect::<Vec<_>>(),
        "MarkerUserID":value.marker_user_id,"MarkerSortOrder":value.marker_sort_order,"EarliestFrozenUntil":value.earliest_frozen_until})
}

fn account(value: PixivAccount, start: i64, end: i64, now: i64) -> Value {
    let time = |value| {
        if [11, 22, now].contains(&value) {
            return value;
        }
        assert!((start..=end).contains(&value));
        -1
    };
    json!({"id":value.user_id,"order":value.sort_order,"name":value.username,"token":String::from_utf8(value.refresh_token_copy()).unwrap(),
        "revision":value.credential_revision,"premium":value.premium_status,"checked":value.premium_checked_at,"frozen":value.pool_frozen_until,
        "selected":value.pool_last_selected,"schedulable":value.schedulable,"created":time(value.created_at),"updated":time(value.updated_at)})
}

#[test]
fn pool_matches_go_selection_freeze_membership_snapshots_and_transaction_rollback() {
    let operations: Vec<Operation> =
        serde_json::from_str(include_str!("../../../docs/migration/contracts/pool.json")).unwrap();
    assert_eq!(operations.len(), 51);
    let directory = tempfile::tempdir().unwrap();
    let mut database = Database::open(directory.path()).unwrap();
    for (index, operation) in operations.into_iter().enumerate() {
        let connection = Connection::open(database.path()).unwrap();
        connection
            .execute("UPDATE pixiv_account SET created_at=11,updated_at=22", [])
            .unwrap();
        drop(connection);
        let start = Utc::now().timestamp();
        let ids = operation.ids.unwrap_or_default();
        let mut captured = Value::Null;
        let mut status = Value::Null;
        let mut selected = None;
        let mut sizes = Vec::new();
        let mut chooser = |value: &PoolSnapshot| {
            captured = snapshot(value);
            match operation.strategy.as_str() {
                "outside" => Ok(99),
                "chooser_error" => Err(PoolError::Message("synthetic chooser error".into())),
                strategy => {
                    let mut random = |size| {
                        sizes.push(size);
                        if strategy == "random_error" {
                            Err(PoolError::Message("synthetic random error".into()))
                        } else {
                            Ok(operation.random)
                        }
                    };
                    choose_pool_account(
                        value,
                        if strategy == "random_error" {
                            "random"
                        } else {
                            strategy
                        },
                        Some(&mut random),
                    )
                }
            }
        };
        let result = match operation.action.as_str() {
            "seed" => {
                for id in &ids {database.save_pixiv_credential(&PixivAccount::new(*id,"synthetic",b"synthetic-token")).unwrap();}
                Ok(())
            },
            "members" => database.set_pixiv_schedulable(&ids,operation.enabled),
            "all" => database.set_all_pixiv_schedulable(operation.enabled),
            "freeze" => database.freeze_pixiv(ids[0],operation.until),
            "remove" => database.remove_pixiv(ids[0]).map_err(PoolError::Account),
            "status" => database.list_pixiv_pool_status(operation.now).map(|value| {
                status=json!({"Accounts":value.accounts.iter().map(candidate).collect::<Vec<_>>(),"EarliestFrozenUntil":value.earliest_frozen_until});
            }),
            "select" => {
                let callback = if operation.strategy=="nil" {None} else {Some(&mut chooser as &mut dyn FnMut(&PoolSnapshot)->Result<i64,PoolError>)};
                database.select_pixiv(operation.now,&ids,callback).map(|value| {selected=Some(value);})
            },
            action => panic!("unknown action {action}"),
        };
        let end = Utc::now().timestamp();
        if operation.error.is_empty() {
            result.unwrap();
        } else {
            let error = result.unwrap_err();
            assert_eq!(error.to_string(), operation.error, "step {index}");
            if let PoolError::Selection {
                kind,
                earliest_frozen_until,
            } = error
            {
                assert_eq!(kind.as_str(), operation.kind);
                assert_eq!(earliest_frozen_until, operation.earliest);
            } else {
                assert!(operation.kind.is_empty());
            }
        }
        assert_eq!(captured, operation.snapshot, "step {index}");
        assert_eq!(status, operation.status, "step {index}");
        assert_eq!(
            sizes,
            operation.random_sizes.unwrap_or_default(),
            "step {index}"
        );
        assert_eq!(
            selected
                .map(|value| account(value, start, end, operation.now))
                .unwrap_or(Value::Null),
            operation.selected,
            "step {index}"
        );
        let rows: Vec<_> = database
            .list_pixiv()
            .unwrap()
            .into_iter()
            .map(|value| account(value, start, end, operation.now))
            .collect();
        assert_eq!(json!(rows), operation.rows, "step {index}");
    }
}

#[test]
fn pool_default_random_source_and_empty_snapshot_preserve_go_results() {
    use pixiv_app::database::{PoolCandidate, PoolSelectionKind};
    let mut snapshot = PoolSnapshot {
        candidates: vec![PoolCandidate {
            user_id: 42,
            sort_order: 1,
            schedulable: true,
            pool_frozen_until: None,
            pool_last_selected: false,
            eligible: true,
        }],
        marker_user_id: None,
        marker_sort_order: None,
        earliest_frozen_until: None,
    };
    assert_eq!(choose_pool_account(&snapshot, "random", None).unwrap(), 42);
    snapshot.candidates.clear();
    snapshot.earliest_frozen_until = Some(700);
    assert!(matches!(
        choose_pool_account(&snapshot, "unsupported", None),
        Err(PoolError::Selection {
            kind: PoolSelectionKind::Exhausted,
            earliest_frozen_until: Some(700)
        })
    ));
}
