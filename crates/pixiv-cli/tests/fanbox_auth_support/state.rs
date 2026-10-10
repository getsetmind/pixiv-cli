use super::{
    io::encode_hex,
    schema::{Case, State},
};
use pixiv_app::{database::Database, fanbox_account::Account, lifecycle::Context};
use rusqlite::{Connection, OpenFlags};
use serde_json::json;
use std::{
    collections::BTreeMap,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

pub fn seed(home: &Path, case: &Case) {
    let directory = home.join(".pixiv-cli");
    if let Some(config) = &case.config_before {
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("config.toml"), config).unwrap();
        private_mode(&directory, true);
        private_mode(&directory.join("config.toml"), false);
    }
    if !case.seed.is_empty() {
        let mut database = Database::open(&directory).unwrap();
        for id in &case.seed {
            let mut account = Account::new(
                *id,
                format!("seed {id}"),
                "",
                format!("synthetic-seed-{id}").as_bytes(),
            );
            account.validated_at = 1_700_000_000;
            database
                .save_fanbox_credential(&Context::background(), &account)
                .unwrap();
        }
        drop(database);
    }
}

pub fn snapshot(home: &Path, start: i64, created: &mut BTreeMap<i64, i64>) -> State {
    let directory = home.join(".pixiv-cli");
    let config_path = directory.join("config.toml");
    let database_path = directory.join("pixiv-cli.db");
    let config = match std::fs::read_to_string(&config_path) {
        Ok(config) => Some(config),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => panic!("snapshot config: {error}"),
    };
    let mut state = State {
        config,
        ..Default::default()
    };
    for path in [&directory, &config_path, &database_path] {
        if let Ok(metadata) = std::fs::metadata(path) {
            state.modes.insert(
                path.strip_prefix(home)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
                mode(&metadata),
            );
        }
    }
    if !database_path.exists() {
        return state;
    }
    state.database = true;
    let connection =
        Connection::open_with_flags(database_path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    state.user_version = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    state.application_id = connection
        .query_row("PRAGMA application_id", [], |row| row.get(0))
        .unwrap();
    let mut statement = connection.prepare("SELECT user_id,sort_order,display_name,creator_id,session_id,credential_revision,validated_at,created_at,updated_at FROM fanbox_account ORDER BY sort_order,user_id").unwrap();
    let accounts = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Vec<u8>>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, i64>(8)?,
            ))
        })
        .unwrap();
    for account in accounts {
        let (id, order, name, creator, session, revision, valid, create, update) = account.unwrap();
        let end = now();
        assert!(
            valid == 1_700_000_000 || (valid >= start && valid <= end),
            "validation timestamp {valid} outside observed [{start},{end}]"
        );
        assert!(
            create >= start && create <= end && update >= create && update <= end,
            "stored timestamps outside observed interval"
        );
        let preserved = created.get(&id).is_none_or(|old| *old == create);
        assert!(preserved, "upsert changed creation timestamp for {id}");
        created.entry(id).or_insert(create);
        state.rows.push(json!({
            "user_id":id,"sort_order":order,"display_name":name,"creator_id":creator,
            "session_hex":encode_hex(&session),"credential_revision":revision,
            "validated_at":"positive-observed-unix-or-seed",
            "created_at":"within-observed-unix-interval",
            "updated_at":"within-observed-unix-interval","creation_preserved":preserved,
        }));
    }
    state
}

#[cfg(unix)]
fn mode(metadata: &std::fs::Metadata) -> String {
    use std::os::unix::fs::PermissionsExt;
    let mut text = if metadata.is_dir() { "d" } else { "-" }.to_owned();
    let value = metadata.permissions().mode();
    for (mask, letter) in [
        (0o400, 'r'),
        (0o200, 'w'),
        (0o100, 'x'),
        (0o040, 'r'),
        (0o020, 'w'),
        (0o010, 'x'),
        (0o004, 'r'),
        (0o002, 'w'),
        (0o001, 'x'),
    ] {
        text.push(if value & mask != 0 { letter } else { '-' });
    }
    text
}
#[cfg(not(unix))]
fn mode(metadata: &std::fs::Metadata) -> String {
    format!(
        "{}readonly={}",
        if metadata.is_dir() {
            "directory/"
        } else {
            "file/"
        },
        metadata.permissions().readonly()
    )
}
#[cfg(unix)]
fn private_mode(path: &Path, directory: bool) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(
        path,
        std::fs::Permissions::from_mode(if directory { 0o700 } else { 0o600 }),
    )
    .unwrap();
}
#[cfg(not(unix))]
fn private_mode(_: &Path, _: bool) {}
