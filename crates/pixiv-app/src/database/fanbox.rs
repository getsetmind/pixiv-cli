use super::Database;
use crate::{
    fanbox_account::{Account, Repository},
    lifecycle::{Context, ContextError},
    scheduler::SchedulerError,
};
use chrono::Utc;
use rusqlite::{DropBehavior, OptionalExtension, Row, params, types::ValueRef};
use std::{fmt, sync::Mutex};

const COLUMNS: &str = "user_id,sort_order,display_name,creator_id,session_id,credential_revision,validated_at,created_at,updated_at";

#[derive(Debug)]
pub enum FanboxAccountError {
    NotFound,
    CredentialConflict,
    InvalidAccount,
    InvalidRotation,
    Context(ContextError),
    Storage(rusqlite::Error),
}
impl fmt::Display for FanboxAccountError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("fanbox account not found"),
            Self::CredentialConflict => f.write_str("fanbox account credential revision conflict"),
            Self::InvalidAccount => f.write_str("database: invalid fanbox account"),
            Self::InvalidRotation => f.write_str("database: invalid fanbox rotation input"),
            Self::Context(error) => fmt::Display::fmt(error, f),
            Self::Storage(rusqlite::Error::SqliteFailure(error, message)) => {
                let description = rusqlite::ffi::code_to_str(error.extended_code);
                f.write_str(description)?;
                if let Some(message) = message.as_deref().filter(|message| *message != description)
                {
                    write!(f, ": {message}")?;
                }
                write!(f, " ({})", error.extended_code)
            }
            Self::Storage(error) => fmt::Display::fmt(error, f),
        }
    }
}
impl std::error::Error for FanboxAccountError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Storage(error) => Some(error),
            Self::Context(error) => Some(error),
            _ => None,
        }
    }
}
impl From<rusqlite::Error> for FanboxAccountError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Storage(error)
    }
}
impl From<ContextError> for FanboxAccountError {
    fn from(error: ContextError) -> Self {
        Self::Context(error)
    }
}
fn check_context(context: &Context) -> Result<(), FanboxAccountError> {
    context.error().map_or(Ok(()), |error| Err(error.into()))
}

impl Database {
    pub fn save_fanbox_credential(
        &mut self,
        context: &Context,
        account: &Account,
    ) -> Result<(), FanboxAccountError> {
        if account.user_id <= 0 || !account.has_session() || account.validated_at <= 0 {
            return Err(FanboxAccountError::InvalidAccount);
        }
        let now = Utc::now().timestamp();
        check_context(context)?;
        let mut transaction = self.connection.transaction()?;
        check_context(context)?;
        let existing: Option<i64> = transaction
            .query_row(
                "SELECT sort_order FROM fanbox_account WHERE user_id=?1",
                [account.user_id],
                |row| scan_integer(row, 0),
            )
            .optional()?;
        check_context(context)?;
        if existing.is_some() {
            transaction.execute("UPDATE fanbox_account SET display_name=?1,creator_id=?2,session_id=?3,credential_revision=credential_revision+1,validated_at=?4,updated_at=?5 WHERE user_id=?6", params![account.display_name,account.creator_id,account.session_id_copy(),account.validated_at,now,account.user_id])?;
        } else {
            let order = if account.sort_order > 0 {
                account.sort_order
            } else {
                transaction.query_row(
                    "SELECT COALESCE(MAX(sort_order),0)+1 FROM fanbox_account",
                    [],
                    |row| scan_integer(row, 0),
                )?
            };
            check_context(context)?;
            transaction.execute(
                &format!(
                    "INSERT INTO fanbox_account ({COLUMNS}) VALUES(?1,?2,?3,?4,?5,1,?6,?7,?7)"
                ),
                params![
                    account.user_id,
                    order,
                    account.display_name,
                    account.creator_id,
                    account.session_id_copy(),
                    account.validated_at,
                    now
                ],
            )?;
        }
        check_context(context)?;
        // The frozen Go driver leaves failed COMMIT transactions open until the connection closes.
        transaction.set_drop_behavior(DropBehavior::Ignore);
        transaction.commit()?;
        Ok(())
    }

    pub fn rotate_fanbox_session(
        &self,
        context: &Context,
        user_id: i64,
        expected_revision: i64,
        session: &[u8],
        validated_at: i64,
    ) -> Result<(), FanboxAccountError> {
        if user_id <= 0 || expected_revision <= 0 || session.is_empty() || validated_at <= 0 {
            return Err(FanboxAccountError::InvalidRotation);
        }
        check_context(context)?;
        let changed = self.connection.execute("UPDATE fanbox_account SET session_id=?1,credential_revision=credential_revision+1,validated_at=?2,updated_at=?3 WHERE user_id=?4 AND credential_revision=?5", params![session,validated_at,Utc::now().timestamp(),user_id,expected_revision])?;
        if changed == 0 {
            check_context(context)?;
            let exists: Option<i64> = self
                .connection
                .query_row(
                    "SELECT 1 FROM fanbox_account WHERE user_id=?1",
                    [user_id],
                    |row| row.get(0),
                )
                .optional()?;
            return Err(if exists.is_some() {
                FanboxAccountError::CredentialConflict
            } else {
                FanboxAccountError::NotFound
            });
        }
        Ok(())
    }

    pub fn remove_fanbox(&self, context: &Context, user_id: i64) -> Result<(), FanboxAccountError> {
        check_context(context)?;
        if self
            .connection
            .execute("DELETE FROM fanbox_account WHERE user_id=?1", [user_id])?
            == 0
        {
            return Err(FanboxAccountError::NotFound);
        }
        Ok(())
    }

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
fn column_type_error(row: &Row<'_>, index: usize, value: ValueRef<'_>) -> rusqlite::Error {
    rusqlite::Error::InvalidColumnType(
        index,
        row.as_ref().column_name(index).unwrap_or_default().into(),
        value.data_type(),
    )
}
fn float_text(value: f64) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value.is_infinite() {
        return if value.is_sign_negative() {
            "-Inf"
        } else {
            "+Inf"
        }
        .into();
    }
    let scientific = format!("{value:e}");
    let Some((mantissa, exponent)) = scientific.split_once('e') else {
        return value.to_string();
    };
    let exponent: i32 = exponent.parse().unwrap_or_default();
    if !(-4..6).contains(&exponent) {
        format!("{mantissa}e{exponent:+03}")
    } else {
        value.to_string()
    }
}
fn numeric_text(value: ValueRef<'_>) -> Option<String> {
    match value {
        ValueRef::Integer(value) => Some(value.to_string()),
        ValueRef::Real(value) => Some(float_text(value)),
        _ => None,
    }
}
fn scan_integer(row: &Row<'_>, index: usize) -> rusqlite::Result<i64> {
    let value = row.get_ref(index)?;
    if let ValueRef::Integer(value) = value {
        return Ok(value);
    }
    let text = match value {
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => {
            std::str::from_utf8(bytes).ok().map(str::to_owned)
        }
        _ => numeric_text(value),
    };
    text.and_then(|text| text.parse().ok())
        .ok_or_else(|| column_type_error(row, index, value))
}
fn scan_string(row: &Row<'_>, index: usize) -> rusqlite::Result<String> {
    let value = row.get_ref(index)?;
    match value {
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => {
            String::from_utf8(bytes.to_vec()).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(index, value.data_type(), Box::new(error))
            })
        }
        _ => numeric_text(value).ok_or_else(|| column_type_error(row, index, value)),
    }
}
fn scan_session(row: &Row<'_>, index: usize) -> rusqlite::Result<Vec<u8>> {
    let value = row.get_ref(index)?;
    match value {
        ValueRef::Null => Ok(Vec::new()),
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => Ok(bytes.to_vec()),
        _ => numeric_text(value)
            .map(String::into_bytes)
            .ok_or_else(|| column_type_error(row, index, value)),
    }
}
fn scan(row: &Row<'_>) -> rusqlite::Result<Account> {
    let user_id = scan_integer(row, 0)?;
    let sort_order = scan_integer(row, 1)?;
    let display_name = scan_string(row, 2)?;
    let creator_id = scan_string(row, 3)?;
    let session = scan_session(row, 4)?;
    let mut account = Account::new(user_id, display_name, creator_id, &session);
    account.sort_order = sort_order;
    account.credential_revision = scan_integer(row, 5)?;
    account.validated_at = scan_integer(row, 6)?;
    account.created_at = scan_integer(row, 7)?;
    account.updated_at = scan_integer(row, 8)?;
    Ok(account)
}
impl Repository for Mutex<Database> {
    fn list(&self, context: &Context) -> Result<Vec<Account>, SchedulerError> {
        check_context(context)?;
        let database = self.lock().unwrap_or_else(|poison| poison.into_inner());
        check_context(context)?;
        database.list_fanbox().map_err(Into::into)
    }
    fn get(&self, context: &Context, user_id: i64) -> Result<Account, SchedulerError> {
        check_context(context)?;
        let database = self.lock().unwrap_or_else(|poison| poison.into_inner());
        check_context(context)?;
        database.get_fanbox(user_id).map_err(Into::into)
    }
    fn save_credential(&self, context: &Context, account: &Account) -> Result<(), SchedulerError> {
        self.lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .save_fanbox_credential(context, account)
            .map_err(Into::into)
    }
    fn remove(&self, context: &Context, user_id: i64) -> Result<(), SchedulerError> {
        self.lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove_fanbox(context, user_id)
            .map_err(Into::into)
    }
    fn rotate_session(
        &self,
        context: &Context,
        user_id: i64,
        expected_revision: i64,
        session: &[u8],
        validated_at: i64,
    ) -> Result<(), SchedulerError> {
        self.lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .rotate_fanbox_session(context, user_id, expected_revision, session, validated_at)
            .map_err(Into::into)
    }
}
