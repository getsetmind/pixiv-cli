use chrono::Utc;
use pixiv_app::database::{AccountError, Database, PixivAccount};
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
struct Input {
    id: i64,
    order: i64,
    name: String,
    token: String,
    revision: i64,
    premium: Option<bool>,
    checked: Option<i64>,
    frozen: Option<i64>,
    selected: bool,
    schedulable: bool,
}

#[derive(Deserialize)]
struct Operation {
    action: String,
    accounts: Option<Vec<Input>>,
    id: i64,
    revision: i64,
    token: String,
    name: String,
    premium: Option<bool>,
    checked: Option<i64>,
    error: String,
    kind: String,
    rows: Value,
    get: Value,
}

fn snapshot(account: PixivAccount, start: i64, end: i64) -> Value {
    let normalize = |time| {
        if time == 11 || time == 22 {
            return time;
        }
        assert!(
            (start..=end).contains(&time),
            "timestamp outside operation window: {time}"
        );
        -1
    };
    json!({"id": account.user_id, "order": account.sort_order, "name": account.username,
        "token": String::from_utf8(account.refresh_token_copy()).unwrap(), "revision": account.credential_revision,
        "premium": account.premium_status, "checked": account.premium_checked_at, "frozen": account.pool_frozen_until,
        "selected": account.pool_last_selected, "schedulable": account.schedulable,
        "created": normalize(account.created_at), "updated": normalize(account.updated_at)})
}

#[test]
fn accounts_match_go_atomic_import_metadata_removal_and_revision_conflicts() {
    let operations: Vec<Operation> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/accounts.json"
    ))
    .unwrap();
    assert_eq!(operations.len(), 31);
    let directory = tempfile::tempdir().unwrap();
    let mut database = Database::open(directory.path()).unwrap();
    for (index, operation) in operations.into_iter().enumerate() {
        let connection = Connection::open(database.path()).unwrap();
        connection
            .execute("UPDATE pixiv_account SET created_at=11,updated_at=22", [])
            .unwrap();
        drop(connection);
        let start = Utc::now().timestamp();
        let accounts: Vec<_> = operation
            .accounts
            .unwrap_or_default()
            .into_iter()
            .map(|input| {
                let mut account = PixivAccount::new(input.id, input.name, input.token.as_bytes());
                account.sort_order = input.order;
                account.credential_revision = input.revision;
                account.premium_status = input.premium;
                account.premium_checked_at = input.checked;
                account.pool_frozen_until = input.frozen;
                account.pool_last_selected = input.selected;
                account.schedulable = input.schedulable;
                account
            })
            .collect();
        let mut got = None;
        let result = match operation.action.as_str() {
            "list" => Ok(()),
            "get" => database.get_pixiv(operation.id).map(|account| {
                got = Some(account);
            }),
            "save" => database.save_pixiv_credential(&accounts[0]),
            "batch" => database.save_pixiv_credentials(&accounts),
            "metadata" => database.update_pixiv_metadata(
                operation.id,
                &operation.name,
                operation.premium,
                operation.checked,
            ),
            "rotate" => database.rotate_pixiv_credentials(
                operation.id,
                operation.revision,
                operation.token.as_bytes(),
            ),
            "remove" => database.remove_pixiv(operation.id),
            action => panic!("unknown action {action}"),
        };
        let end = Utc::now().timestamp();
        if operation.error.is_empty() {
            result.unwrap();
        } else {
            let error = result.unwrap_err();
            let kind = match &error {
                AccountError::NotFound => "not_found",
                AccountError::CredentialConflict => "credential_conflict",
                AccountError::DuplicateAccount(_) => "duplicate",
                AccountError::InvalidAccount
                | AccountError::InvalidMetadata
                | AccountError::InvalidRotation => "invalid",
                AccountError::Storage(_) => "storage",
            };
            assert_eq!(kind, operation.kind, "step {index}");
            if kind != "storage" {
                assert_eq!(error.to_string(), operation.error, "step {index}");
            } else {
                let AccountError::Storage(sqlite) = error else {
                    unreachable!()
                };
                assert_eq!(
                    sqlite.sqlite_error_code(),
                    Some(rusqlite::ErrorCode::ConstraintViolation)
                );
                let field = if operation.error.contains("sort_order") {
                    "pixiv_account.sort_order"
                } else {
                    "pixiv_account.pool_last_selected"
                };
                assert!(sqlite.to_string().contains(field));
            }
        }
        assert_eq!(
            got.map(|account| snapshot(account, start, end))
                .unwrap_or(Value::Null),
            operation.get,
            "step {index}"
        );
        let rows: Vec<_> = database
            .list_pixiv()
            .unwrap()
            .into_iter()
            .map(|account| snapshot(account, start, end))
            .collect();
        assert_eq!(json!(rows), operation.rows, "step {index}");
    }
}

#[test]
fn account_credentials_are_copied_and_debug_output_is_redacted() {
    let mut token = b"synthetic-private-token".to_vec();
    let account = PixivAccount::new(1, "user", &token);
    token.fill(b'x');
    let mut copy = account.refresh_token_copy();
    copy.fill(b'y');
    assert_eq!(account.refresh_token_copy(), b"synthetic-private-token");
    assert!(!format!("{account:?}").contains("synthetic-private-token"));
    let directory = tempfile::tempdir().unwrap();
    let mut database = Database::open(directory.path()).unwrap();
    database.save_pixiv_credential(&account).unwrap();
    let mut returned = database.get_pixiv(1).unwrap().refresh_token_copy();
    returned.fill(b'z');
    assert_eq!(
        database.get_pixiv(1).unwrap().refresh_token_copy(),
        b"synthetic-private-token"
    );
}

#[test]
fn stale_parallel_refreshes_commit_exactly_one_rotation() {
    use std::sync::{Arc, Barrier, Mutex};
    let directory = tempfile::tempdir().unwrap();
    let mut database = Database::open(directory.path()).unwrap();
    database
        .save_pixiv_credential(&PixivAccount::new(1, "user", b"synthetic-first"))
        .unwrap();
    let database = Arc::new(Mutex::new(database));
    let barrier = Arc::new(Barrier::new(8));
    let outcomes = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|index| {
                let database = Arc::clone(&database);
                let barrier = Arc::clone(&barrier);
                scope.spawn(move || {
                    barrier.wait();
                    database.lock().unwrap().rotate_pixiv_credentials(
                        1,
                        1,
                        format!("synthetic-{index}").as_bytes(),
                    )
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter(|result| matches!(result, Err(AccountError::CredentialConflict)))
            .count(),
        7
    );
    assert_eq!(
        database
            .lock()
            .unwrap()
            .get_pixiv(1)
            .unwrap()
            .credential_revision,
        2
    );
}

#[test]
#[ignore = "invoked by the Go account repository interoperability contract"]
fn accounts_exchange_with_go_repository() {
    let directory = std::env::var_os("PIXIV_MIGRATION_DATABASE_DIRECTORY").unwrap();
    let mut database = Database::open(std::path::PathBuf::from(directory)).unwrap();
    match std::env::var("PIXIV_MIGRATION_ACCOUNT_PHASE")
        .unwrap()
        .as_str()
    {
        "write" => {
            let before = database.get_pixiv(7).unwrap();
            assert_eq!(before.credential_revision, 1);
            assert_eq!(before.refresh_token_copy(), b"synthetic-go-first");
            database
                .rotate_pixiv_credentials(7, 1, b"synthetic-rust-rotated")
                .unwrap();
            assert!(matches!(
                database.rotate_pixiv_credentials(7, 1, b"synthetic-stale"),
                Err(AccountError::CredentialConflict)
            ));
            database
                .update_pixiv_metadata(7, "rust-metadata", Some(false), Some(333))
                .unwrap();
            database
                .save_pixiv_credential(&PixivAccount::new(9, "rust-new", b"synthetic-rust-new"))
                .unwrap();
        }
        "read" => {
            let account = database.get_pixiv(7).unwrap();
            assert_eq!(account.credential_revision, 3);
            assert_eq!(account.refresh_token_copy(), b"synthetic-go-rotated");
            assert_eq!(account.username, "rust-metadata");
            assert_eq!(account.premium_status, Some(false));
            assert_eq!(account.premium_checked_at, Some(333));
            assert_eq!(account.pool_frozen_until, Some(321));
            assert!(account.pool_last_selected);
            assert!(matches!(database.get_pixiv(9), Err(AccountError::NotFound)));
        }
        phase => panic!("unknown account phase {phase}"),
    }
}
