use rusqlite::{Connection, params};
mod accounts;
pub use accounts::{AccountError, PixivAccount};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fmt, fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const APPLICATION_ID: i64 = 0x50495843;
const LEGACY_INITIAL_CHECKSUM: &str =
    "0247f7ea8739433ce47074048a1c8707728e7f3d04cf47c3ff6f15282f8e641f";
const MIGRATIONS: [&str; 3] = [
    include_str!("../../../internal/storage/database/migrations/0001_initial.sql"),
    include_str!(
        "../../../internal/storage/database/migrations/0002_fanbox_creator_id_not_null.sql"
    ),
    include_str!(
        "../../../internal/storage/database/migrations/0003_pixiv_account_schedulable.sql"
    ),
];
const NAMES: [&str; 3] = [
    "0001_initial",
    "0002_fanbox_creator_id_not_null",
    "0003_pixiv_account_schedulable",
];

#[derive(Debug)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for Error {}
impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Self {
        Self(error.to_string())
    }
}

pub struct Database {
    connection: Connection,
    path: PathBuf,
}

impl Database {
    pub fn open(directory: impl AsRef<Path>) -> Result<Self, Error> {
        let directory = directory.as_ref();
        if directory.as_os_str().to_string_lossy().trim().is_empty() {
            return Err(Error("database: app data directory is required".into()));
        }
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(directory)
            .map_err(|error| Error(format!("database: create app data directory: {error}")))?;
        let path = directory.join("pixiv-cli.db");
        let connection = Connection::open(&path)
            .map_err(|error| Error(format!("database: open database: {error}")))?;
        connection.busy_timeout(std::time::Duration::ZERO)?;
        connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL; PRAGMA secure_delete=ON; PRAGMA trusted_schema=OFF;")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).map_err(|error| {
                Error(format!("database: tighten directory permissions: {error}"))
            })?;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).map_err(|error| {
                Error(format!("database: tighten database permissions: {error}"))
            })?;
        }
        let mut database = Self { connection, path };
        database.initialize()?;
        Ok(database)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn initialize(&mut self) -> Result<(), Error> {
        let id: i64 = self
            .connection
            .query_row("PRAGMA application_id", [], |row| row.get(0))?;
        if id == 0 {
            self.connection
                .pragma_update(None, "application_id", APPLICATION_ID)?;
        } else if id != APPLICATION_ID {
            return Err(Error(format!(
                "database: database belongs to another application (application_id=0x{id:X})"
            )));
        }
        self.migrate()
    }

    fn migrate(&mut self) -> Result<(), Error> {
        let user_version: i64 = self
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))?;
        let has_ledger: bool = self.connection.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='schema_migration'",
            [],
            |row| row.get(0),
        )?;
        let mut applied = BTreeMap::new();
        if has_ledger {
            let mut statement = self
                .connection
                .prepare("SELECT version,name,checksum FROM schema_migration ORDER BY version")?;
            for row in statement.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })? {
                let (version, name, checksum) = row?;
                applied.insert(version, (name, checksum));
            }
        }
        let mut previous = None;
        for version in applied.keys().copied() {
            match previous {
                None if version != 1 => {
                    return Err(Error(format!(
                        "migration version gap: first applied version is {version}, want 1"
                    )));
                }
                Some(before) if version != before + 1 => {
                    return Err(Error(format!(
                        "migration version gap: {version} follows {before}"
                    )));
                }
                _ => previous = Some(version),
            }
        }
        if user_version > MIGRATIONS.len() as i64 {
            return Err(Error(format!(
                "database schema version {user_version} is newer than binary schema version 3; refusing to run a downgrade"
            )));
        }
        for (index, sql) in MIGRATIONS.iter().enumerate() {
            let version = index as i64 + 1;
            let name = NAMES[index];
            if let Some((stored_name, stored_checksum)) = applied.get(&version) {
                if stored_name != name {
                    return Err(Error(format!(
                        "migration {version} name drifted: {stored_name:?} != {name:?}"
                    )));
                }
                let lf = sql.replace("\r\n", "\n");
                if stored_checksum != &checksum(sql)
                    && stored_checksum != &checksum(&lf)
                    && stored_checksum != &checksum(&lf.replace('\n', "\r\n"))
                    && !(version == 1 && stored_checksum == LEGACY_INITIAL_CHECKSUM)
                {
                    return Err(Error(format!("migration {version} checksum drifted")));
                }
                continue;
            }
            let satisfied = match version {
                2 => self
                    .column("fanbox_account", "creator_id")?
                    .is_some_and(|not_null| not_null),
                3 => self.column("pixiv_account", "schedulable")?.is_some(),
                _ => false,
            };
            let action = if satisfied { "record" } else { "apply" };
            self.apply(index, satisfied).map_err(|error| {
                Error(format!("{action} migration {version} ({name}): {error}"))
            })?;
        }
        self.connection
            .pragma_update(None, "user_version", MIGRATIONS.len() as i64)?;
        Ok(())
    }

    fn column(&self, table: &str, name: &str) -> Result<Option<bool>, Error> {
        let mut statement = self
            .connection
            .prepare("SELECT name,\"notnull\" FROM pragma_table_info(?1)")?;
        for row in statement.query_map([table], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?))
        })? {
            let (column, not_null) = row?;
            if column == name {
                return Ok(Some(not_null));
            }
        }
        Ok(None)
    }

    fn apply(&mut self, index: usize, satisfied: bool) -> Result<(), Error> {
        let transaction = self.connection.transaction()?;
        if !satisfied {
            transaction.execute_batch(MIGRATIONS[index])?;
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| Error(error.to_string()))?
            .as_secs() as i64;
        transaction.execute(
            "INSERT INTO schema_migration(version,name,checksum,applied_at) VALUES(?1,?2,?3,?4)",
            params![
                index as i64 + 1,
                NAMES[index],
                checksum(MIGRATIONS[index]),
                now
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }
}

fn checksum(sql: &str) -> String {
    format!("{:x}", Sha256::digest(sql.as_bytes()))
}
