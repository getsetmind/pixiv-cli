use super::{AccountError, Database, PixivAccount, accounts};
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter, types::Value};
use std::{collections::BTreeSet, fmt};

#[derive(Clone, Debug)]
pub struct PoolCandidate {
    pub user_id: i64,
    pub sort_order: i64,
    pub schedulable: bool,
    pub pool_frozen_until: Option<i64>,
    pub pool_last_selected: bool,
    pub eligible: bool,
}
#[derive(Debug)]
pub struct PoolSnapshot {
    pub candidates: Vec<PoolCandidate>,
    pub marker_user_id: Option<i64>,
    pub marker_sort_order: Option<i64>,
    pub earliest_frozen_until: Option<i64>,
}
#[derive(Debug)]
pub struct PoolStatus {
    pub accounts: Vec<PoolCandidate>,
    pub earliest_frozen_until: Option<i64>,
}

pub type PoolChooser<'a> = dyn FnMut(&PoolSnapshot) -> Result<i64, PoolError> + 'a;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PoolSelectionKind {
    NoLocalAccount,
    NoSchedulableAccount,
    AllFrozen,
    Exhausted,
}
impl PoolSelectionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoLocalAccount => "no_local_account",
            Self::NoSchedulableAccount => "no_schedulable_account",
            Self::AllFrozen => "all_frozen",
            Self::Exhausted => "exhausted",
        }
    }
}
#[derive(Debug)]
pub enum PoolError {
    Selection {
        kind: PoolSelectionKind,
        earliest_frozen_until: Option<i64>,
    },
    Account(AccountError),
    Storage(rusqlite::Error),
    Message(String),
    UnknownSelection(String),
}
impl fmt::Display for PoolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Selection { kind, .. } => {
                write!(f, "pixiv account pool selection failed: {}", kind.as_str())
            }
            Self::Account(error) => fmt::Display::fmt(error, f),
            Self::Storage(error) => fmt::Display::fmt(error, f),
            Self::Message(message) => f.write_str(message),
            Self::UnknownSelection(kind) => {
                write!(f, "pixiv account pool selection failed: {kind}")
            }
        }
    }
}
impl std::error::Error for PoolError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Account(error) => Some(error),
            Self::Storage(error) => Some(error),
            _ => None,
        }
    }
}
impl From<rusqlite::Error> for PoolError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Storage(error)
    }
}

pub fn choose_pool_account(
    snapshot: &PoolSnapshot,
    strategy: &str,
    mut random: Option<&mut dyn FnMut(usize) -> Result<i64, PoolError>>,
) -> Result<i64, PoolError> {
    if snapshot.candidates.is_empty() {
        return Err(PoolError::Selection {
            kind: PoolSelectionKind::Exhausted,
            earliest_frozen_until: snapshot.earliest_frozen_until,
        });
    }
    match strategy {
        "round_robin" => Ok(snapshot
            .candidates
            .iter()
            .find(|candidate| {
                snapshot
                    .marker_sort_order
                    .is_some_and(|marker| candidate.sort_order > marker)
            })
            .unwrap_or(&snapshot.candidates[0])
            .user_id),
        "random" => {
            let size = snapshot.candidates.len();
            let index = match random.as_mut() {
                Some(source) => source(size)?,
                None => random_index(size)?,
            };
            if index < 0 || index as usize >= size {
                return Err(PoolError::Message(
                    "pixiv account pool random source returned an invalid index".into(),
                ));
            }
            Ok(snapshot.candidates[index as usize].user_id)
        }
        _ => Err(PoolError::Message(format!(
            "unsupported account pool strategy {strategy:?}"
        ))),
    }
}

fn random_index(size: usize) -> Result<i64, PoolError> {
    let bound = size as u64;
    let ceiling = u64::MAX - u64::MAX % bound;
    loop {
        let mut bytes = [0; 8];
        getrandom::fill(&mut bytes).map_err(|error| PoolError::Message(error.to_string()))?;
        let value = u64::from_ne_bytes(bytes);
        if value < ceiling {
            return Ok((value % bound) as i64);
        }
    }
}

impl Database {
    pub fn set_pixiv_schedulable(&mut self, ids: &[i64], enabled: bool) -> Result<(), PoolError> {
        if ids.is_empty() {
            return Err(PoolError::Message(
                "database: account pool requires at least one user id".into(),
            ));
        }
        let mut seen = BTreeSet::new();
        for id in ids {
            if *id <= 0 {
                return Err(PoolError::Message(
                    "database: account pool user id must be positive".into(),
                ));
            }
            if !seen.insert(*id) {
                return Err(PoolError::Message(format!(
                    "database: duplicate account pool user id {id}"
                )));
            }
        }
        let transaction = self.connection.transaction()?;
        let placeholders = vec!["?"; ids.len()].join(",");
        let count: i64 = transaction.query_row(
            &format!("SELECT count(*) FROM pixiv_account WHERE user_id IN ({placeholders})"),
            params_from_iter(ids),
            |row| row.get(0),
        )?;
        if count != ids.len() as i64 {
            return Err(PoolError::Account(AccountError::NotFound));
        }
        let mut values = vec![
            Value::Integer(i64::from(enabled)),
            Value::Integer(Utc::now().timestamp()),
        ];
        values.extend(ids.iter().copied().map(Value::Integer));
        transaction.execute(
            &format!("UPDATE pixiv_account SET schedulable=?,updated_at=? WHERE user_id IN ({placeholders})"),
            params_from_iter(values),
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn set_all_pixiv_schedulable(&mut self, enabled: bool) -> Result<(), PoolError> {
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "UPDATE pixiv_account SET schedulable=?1,updated_at=?2",
            params![enabled, Utc::now().timestamp()],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn freeze_pixiv(&self, user_id: i64, until: i64) -> Result<(), PoolError> {
        let changed=self.connection.execute("UPDATE pixiv_account SET pool_frozen_until=CASE WHEN pool_frozen_until IS NULL OR pool_frozen_until<?1 THEN ?1 ELSE pool_frozen_until END,updated_at=?2 WHERE user_id=?3",params![until,Utc::now().timestamp(),user_id])?;
        if changed == 0 {
            return Err(PoolError::Account(AccountError::NotFound));
        }
        Ok(())
    }

    pub fn list_pixiv_pool_status(&mut self, now: i64) -> Result<PoolStatus, PoolError> {
        let transaction = self.connection.transaction()?;
        clear_expired(&transaction, now)?;
        let accounts = candidates(&transaction, now, "", &[])?;
        let earliest_frozen_until = accounts
            .iter()
            .filter_map(|account| account.pool_frozen_until)
            .min();
        transaction.commit()?;
        Ok(PoolStatus {
            accounts,
            earliest_frozen_until,
        })
    }

    pub fn select_pixiv(
        &mut self,
        now: i64,
        attempted: &[i64],
        chooser: Option<&mut PoolChooser<'_>>,
    ) -> Result<PixivAccount, PoolError> {
        let transaction = self.connection.transaction()?;
        clear_expired(&transaction, now)?;
        let failure = |kind, earliest_frozen_until| PoolError::Selection {
            kind,
            earliest_frozen_until,
        };
        if count(&transaction, "", &[])? == 0 {
            return Err(failure(PoolSelectionKind::NoLocalAccount, None));
        }
        if count(&transaction, " WHERE schedulable=1", &[])? == 0 {
            return Err(failure(PoolSelectionKind::NoSchedulableAccount, None));
        }
        let mut scope =
            " WHERE schedulable=1 AND (pool_frozen_until IS NULL OR pool_frozen_until<=?)"
                .to_owned();
        if count(&transaction, &scope, &[now])? == 0 {
            return Err(failure(
                PoolSelectionKind::AllFrozen,
                earliest(&transaction, now)?,
            ));
        }
        let mut arguments = vec![now];
        if !attempted.is_empty() {
            scope.push_str(&format!(
                " AND user_id NOT IN ({})",
                vec!["?"; attempted.len()].join(",")
            ));
            arguments.extend_from_slice(attempted);
        }
        let choices = candidates(&transaction, now, &scope, &arguments)?;
        if choices.is_empty() {
            return Err(failure(
                PoolSelectionKind::Exhausted,
                earliest(&transaction, now)?,
            ));
        }
        let marker: Option<(i64, i64)> = transaction
            .query_row(
                "SELECT user_id,sort_order FROM pixiv_account WHERE pool_last_selected=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let snapshot = PoolSnapshot {
            candidates: choices,
            marker_user_id: marker.map(|(id, _)| id),
            marker_sort_order: marker.map(|(_, order)| order),
            earliest_frozen_until: None,
        };
        let chooser = chooser
            .ok_or_else(|| PoolError::Message("database: pixiv pool chooser is required".into()))?;
        let chosen = chooser(&snapshot)?;
        if !snapshot
            .candidates
            .iter()
            .any(|account| account.user_id == chosen)
        {
            return Err(PoolError::Message(format!(
                "database: pixiv pool chooser selected uid {chosen} outside transaction snapshot"
            )));
        }
        let mut selected = transaction.query_row(
            &format!(
                "SELECT {} FROM pixiv_account WHERE user_id=?1",
                accounts::COLUMNS
            ),
            [chosen],
            accounts::scan,
        )?;
        transaction.execute(
            "UPDATE pixiv_account SET pool_last_selected=0 WHERE pool_last_selected=1",
            [],
        )?;
        transaction.execute(
            "UPDATE pixiv_account SET pool_last_selected=1,updated_at=?1 WHERE user_id=?2",
            params![now, chosen],
        )?;
        transaction.commit()?;
        selected.pool_last_selected = true;
        Ok(selected)
    }
}

fn clear_expired(connection: &Connection, now: i64) -> rusqlite::Result<()> {
    connection.execute("UPDATE pixiv_account SET pool_frozen_until=NULL WHERE pool_frozen_until IS NOT NULL AND pool_frozen_until<=?1",[now])?;
    Ok(())
}

fn candidates(
    connection: &Connection,
    now: i64,
    scope: &str,
    arguments: &[i64],
) -> rusqlite::Result<Vec<PoolCandidate>> {
    let mut statement=connection.prepare(&format!("SELECT user_id,sort_order,schedulable,pool_frozen_until,pool_last_selected FROM pixiv_account{scope} ORDER BY sort_order"))?;
    statement
        .query_map(params_from_iter(arguments), |row| {
            let schedulable = row.get::<_, i64>(2)? == 1;
            let frozen: Option<i64> = row.get(3)?;
            Ok(PoolCandidate {
                user_id: row.get(0)?,
                sort_order: row.get(1)?,
                schedulable,
                pool_frozen_until: frozen,
                pool_last_selected: row.get::<_, i64>(4)? == 1,
                eligible: schedulable && frozen.is_none_or(|until| until <= now),
            })
        })?
        .collect()
}

fn count(connection: &Connection, scope: &str, arguments: &[i64]) -> rusqlite::Result<i64> {
    connection.query_row(
        &format!("SELECT count(*) FROM pixiv_account{scope}"),
        params_from_iter(arguments),
        |row| row.get(0),
    )
}

fn earliest(connection: &Connection, now: i64) -> rusqlite::Result<Option<i64>> {
    connection.query_row("SELECT MIN(pool_frozen_until) FROM pixiv_account WHERE schedulable=1 AND pool_frozen_until>?1", [now], |row| row.get(0))
}
