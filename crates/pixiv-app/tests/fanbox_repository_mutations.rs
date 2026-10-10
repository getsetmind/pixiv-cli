use pixiv_app::{
    database::{Database, FanboxAccountError},
    fanbox_account::{Account, Repository},
    lifecycle::{Context, ContextError},
    scheduler::SchedulerError,
};
use rusqlite::{Connection, params, types::Type};
use serde_json::{Value, json};
use std::{
    error::Error,
    sync::Mutex,
    time::{Duration, Instant},
};

fn fixture() -> Value {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/fanbox-saved-accounts.json")).unwrap();
    assert_eq!(
        fixture["source_commit"],
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    assert_eq!(fixture["repository"]["cases"].as_array().unwrap().len(), 75);
    assert_eq!(
        fixture["repository"]["source_sha256"]["internal/storage/database/repository.go"],
        "75abdfe0d16877d6cff0820efe705a0bb013ceb58088e372ea1a69c95e913477"
    );
    fixture
}
fn row<'a>(fixture: &'a Value, name: &str) -> &'a Value {
    fixture["repository"]["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == name)
        .unwrap()
}
fn now() -> i64 {
    chrono::Utc::now().timestamp()
}
fn snapshot(account: &Account, start: i64, end: i64) -> Value {
    let stamp = |value: i64| {
        if value > 100 {
            assert!(
                (start..=end).contains(&value),
                "timestamp {value} outside [{start}, {end}]"
            );
            json!("current-time")
        } else {
            json!(value)
        }
    };
    json!({"user_id": account.user_id, "sort_order": account.sort_order, "display_name": account.display_name, "creator_id": account.creator_id, "session": String::from_utf8(account.session_id_copy()).unwrap(), "session_nil": false, "has_session": account.has_session(), "credential_revision": account.credential_revision, "validated_at": account.validated_at, "created_at": stamp(account.created_at), "updated_at": stamp(account.updated_at)})
}
fn snapshots(accounts: &[Account], start: i64, end: i64) -> Value {
    json!(
        accounts
            .iter()
            .map(|account| snapshot(account, start, end))
            .collect::<Vec<_>>()
    )
}
fn assert_snapshot(actual: &Value, expected: &Value, name: &str) {
    if expected["session_nil"] == true {
        // Rust's owned Vec cannot distinguish Go's nil session from empty bytes.
        assert_eq!(actual["session"], "", "{name}");
        assert_eq!(actual["has_session"], false, "{name}");
        let mut expected = expected.clone();
        expected["session_nil"] = json!(false);
        assert_eq!(actual, &expected, "{name}");
    } else {
        assert_eq!(actual, expected, "{name}");
    }
}
fn assert_snapshots(actual: &Value, expected: &Value, name: &str) {
    assert_eq!(
        actual.as_array().unwrap().len(),
        expected.as_array().unwrap().len(),
        "{name}"
    );
    for (actual, expected) in actual
        .as_array()
        .unwrap()
        .iter()
        .zip(expected.as_array().unwrap())
    {
        assert_snapshot(actual, expected, name);
    }
}
fn assert_error(error: Option<&FanboxAccountError>, expected: &Value, name: &str) {
    let text = error.map(ToString::to_string).unwrap_or_default();
    assert_eq!(text, expected["text"], "{name}");
    assert_eq!(
        error.is_some_and(|error| matches!(error, FanboxAccountError::NotFound)),
        expected["not_found"].as_bool().unwrap(),
        "{name}"
    );
    assert_eq!(
        error.is_some_and(|error| matches!(error, FanboxAccountError::CredentialConflict)),
        expected["credential_conflict"].as_bool().unwrap(),
        "{name}"
    );
    assert_eq!(
        error.is_some_and(|error| matches!(
            error,
            FanboxAccountError::Context(ContextError::Canceled)
        )),
        expected["canceled"].as_bool().unwrap(),
        "{name}"
    );
    assert_eq!(
        error.is_some_and(|error| matches!(
            error,
            FanboxAccountError::Context(ContextError::DeadlineExceeded)
        )),
        expected["deadline"].as_bool().unwrap(),
        "{name}"
    );
    let code = match error {
        Some(FanboxAccountError::Storage(rusqlite::Error::SqliteFailure(code, _))) => {
            code.extended_code
        }
        _ => 0,
    };
    assert_eq!(
        i64::from(code),
        expected["sqlite_extended_code"].as_i64().unwrap(),
        "{name}"
    );
    if let Some(error) = error {
        match error {
            FanboxAccountError::Storage(_) => {
                assert!(error.source().unwrap().is::<rusqlite::Error>(), "{name}");
            }
            FanboxAccountError::Context(_) => {
                assert!(error.source().unwrap().is::<ContextError>(), "{name}");
            }
            _ => assert!(error.source().is_none(), "{name}"),
        }
    }
}
fn assert_scheduler_error(error: &SchedulerError, expected: &Value, name: &str) {
    assert_eq!(error.to_string(), expected["text"], "{name}");
    assert_eq!(
        error.is_canceled(),
        expected["canceled"].as_bool().unwrap(),
        "{name}"
    );
    assert_eq!(
        error.is_deadline_exceeded(),
        expected["deadline"].as_bool().unwrap(),
        "{name}"
    );
}
fn input_account(input: &Value) -> Account {
    let mut account = Account::new(
        input["id"].as_i64().unwrap(),
        input["display_name"].as_str().unwrap(),
        input["creator_id"].as_str().unwrap(),
        input["session"].as_str().unwrap().as_bytes(),
    );
    account.sort_order = input["sort_order"].as_i64().unwrap();
    account.credential_revision = input["revision"].as_i64().unwrap();
    account.validated_at = input["validated_at"].as_i64().unwrap();
    account.created_at = 88;
    account.updated_at = 99;
    account
}
fn saved_account() -> Account {
    let mut account = Account::new(7, "fixture", "", b"fixture-session");
    account.validated_at = 30;
    account
}
fn seed(database: &Database) {
    Connection::open(database.path()).unwrap().execute("INSERT INTO fanbox_account(user_id,sort_order,display_name,creator_id,session_id,credential_revision,validated_at,created_at,updated_at) VALUES(7,1,'fixture','',?1,1,30,11,22)", [b"fixture-session".as_slice()]).unwrap();
}
fn execute(database: &Database, sql: &str) {
    Connection::open(database.path())
        .unwrap()
        .execute_batch(sql)
        .unwrap();
}
fn fault_action(
    database: &mut Database,
    context: &Context,
    action: &str,
) -> Result<(), FanboxAccountError> {
    match action {
        "save" | "save_update" => database.save_fanbox_credential(context, &saved_account()),
        "save_insert" => {
            let mut account = Account::new(8, "new", "", b"fixture-new");
            account.validated_at = 31;
            database.save_fanbox_credential(context, &account)
        }
        "rotate" => database.rotate_fanbox_session(context, 7, 1, b"fixture-rotated", 31),
        "remove" => database.remove_fanbox(context, 7),
        "list" => database.list_fanbox().map(|_| ()),
        "get" => database.get_fanbox(7).map(|_| ()),
        _ => panic!("unexpected action {action}"),
    }
}

#[test]
fn sequential_production_mutations_match_all_31_frozen_state_transitions() {
    let fixture = fixture();
    let dir = tempfile::tempdir().unwrap();
    let mut database = Database::open(dir.path()).unwrap();
    for case in &fixture["repository"]["cases"].as_array().unwrap()[..31] {
        let name = case["name"].as_str().unwrap();
        let input = &case["input"];
        execute(
            &database,
            "UPDATE fanbox_account SET created_at=11,updated_at=22",
        );
        let start = now();
        let mut result = None;
        let outcome = match input["action"].as_str().unwrap() {
            "save" => database.save_fanbox_credential(&Context::new(), &input_account(input)),
            "rotate" => database.rotate_fanbox_session(
                &Context::new(),
                input["id"].as_i64().unwrap(),
                input["revision"].as_i64().unwrap(),
                input["session"].as_str().unwrap().as_bytes(),
                input["validated_at"].as_i64().unwrap(),
            ),
            "remove" => database.remove_fanbox(&Context::new(), input["id"].as_i64().unwrap()),
            "get" => database
                .get_fanbox(input["id"].as_i64().unwrap())
                .map(|account| {
                    result = Some(snapshot(&account, start, now()));
                }),
            "list" => database.list_fanbox().map(|accounts| {
                result = Some(snapshots(&accounts, start, now()));
            }),
            action => panic!("unexpected action {action}"),
        };
        assert_error(outcome.as_ref().err(), &case["error"], name);
        if outcome.is_ok() && !case["result"].is_null() {
            assert_eq!(result.as_ref().unwrap(), &case["result"], "{name}");
        }
        if outcome.is_err() && input["action"] == "get" {
            assert!(
                result.is_none(),
                "Rust Result exposes no account on error: {name}"
            );
            assert_eq!(
                case["result"]["user_id"], 0,
                "Go returns a zero account: {name}"
            );
            assert_eq!(case["result"]["session_nil"], true, "{name}");
        }
        assert_eq!(
            snapshots(&database.list_fanbox().unwrap(), start, now()),
            case["rows"],
            "{name}"
        );
        let reopened = Database::open(dir.path()).unwrap();
        assert_eq!(
            snapshots(&reopened.list_fanbox().unwrap(), start, now()),
            case["rows"],
            "durable {name}"
        );
    }
}

#[test]
fn context_failures_keep_exact_causes_and_leave_sqlite_unchanged() {
    let fixture = fixture();
    for action in ["save", "rotate", "list", "get", "remove"] {
        for failure in ["canceled", "deadline"] {
            let name = format!("{action}/{failure}");
            let case = row(&fixture, &name);
            let dir = tempfile::tempdir().unwrap();
            let mut database = Database::open(dir.path()).unwrap();
            seed(&database);
            let context = if failure == "deadline" {
                Context::with_deadline(Instant::now() - Duration::from_secs(1))
            } else {
                Context::new()
            };
            if failure == "canceled" {
                context.cancel();
            }
            if matches!(action, "list" | "get") {
                let repository = Mutex::new(database);
                let error = if action == "list" {
                    repository.list(&context).map(|_| ()).unwrap_err()
                } else {
                    repository.get(&context, 7).map(|_| ()).unwrap_err()
                };
                assert_scheduler_error(&error, &case["error"], &name);
                database = repository
                    .into_inner()
                    .unwrap_or_else(|_| panic!("context failure poisoned repository"));
            } else {
                let error = fault_action(&mut database, &context, action).unwrap_err();
                assert_error(Some(&error), &case["error"], &name);
            }
            assert_eq!(
                snapshots(&database.list_fanbox().unwrap(), 0, 0),
                case["rows"],
                "{name}"
            );
        }
    }
}

#[test]
fn missing_tables_keep_typed_storage_errors_for_every_repository_operation() {
    let fixture = fixture();
    for action in ["save", "rotate", "list", "get", "remove"] {
        let name = format!("{action}/missing_table");
        let case = row(&fixture, &name);
        let dir = tempfile::tempdir().unwrap();
        let mut database = Database::open(dir.path()).unwrap();
        seed(&database);
        execute(&database, "DROP TABLE fanbox_account");
        let error = fault_action(&mut database, &Context::new(), action).unwrap_err();
        assert_error(Some(&error), &case["error"], &name);
        assert!(case["rows"].is_null(), "{name}");
        let connection = Connection::open(database.path()).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE name='fanbox_account'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0,
            "{name}"
        );
    }
}

#[test]
fn closed_go_handles_are_a_consuming_rust_ownership_boundary() {
    let fixture = fixture();
    for action in ["save", "rotate", "list", "get", "remove"] {
        let name = format!("{action}/closed");
        let case = row(&fixture, &name);
        assert_eq!(
            case["error"],
            json!({"text":"sql: database is closed","not_found":false,"credential_conflict":false,"canceled":false,"deadline":false,"sqlite_extended_code":0}),
            "{name}"
        );
        assert!(case["rows"].is_null(), "{name}");
        let dir = tempfile::tempdir().unwrap();
        let database = Database::open(dir.path()).unwrap();
        seed(&database);
        let path = database.path().to_owned();
        // Closing in Rust consumes the handle, so a subsequent repository call is not expressible.
        drop(database);
        Connection::open(&path).unwrap().close().unwrap();
        let reopened = Database::open(dir.path()).unwrap();
        assert_eq!(
            reopened.get_fanbox(7).unwrap().session_id_copy(),
            b"fixture-session",
            "{name}"
        );
    }
}

#[test]
fn sqlite_trigger_failures_rollback_every_mutation_without_partial_state() {
    let fixture = fixture();
    for action in ["save_insert", "save_update", "rotate", "remove"] {
        let name = format!("{action}/trigger rollback");
        let case = row(&fixture, &name);
        let dir = tempfile::tempdir().unwrap();
        let mut database = Database::open(dir.path()).unwrap();
        seed(&database);
        execute(
            &database,
            &format!(
                "CREATE TRIGGER reject_fanbox BEFORE {} ON fanbox_account BEGIN SELECT RAISE(ABORT,'owned fixture persist failure'); END",
                case["input"]["event"].as_str().unwrap()
            ),
        );
        let error = fault_action(&mut database, &Context::new(), action).unwrap_err();
        assert_error(Some(&error), &case["error"], &name);
        assert_eq!(
            snapshots(&database.list_fanbox().unwrap(), 0, 0),
            case["rows"],
            "{name}"
        );
        let reopened = Database::open(dir.path()).unwrap();
        assert_eq!(
            snapshots(&reopened.list_fanbox().unwrap(), 0, 0),
            case["rows"],
            "durable {name}"
        );
        execute(&database, "DROP TRIGGER reject_fanbox");
        fault_action(&mut database, &Context::new(), action).unwrap();
    }
}

#[test]
fn failed_deferred_commits_preserve_go_visible_transaction_until_connection_drop() {
    let fixture = fixture();
    for action in ["save_insert", "save_update"] {
        let name = format!("{action}/deferred commit failure");
        let case = row(&fixture, &name);
        let dir = tempfile::tempdir().unwrap();
        let mut database = Database::open(dir.path()).unwrap();
        seed(&database);
        execute(
            &database,
            "CREATE TABLE owned_parent(id INTEGER PRIMARY KEY); CREATE TABLE owned_child(id INTEGER REFERENCES owned_parent(id) DEFERRABLE INITIALLY DEFERRED)",
        );
        let event = if action == "save_insert" {
            "INSERT"
        } else {
            "UPDATE"
        };
        execute(
            &database,
            &format!(
                "CREATE TRIGGER deferred_fanbox AFTER {event} ON fanbox_account BEGIN INSERT INTO owned_child VALUES(999); END"
            ),
        );
        let account = if action == "save_insert" {
            let mut account = Account::new(8, "new", "", b"fixture-uncommitted");
            account.validated_at = 31;
            account
        } else {
            saved_account()
        };
        let start = now();
        let error = database
            .save_fanbox_credential(&Context::new(), &account)
            .unwrap_err();
        assert_error(Some(&error), &case["error"], &name);
        assert_eq!(
            snapshots(&database.list_fanbox().unwrap(), start, now()),
            case["rows"],
            "same connection {name}"
        );
        let mut next = Account::new(9, "next", "", b"fixture-next");
        next.validated_at = 32;
        let error = database
            .save_fanbox_credential(&Context::new(), &next)
            .unwrap_err();
        assert_error(Some(&error), &case["result"]["next_save_error"], &name);
        let durable = Connection::open(database.path()).unwrap();
        assert_eq!(
            durable
                .query_row("SELECT count(*) FROM fanbox_account", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1,
            "other connection {name}"
        );
        assert_eq!(
            durable
                .query_row(
                    "SELECT credential_revision FROM fanbox_account WHERE user_id=7",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            1,
            "other connection {name}"
        );
        assert_eq!(
            durable
                .query_row("SELECT count(*) FROM owned_child", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0,
            "other connection {name}"
        );
        drop(database);
        drop(durable);
        let reopened = Database::open(dir.path()).unwrap();
        assert_eq!(
            snapshots(&reopened.list_fanbox().unwrap(), 0, 0),
            case["result"]["durable_after_close"],
            "after close {name}"
        );
        let connection = Connection::open(reopened.path()).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM owned_child", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0,
            "{name}"
        );
    }
}

#[test]
fn malformed_scan_rows_preserve_full_discard_and_session_conversion() {
    let fixture = fixture();
    let columns = [
        "user_id",
        "sort_order",
        "display_name",
        "creator_id",
        "session_id",
        "credential_revision",
        "validated_at",
        "created_at",
        "updated_at",
    ];
    for (index, column) in columns.into_iter().enumerate() {
        for malformed in ["null", "bad_type"] {
            let name = format!("scan/{column}/{malformed}");
            let case = row(&fixture, &name);
            let dir = tempfile::tempdir().unwrap();
            let database = Database::open(dir.path()).unwrap();
            execute(
                &database,
                "DROP TABLE fanbox_account; CREATE TABLE fanbox_account(user_id,sort_order,display_name,creator_id,session_id,credential_revision,validated_at,created_at,updated_at); INSERT INTO fanbox_account VALUES(7,1,'first','',x'6162',1,30,11,22),(8,2,'second','creator',x'6364',2,31,12,23)",
            );
            let value = if malformed == "null" {
                "NULL"
            } else {
                "'not-an-integer'"
            };
            let selector = if column == "user_id" {
                "sort_order=2"
            } else {
                "user_id=8"
            };
            execute(
                &database,
                &format!("UPDATE fanbox_account SET {column}={value} WHERE {selector}"),
            );
            let listed = database.list_fanbox();
            let got = database.get_fanbox(8);
            if case["error"]["text"] == "" {
                assert_eq!(case["result"]["list_nil"], false, "{name}");
                assert_snapshots(&snapshots(&listed.unwrap(), 0, 0), &case["rows"], &name);
                assert_snapshot(
                    &snapshot(&got.unwrap(), 0, 0),
                    &case["result"]["get"],
                    &name,
                );
            } else {
                assert_eq!(case["result"]["list_nil"], true, "{name}");
                assert!(case["rows"].is_null(), "{name}");
                let error = listed.unwrap_err();
                let expected_type = if malformed == "null" {
                    Type::Null
                } else {
                    Type::Text
                };
                // The concrete Go scan diagnostic differs from rusqlite's typed column error.
                assert!(
                    case["error"]["text"]
                        .as_str()
                        .unwrap()
                        .starts_with(&format!(
                            "sql: Scan error on column index {index}, name \"{column}\":"
                        )),
                    "{name}"
                );
                assert!(
                    matches!(error, FanboxAccountError::Storage(rusqlite::Error::InvalidColumnType(actual, ref actual_column, actual_type)) if actual == index && actual_column == column && actual_type == expected_type),
                    "{name}: {error}"
                );
                if column == "user_id" {
                    assert_error(got.as_ref().err(), &case["result"]["get_error"], &name);
                } else {
                    let error = got.unwrap_err();
                    assert!(
                        matches!(error, FanboxAccountError::Storage(rusqlite::Error::InvalidColumnType(actual, ref actual_column, actual_type)) if actual == index && actual_column == column && actual_type == expected_type),
                        "{name}: {error}"
                    );
                }
            }
        }
    }
}

#[test]
fn repository_mutations_forward_context_and_use_real_persisted_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let repository = Mutex::new(Database::open(dir.path()).unwrap());
    let context = Context::new();
    repository
        .save_credential(&context, &saved_account())
        .unwrap();
    assert_eq!(repository.get(&context, 7).unwrap().credential_revision, 1);
    repository
        .rotate_session(&context, 7, 1, b"fixture-rotation", 31)
        .unwrap();
    let stored = repository.get(&context, 7).unwrap();
    assert_eq!(stored.credential_revision, 2);
    assert_eq!(stored.session_id_copy(), b"fixture-rotation");
    assert_eq!(stored.validated_at, 31);
    context.cancel();
    assert!(
        repository
            .save_credential(&context, &saved_account())
            .unwrap_err()
            .is_canceled()
    );
    assert!(
        repository
            .rotate_session(&context, 7, 2, b"fixture-canceled", 32)
            .unwrap_err()
            .is_canceled()
    );
    assert!(repository.remove(&context, 7).unwrap_err().is_canceled());
    let fresh = Context::new();
    assert_eq!(repository.get(&fresh, 7).unwrap().credential_revision, 2);
    repository.remove(&fresh, 7).unwrap();
    assert!(repository.list(&fresh).unwrap().is_empty());
}

#[test]
fn invalid_mutation_inputs_precede_context_errors_and_never_start_transactions() {
    let dir = tempfile::tempdir().unwrap();
    let mut database = Database::open(dir.path()).unwrap();
    seed(&database);
    let context = Context::new();
    context.cancel();
    let mut invalid = saved_account();
    invalid.user_id = 0;
    assert!(matches!(
        database.save_fanbox_credential(&context, &invalid),
        Err(FanboxAccountError::InvalidAccount)
    ));
    assert!(matches!(
        database.rotate_fanbox_session(&context, 7, 0, b"fixture-invalid", 31),
        Err(FanboxAccountError::InvalidRotation)
    ));
    assert_eq!(database.get_fanbox(7).unwrap().credential_revision, 1);
    let connection = Connection::open(database.path()).unwrap();
    let mut value: Vec<u8> = connection
        .query_row(
            "SELECT session_id FROM fanbox_account WHERE user_id=?1",
            params![7],
            |row| row.get(0),
        )
        .unwrap();
    value[0] = b'X';
    assert_eq!(
        database.get_fanbox(7).unwrap().session_id_copy(),
        b"fixture-session"
    );
}

#[test]
fn source_scan_conversions_accept_numeric_text_blob_and_go_float_rendering() {
    let dir = tempfile::tempdir().unwrap();
    let database = Database::open(dir.path()).unwrap();
    execute(
        &database,
        "DROP TABLE fanbox_account; CREATE TABLE fanbox_account(user_id,sort_order,display_name,creator_id,session_id,credential_revision,validated_at,created_at,updated_at)",
    );
    let connection = Connection::open(database.path()).unwrap();
    connection
        .execute(
            "INSERT INTO fanbox_account VALUES('8',x'32',42,2.5,23,'2',x'3331',12.0,'23')",
            [],
        )
        .unwrap();
    let account = database.list_fanbox().unwrap().remove(0);
    assert_eq!(account.user_id, 8);
    assert_eq!(account.sort_order, 2);
    assert_eq!(account.display_name, "42");
    assert_eq!(account.creator_id, "2.5");
    assert_eq!(account.session_id_copy(), b"23");
    assert_eq!(account.credential_revision, 2);
    assert_eq!(account.validated_at, 31);
    assert_eq!(account.created_at, 12);
    assert_eq!(account.updated_at, 23);
    for (number, rendered) in [
        (1_000_000.0, "1e+06"),
        (0.00001, "1e-05"),
        (-0.0, "-0"),
        (f64::INFINITY, "+Inf"),
        (f64::NEG_INFINITY, "-Inf"),
    ] {
        connection
            .execute(
                "UPDATE fanbox_account SET display_name=?1,creator_id=?1,session_id=?1",
                [number],
            )
            .unwrap();
        let account = database.list_fanbox().unwrap().remove(0);
        assert_eq!(account.display_name, rendered);
        assert_eq!(account.creator_id, rendered);
        assert_eq!(account.session_id_copy(), rendered.as_bytes());
    }
    for value in [
        "'2.5'",
        "2.5",
        "1000000.0",
        "'9223372036854775808'",
        "' 2 '",
        "x'32ff'",
    ] {
        connection
            .execute_batch(&format!("UPDATE fanbox_account SET sort_order={value}"))
            .unwrap();
        assert!(
            matches!(
                database.list_fanbox(),
                Err(FanboxAccountError::Storage(
                    rusqlite::Error::InvalidColumnType(1, _, _)
                ))
            ),
            "{value}"
        );
    }
}
