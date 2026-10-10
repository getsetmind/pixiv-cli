use crate::{lifecycle::Context, scheduler::SchedulerError};
use serde::Serialize;
use std::fmt;

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct Account {
    #[serde(rename = "UserID")]
    pub user_id: i64,
    pub sort_order: i64,
    pub display_name: String,
    #[serde(rename = "CreatorID")]
    pub creator_id: String,
    pub credential_revision: i64,
    pub validated_at: i64,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(skip)]
    session_id: Vec<u8>,
}
impl Account {
    pub fn new(
        user_id: i64,
        display_name: impl Into<String>,
        creator_id: impl Into<String>,
        session_id: &[u8],
    ) -> Self {
        Self {
            user_id,
            display_name: display_name.into(),
            creator_id: creator_id.into(),
            session_id: session_id.to_vec(),
            ..Self::default()
        }
    }
    pub fn session_id_copy(&self) -> Vec<u8> {
        self.session_id.clone()
    }
    pub fn has_session(&self) -> bool {
        !self.session_id.is_empty()
    }
}
impl fmt::Display for Account {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "fanbox.Account{{user_id:{} display_name:{:?} credential_revision:{}}}",
            self.user_id, self.display_name, self.credential_revision
        )
    }
}
impl fmt::Debug for Account {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

pub trait Repository: Send + Sync {
    fn save_credential(&self, context: &Context, account: &Account) -> Result<(), SchedulerError>;
    fn rotate_session(
        &self,
        context: &Context,
        user_id: i64,
        expected_revision: i64,
        session: &[u8],
        validated_at: i64,
    ) -> Result<(), SchedulerError>;
    fn remove(&self, context: &Context, user_id: i64) -> Result<(), SchedulerError>;
    fn list(&self, context: &Context) -> Result<Vec<Account>, SchedulerError>;
    fn get(&self, context: &Context, user_id: i64) -> Result<Account, SchedulerError>;
}
pub trait DefaultStore: Send + Sync {
    fn set(&self, user_id: i64) -> Result<(), SchedulerError>;
    fn clear(&self) -> Result<(), SchedulerError>;
    fn read(&self) -> Result<Option<i64>, SchedulerError>;
}
