use pixiv_app::database::Database;
use rusqlite::{Connection, types::ValueRef};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
struct Case {
    name: String,
    setup: String,
    error: String,
    state: Value,
}

fn state(connection: &Connection) -> Value {
    let mut output = serde_json::Map::new();
    for key in ["application_id", "user_version"] {
        let value: i64 = connection
            .query_row(&format!("PRAGMA {key}"), [], |row| row.get(0))
            .unwrap();
        output.insert(key.into(), json!(value));
    }
    for (key, query) in [
        (
            "schema",
            "SELECT type,name,sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY type,name",
        ),
        (
            "ledger",
            "SELECT version,name,checksum FROM schema_migration ORDER BY version",
        ),
        (
            "pixiv",
            "SELECT user_id,sort_order,username,hex(refresh_token),credential_revision,schedulable,created_at,updated_at FROM pixiv_account ORDER BY user_id",
        ),
        (
            "fanbox",
            "SELECT user_id,sort_order,display_name,creator_id,hex(session_id),credential_revision,validated_at,created_at,updated_at FROM fanbox_account ORDER BY user_id",
        ),
    ] {
        let Ok(mut statement) = connection.prepare(query) else {
            output.insert(key.into(), Value::Null);
            continue;
        };
        let width = statement.column_count();
        let rows = statement
            .query_map([], |row| {
                (0..width)
                    .map(|index| {
                        Ok(match row.get_ref(index)? {
                            ValueRef::Null => Value::Null,
                            ValueRef::Integer(value) => json!(value),
                            ValueRef::Text(value) => {
                                let text = std::str::from_utf8(value).unwrap();
                                if key == "schema" {
                                    json!(text.replace("\r\n", "\n"))
                                } else {
                                    json!(text)
                                }
                            }
                            value => panic!("unexpected database value: {value:?}"),
                        })
                    })
                    .collect::<rusqlite::Result<Vec<Value>>>()
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        output.insert(key.into(), json!(rows));
    }
    Value::Object(output)
}

#[test]
fn database_matches_go_schema_upgrade_reopen_and_drift_rejection() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/database.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 19);
    for case in cases {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pixiv-cli.db");
        if !case.setup.is_empty() {
            let connection = Connection::open(&path).unwrap();
            connection.execute_batch(&case.setup).unwrap();
        }
        let result = Database::open(directory.path());
        if case.error.is_empty() {
            let database = result.unwrap_or_else(|error| panic!("{}: {error}", case.name));
            assert_eq!(database.path(), path, "{}", case.name);
            drop(database);
        } else {
            let error = result
                .err()
                .unwrap_or_else(|| panic!("{} unexpectedly opened", case.name))
                .to_string();
            if case.name == "empty-ledger" {
                assert!(
                    error.starts_with("apply migration 1 (0001_initial):"),
                    "{error}"
                );
            } else {
                assert_eq!(error, case.error, "{}", case.name);
            }
        }
        let connection = Connection::open(&path).unwrap();
        assert_eq!(state(&connection), case.state, "{}", case.name);
    }
}

#[test]
fn database_rejects_empty_directory_and_creates_private_files() {
    assert_eq!(
        Database::open("").err().unwrap().to_string(),
        "database: app data directory is required"
    );
    let directory = tempfile::tempdir().unwrap();
    let database = Database::open(directory.path()).unwrap();
    drop(database);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            directory.path().metadata().unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            directory
                .path()
                .join("pixiv-cli.db")
                .metadata()
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[test]
#[ignore = "invoked by the Go database interoperability contract"]
fn database_opens_shared_go_file() {
    let directory = std::env::var_os("PIXIV_MIGRATION_DATABASE_DIRECTORY").unwrap();
    let database = Database::open(std::path::PathBuf::from(directory)).unwrap();
    drop(database);
}
