use super::Database;
use crate::{
    fanbox_account::{Account, Repository},
    lifecycle::Context,
    scheduler::SchedulerError,
};
use rusqlite::{OptionalExtension, Row};
use std::{fmt, sync::Mutex};

const COLUMNS: &str = "user_id,sort_order,display_name,creator_id,session_id,credential_revision,validated_at,created_at,updated_at";

#[derive(Debug)]
pub enum FanboxAccountError {
    NotFound,
    Storage(rusqlite::Error),
}
impl fmt::Display for FanboxAccountError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("fanbox account not found"),
            Self::Storage(error) => fmt::Display::fmt(error, f),
        }
    }
}
impl std::error::Error for FanboxAccountError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Storage(error) => Some(error),
            Self::NotFound => None,
        }
    }
}
impl From<rusqlite::Error> for FanboxAccountError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Storage(error)
    }
}

impl Database {
    pub fn list_fanbox(&self) -> Result<Vec<Account>, FanboxAccountError> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT {COLUMNS} FROM fanbox_account ORDER BY sort_order"
        ))?;
        let rows = statement.query_map([], scan)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
    pub fn get_fanbox(&self, user_id: i64) -> Result<Account, FanboxAccountError> {
        self.connection
            .query_row(
                &format!("SELECT {COLUMNS} FROM fanbox_account WHERE user_id=?1"),
                [user_id],
                scan,
            )
            .optional()?
            .ok_or(FanboxAccountError::NotFound)
    }
}
fn scan(row: &Row<'_>) -> rusqlite::Result<Account> {
    let session: Vec<u8> = row.get(4)?;
    let mut account = Account::new(
        row.get(0)?,
        row.get::<_, String>(2)?,
        row.get::<_, String>(3)?,
        &session,
    );
    account.sort_order = row.get(1)?;
    account.credential_revision = row.get(5)?;
    account.validated_at = row.get(6)?;
    account.created_at = row.get(7)?;
    account.updated_at = row.get(8)?;
    Ok(account)
}
impl Repository for Mutex<Database> {
    fn list(&self, context: &Context) -> Result<Vec<Account>, SchedulerError> {
        if let Some(error) = context.error() {
            return Err(error.into());
        }
        self.lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .list_fanbox()
            .map_err(|error| SchedulerError::Message(error.to_string()))
    }
    fn get(&self, context: &Context, user_id: i64) -> Result<Account, SchedulerError> {
        if let Some(error) = context.error() {
            return Err(error.into());
        }
        self.lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get_fanbox(user_id)
            .map_err(|error| SchedulerError::Message(error.to_string()))
    }
}
