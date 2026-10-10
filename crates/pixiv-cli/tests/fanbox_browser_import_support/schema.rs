use crate::fanbox_auth_support::schema::{Case as AuthCase, Step};
use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cookie {
    pub host: String,
    pub name: String,
    pub value_hex: String,
    pub encrypted_hex: String,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub id: String,
    pub path: String,
    pub mode: String,
    pub cookies: Option<Vec<Cookie>>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct File {
    pub path: String,
    pub mode: String,
    pub bytes_hex: String,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    pub name: String,
    pub browser: String,
    #[serde(default)]
    pub xdg_empty: bool,
    pub profiles: Vec<Profile>,
    pub firefox_ini: Option<String>,
    pub files: Vec<File>,
    pub secret_mode: String,
    pub secret_hex: String,
    #[serde(default)]
    pub sqlite_unavailable: bool,
    pub config_before: Option<String>,
    pub seed: Vec<i64>,
    pub steps: Vec<Step>,
    pub discovery: Value,
    pub observations: Vec<Value>,
}
impl Case {
    pub fn auth_case(&self) -> AuthCase {
        AuthCase {
            name: self.name.clone(),
            config_before: self.config_before.clone(),
            seed: self.seed.clone(),
            steps: self.steps.clone(),
            observations: Vec::new(),
        }
    }
}
