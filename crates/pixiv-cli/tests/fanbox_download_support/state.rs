use super::schema::{FileState, Input};
use pixiv_app::database::Database;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Component, Path},
};

pub fn seed(directory: &Path, input: &Input) {
    std::fs::create_dir_all(directory).unwrap();
    std::fs::write(directory.join("config.toml"), &input.config).unwrap();
    if input.db_failure == "corrupt" {
        std::fs::write(directory.join("pixiv-cli.db"), b"owned non-SQLite database").unwrap();
        return;
    }
    let database = Database::open(directory).unwrap();
    let connection = rusqlite::Connection::open(database.path()).unwrap();
    connection
        .execute("UPDATE schema_migration SET applied_at=10", [])
        .unwrap();
    connection.execute("INSERT INTO pixiv_account(user_id,sort_order,username,refresh_token,credential_revision,created_at,updated_at) VALUES(99,1,'pixiv canary',x'6e657665722d757365',1,11,22)",[]).unwrap();
    if input.saved != "none" {
        let session = match input.saved.as_str() {
            "empty" => "",
            "invalid" => "bad\r\nsecret-session",
            _ => "owned-session-42",
        };
        for (id, order, session) in [(42, 9, session), (7, 2, "owned-session-7")] {
            connection.execute("INSERT INTO fanbox_account(user_id,sort_order,display_name,creator_id,session_id,credential_revision,validated_at,created_at,updated_at) VALUES(?1,?2,'owned account',?3,?4,3,10,11,22)",rusqlite::params![id,order,id.to_string(),session.as_bytes()]).unwrap();
        }
    }
    if input.db_failure == "missing-table" {
        connection.execute("DROP TABLE fanbox_account", []).unwrap();
    }
}
pub fn hash(path: &Path) -> String {
    format!("{:x}", Sha256::digest(std::fs::read(path).unwrap()))
}
pub fn rows(path: &Path) -> Vec<String> {
    let connection =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let mut values = vec![];
    for query in [
        "SELECT user_id,sort_order,display_name,creator_id,hex(session_id),credential_revision,validated_at,created_at,updated_at FROM fanbox_account ORDER BY user_id",
        "SELECT user_id,sort_order,username,hex(refresh_token),credential_revision,schedulable,created_at,updated_at FROM pixiv_account ORDER BY user_id",
        "SELECT version,name,checksum,applied_at FROM schema_migration ORDER BY version",
    ] {
        let mut statement = match connection.prepare(query) {
            Ok(statement) => statement,
            Err(error) => {
                let source = std::error::Error::source(&error)
                    .unwrap()
                    .downcast_ref::<rusqlite::ffi::Error>()
                    .unwrap();
                match source.extended_code {
                    26 => assert_eq!(error.to_string(), "file is not a database"),
                    1 => assert!(
                        error
                            .to_string()
                            .starts_with("no such table: fanbox_account")
                    ),
                    code => panic!("unexpected actual query error code {code}: {error}"),
                }
                values.push(error.to_string());
                continue;
            }
        };
        let columns = statement.column_count();
        let result = statement
            .query_map([], |row| {
                let mut values = vec![];
                for index in 0..columns {
                    let value = match row.get_ref(index)? {
                        rusqlite::types::ValueRef::Null => Value::Null,
                        rusqlite::types::ValueRef::Integer(value) => json!(value),
                        rusqlite::types::ValueRef::Real(value) => json!(value),
                        rusqlite::types::ValueRef::Text(value) => {
                            json!(std::str::from_utf8(value).unwrap())
                        }
                        rusqlite::types::ValueRef::Blob(_) => {
                            panic!("persistence query must select hex blobs")
                        }
                    };
                    values.push(value);
                }
                Ok(serde_json::to_string(&values).unwrap())
            })
            .unwrap();
        values.extend(result.map(Result::unwrap));
    }
    values
}

pub fn owned_relative(path: &Path) {
    assert!(!path.as_os_str().is_empty());
    assert!(
        path.components()
            .all(|part| matches!(part, Component::Normal(_) | Component::CurDir))
    );
}
pub fn seed_files(home: &Path, input: &Input) {
    owned_relative(Path::new(&input.output_root));
    for file in &input.seed_files {
        owned_relative(Path::new(&file.path));
        assert!(Path::new(&file.path).starts_with(&input.output_root));
        let path = home.join(&file.path);
        let mode = u32::from_str_radix(&file.mode, 8).unwrap();
        if file.kind == "directory" {
            fs::create_dir_all(&path).unwrap();
        } else {
            assert_eq!(file.kind, "file");
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, decode_hex(&file.bytes_hex)).unwrap();
        }
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }
}
pub fn files(home: &Path, root: &str) -> Vec<FileState> {
    fn walk(home: &Path, path: &Path, found: &mut Vec<FileState>) {
        let metadata = fs::symlink_metadata(path).unwrap();
        assert!(metadata.is_dir() || metadata.is_file());
        let bytes = if metadata.is_file() {
            fs::read(path).unwrap()
        } else {
            vec![]
        };
        found.push(FileState {
            path: path
                .strip_prefix(home)
                .unwrap()
                .to_str()
                .unwrap()
                .replace('\\', "/"),
            kind: if metadata.is_file() {
                "file"
            } else {
                "directory"
            }
            .into(),
            mode: format!("{:04o}", metadata.permissions().mode() & 0o777),
            bytes_hex: hex(&bytes),
            size: bytes.len(),
            sha256: if metadata.is_file() {
                format!("{:x}", Sha256::digest(&bytes))
            } else {
                String::new()
            },
        });
        if metadata.is_dir() {
            let mut children = fs::read_dir(path)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect::<Vec<_>>();
            children.sort();
            for child in children {
                walk(home, &child, found);
            }
        }
    }
    let path = home.join(root);
    let mut found = vec![];
    if path.exists() {
        walk(home, &path, &mut found);
    }
    found
}
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn decode_hex(value: &str) -> Vec<u8> {
    assert!(value.len().is_multiple_of(2));
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).unwrap())
        .collect()
}
