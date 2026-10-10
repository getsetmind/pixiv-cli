use pixiv_app::database::{AccountError, Database, PixivAccount};
use rusqlite::{Connection, ErrorCode};
use serde::Deserialize;
use std::{collections::BTreeMap, error::Error};

#[derive(Deserialize)]
struct GoContract {
    source_commit: String,
    source_sha256: BTreeMap<String, String>,
    go_version: String,
    driver_version: String,
    sqlite_version: String,
    cases: Vec<GoCase>,
}

#[derive(Deserialize)]
struct GoCase {
    name: String,
    trigger: String,
    message: String,
    extended_code: i32,
    token: String,
    revision: i64,
    account_count: usize,
}

fn go_contract() -> GoContract {
    let contract: GoContract =
        serde_json::from_str(include_str!("fixtures/account_storage_errors.json")).unwrap();
    assert_eq!(
        contract.source_commit,
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    assert_eq!(contract.go_version, "go1.27.1");
    assert_eq!(contract.driver_version, "v1.40.1");
    assert_eq!(contract.sqlite_version, "3.50.4");
    assert_eq!(
        contract.source_sha256["internal/storage/database/repository.go"],
        "75abdfe0d16877d6cff0820efe705a0bb013ceb58088e372ea1a69c95e913477"
    );
    assert_eq!(contract.cases.len(), 3);
    contract
}

fn sqlite_source(error: &AccountError) -> &rusqlite::Error {
    let source = error
        .source()
        .unwrap()
        .downcast_ref::<rusqlite::Error>()
        .unwrap();
    let AccountError::Storage(storage) = error else {
        panic!("expected original storage error");
    };
    assert!(std::ptr::eq(source, storage));
    source
}

#[test]
fn rejected_credential_rotations_keep_go_storage_text_typed_cause_and_account_state() {
    for case in go_contract()
        .cases
        .into_iter()
        .filter(|case| !case.trigger.is_empty())
    {
        let directory = tempfile::tempdir().unwrap();
        let mut database = Database::open(directory.path()).unwrap();
        database
            .save_pixiv_credential(&PixivAccount::new(7, "fixture", b"fixture-original"))
            .unwrap();
        let before = database.get_pixiv(7).unwrap();
        let connection = Connection::open(database.path()).unwrap();
        connection.execute_batch(&case.trigger).unwrap();
        let error = database
            .rotate_pixiv_credentials(7, 1, b"fixture-rotated")
            .unwrap_err();
        assert_eq!(error.to_string(), case.message, "{}", case.name);
        let source = sqlite_source(&error);
        assert_eq!(
            source.sqlite_error_code(),
            Some(ErrorCode::ConstraintViolation)
        );
        assert_eq!(
            source.sqlite_error().unwrap().extended_code,
            case.extended_code
        );
        let after = database.get_pixiv(7).unwrap();
        assert_eq!(after.refresh_token_copy(), before.refresh_token_copy());
        assert_eq!(after.credential_revision, before.credential_revision);
        assert_eq!(after.updated_at, before.updated_at);
        assert_eq!(after.refresh_token_copy(), case.token.as_bytes());
        assert_eq!(after.credential_revision, case.revision);
        assert_eq!(database.list_pixiv().unwrap().len(), case.account_count);
    }
}

#[test]
fn account_unique_constraints_keep_go_storage_prefix_extended_code_and_atomicity() {
    let case = go_contract()
        .cases
        .into_iter()
        .find(|case| case.name == "unique")
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let mut database = Database::open(directory.path()).unwrap();
    database
        .save_pixiv_credential(&PixivAccount::new(7, "fixture", b"fixture-original"))
        .unwrap();
    let first = PixivAccount::new(8, "first", b"fixture-first");
    let mut conflicting = PixivAccount::new(9, "conflicting", b"fixture-second");
    conflicting.sort_order = 1;
    let error = database
        .save_pixiv_credentials(&[first, conflicting])
        .unwrap_err();
    assert_eq!(error.to_string(), case.message);
    let source = sqlite_source(&error);
    assert_eq!(
        source.sqlite_error_code(),
        Some(ErrorCode::ConstraintViolation)
    );
    assert_eq!(
        source.sqlite_error().unwrap().extended_code,
        case.extended_code
    );
    assert_eq!(database.list_pixiv().unwrap().len(), case.account_count);
    let stored = database.get_pixiv(7).unwrap();
    assert_eq!(stored.refresh_token_copy(), case.token.as_bytes());
    assert_eq!(stored.credential_revision, case.revision);
    assert!(matches!(database.get_pixiv(8), Err(AccountError::NotFound)));
    assert!(matches!(database.get_pixiv(9), Err(AccountError::NotFound)));
}
