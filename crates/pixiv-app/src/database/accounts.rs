use super::Database;
use chrono::Utc;
use rusqlite::{OptionalExtension, Row, Transaction, params};
use std::{collections::BTreeSet, fmt};

pub(super) const COLUMNS: &str = "user_id,sort_order,username,refresh_token,credential_revision,premium_status,premium_checked_at,pool_frozen_until,pool_last_selected,created_at,updated_at,schedulable";

#[derive(Clone)]
pub struct PixivAccount {
    pub user_id: i64,
    pub sort_order: i64,
    pub username: String,
    pub credential_revision: i64,
    pub premium_status: Option<bool>,
    pub premium_checked_at: Option<i64>,
    pub pool_frozen_until: Option<i64>,
    pub pool_last_selected: bool,
    pub created_at: i64,
    pub updated_at: i64,
    pub schedulable: bool,
    refresh_token: Vec<u8>,
}

impl PixivAccount {
    pub fn new(user_id: i64, username: impl Into<String>, refresh_token: &[u8]) -> Self {
        Self {
            user_id,
            sort_order: 0,
            username: username.into(),
            credential_revision: 0,
            premium_status: None,
            premium_checked_at: None,
            pool_frozen_until: None,
            pool_last_selected: false,
            created_at: 0,
            updated_at: 0,
            schedulable: false,
            refresh_token: refresh_token.to_vec(),
        }
    }
    pub fn refresh_token_copy(&self) -> Vec<u8> {
        self.refresh_token.clone()
    }
    pub fn has_refresh_token(&self) -> bool {
        !self.refresh_token.is_empty()
    }
}

impl fmt::Debug for PixivAccount {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PixivAccount")
            .field("user_id", &self.user_id)
            .field("username", &self.username)
            .field("credential_revision", &self.credential_revision)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub enum AccountError {
    NotFound,
    CredentialConflict,
    InvalidAccount,
    DuplicateAccount(i64),
    InvalidMetadata,
    InvalidRotation,
    Storage(rusqlite::Error),
}

impl fmt::Display for AccountError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => formatter.write_str("pixiv account not found"),
            Self::CredentialConflict => {
                formatter.write_str("pixiv account credential revision conflict")
            }
            Self::InvalidAccount => formatter.write_str("database: invalid pixiv account"),
            Self::DuplicateAccount(id) => {
                write!(formatter, "database: duplicate pixiv account {id}")
            }
            Self::InvalidMetadata => formatter.write_str("database: invalid pixiv metadata input"),
            Self::InvalidRotation => formatter.write_str("database: invalid rotation input"),
            Self::Storage(rusqlite::Error::SqliteFailure(error, message)) => {
                let description = rusqlite::ffi::code_to_str(error.extended_code);
                formatter.write_str(description)?;
                if let Some(message) = message.as_deref().filter(|message| *message != description)
                {
                    write!(formatter, ": {message}")?;
                }
                write!(formatter, " ({})", error.extended_code)?;
                if error.extended_code == rusqlite::ffi::SQLITE_BUSY {
                    formatter.write_str(" (SQLITE_BUSY)")?;
                }
                Ok(())
            }
            Self::Storage(error) => fmt::Display::fmt(error, formatter),
        }
    }
}
impl std::error::Error for AccountError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Storage(error) => Some(error),
            _ => None,
        }
    }
}
impl From<rusqlite::Error> for AccountError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Storage(error)
    }
}

impl Database {
    pub fn save_pixiv_credential(&mut self, account: &PixivAccount) -> Result<(), AccountError> {
        self.save_pixiv_credentials(std::slice::from_ref(account))
    }

    pub fn save_pixiv_credentials(
        &mut self,
        accounts: &[PixivAccount],
    ) -> Result<(), AccountError> {
        let mut seen = BTreeSet::new();
        for account in accounts {
            if account.user_id <= 0 || !account.has_refresh_token() {
                return Err(AccountError::InvalidAccount);
            }
            if !seen.insert(account.user_id) {
                return Err(AccountError::DuplicateAccount(account.user_id));
            }
        }
        let transaction = self.connection.transaction()?;
        for account in accounts {
            save(&transaction, account)?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn update_pixiv_metadata(
        &self,
        user_id: i64,
        username: &str,
        premium_status: Option<bool>,
        premium_checked_at: Option<i64>,
    ) -> Result<(), AccountError> {
        if user_id <= 0 {
            return Err(AccountError::InvalidMetadata);
        }
        let changed = self.connection.execute("UPDATE pixiv_account SET username=?1,premium_status=?2,premium_checked_at=?3,updated_at=?4 WHERE user_id=?5", params![username,premium_status,premium_checked_at,Utc::now().timestamp(),user_id])?;
        if changed == 0 {
            return Err(AccountError::NotFound);
        }
        Ok(())
    }

    pub fn rotate_pixiv_credentials(
        &self,
        user_id: i64,
        expected_revision: i64,
        refresh_token: &[u8],
    ) -> Result<(), AccountError> {
        if user_id <= 0 || expected_revision <= 0 || refresh_token.is_empty() {
            return Err(AccountError::InvalidRotation);
        }
        let changed = self.connection.execute("UPDATE pixiv_account SET refresh_token=?1,credential_revision=credential_revision+1,updated_at=?2 WHERE user_id=?3 AND credential_revision=?4", params![refresh_token,Utc::now().timestamp(),user_id,expected_revision])?;
        if changed == 0 {
            let exists: bool = self.connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM pixiv_account WHERE user_id=?1)",
                [user_id],
                |row| row.get(0),
            )?;
            return Err(if exists {
                AccountError::CredentialConflict
            } else {
                AccountError::NotFound
            });
        }
        Ok(())
    }

    pub fn list_pixiv(&self) -> Result<Vec<PixivAccount>, AccountError> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT {COLUMNS} FROM pixiv_account ORDER BY sort_order"
        ))?;
        let accounts = statement
            .query_map([], scan)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(accounts)
    }

    pub fn get_pixiv(&self, user_id: i64) -> Result<PixivAccount, AccountError> {
        self.connection
            .query_row(
                &format!("SELECT {COLUMNS} FROM pixiv_account WHERE user_id=?1"),
                [user_id],
                scan,
            )
            .optional()?
            .ok_or(AccountError::NotFound)
    }

    pub fn remove_pixiv(&self, user_id: i64) -> Result<(), AccountError> {
        if self
            .connection
            .execute("DELETE FROM pixiv_account WHERE user_id=?1", [user_id])?
            == 0
        {
            return Err(AccountError::NotFound);
        }
        Ok(())
    }
}

fn save(transaction: &Transaction<'_>, account: &PixivAccount) -> Result<(), AccountError> {
    let now = Utc::now().timestamp();
    let existing: Option<i64> = transaction
        .query_row(
            "SELECT sort_order FROM pixiv_account WHERE user_id=?1",
            [account.user_id],
            |row| row.get(0),
        )
        .optional()?;
    if existing.is_some() {
        transaction.execute("UPDATE pixiv_account SET username=?1,refresh_token=?2,credential_revision=credential_revision+1,updated_at=?3 WHERE user_id=?4", params![account.username,account.refresh_token,now,account.user_id])?;
    } else {
        let order = if account.sort_order > 0 {
            account.sort_order
        } else {
            transaction.query_row(
                "SELECT COALESCE(MAX(sort_order),0)+1 FROM pixiv_account",
                [],
                |row| row.get(0),
            )?
        };
        transaction.execute(
            &format!(
                "INSERT INTO pixiv_account ({COLUMNS}) VALUES(?1,?2,?3,?4,1,?5,?6,?7,?8,?9,?9,1)"
            ),
            params![
                account.user_id,
                order,
                account.username,
                account.refresh_token,
                account.premium_status,
                account.premium_checked_at,
                account.pool_frozen_until,
                account.pool_last_selected,
                now
            ],
        )?;
    }
    Ok(())
}

pub(super) fn scan(row: &Row<'_>) -> rusqlite::Result<PixivAccount> {
    Ok(PixivAccount {
        user_id: row.get(0)?,
        sort_order: row.get(1)?,
        username: row.get(2)?,
        refresh_token: row.get(3)?,
        credential_revision: row.get(4)?,
        premium_status: row.get::<_, Option<i64>>(5)?.map(|value| value == 1),
        premium_checked_at: row.get(6)?,
        pool_frozen_until: row.get(7)?,
        pool_last_selected: row.get::<_, i64>(8)? == 1,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
        schedulable: row.get::<_, i64>(11)? == 1,
    })
}
