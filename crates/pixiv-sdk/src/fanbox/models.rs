use serde::Serialize;
use std::fmt;

#[derive(Clone, Default, Serialize)]
pub struct SessionCredentials {
    #[serde(skip)]
    pub fanbox_sessid: String,
}
impl fmt::Display for SessionCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("fanbox.SessionCredentials{}")
    }
}
impl fmt::Debug for SessionCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct User {
    pub user_id: i64,
    pub display_name: String,
    pub creator_id: String,
    pub creator_status: String,
    pub is_creator: bool,
}
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct UserDto {
    pub user_id: i64,
    pub display_name: String,
    pub creator_id: String,
    pub creator_status: String,
    pub is_creator: bool,
}
impl User {
    pub fn to_dto(&self) -> UserDto {
        UserDto {
            user_id: self.user_id,
            display_name: self.display_name.clone(),
            creator_id: self.creator_id.clone(),
            creator_status: self.creator_status.clone(),
            is_creator: self.is_creator,
        }
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct CurrentUserRequest {}
