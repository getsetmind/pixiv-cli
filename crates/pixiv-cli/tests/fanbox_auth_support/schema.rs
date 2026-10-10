use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    pub name: String,
    pub config_before: Option<String>,
    pub seed: Vec<i64>,
    pub steps: Vec<Step>,
    pub observations: Vec<Observation>,
}

#[derive(Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Step {
    pub args: Vec<String>,
    pub input_hex: String,
    pub input_error: bool,
    pub can_prompt: bool,
    pub prompt_value: String,
    pub prompt_error: bool,
    pub confirm: bool,
    pub browser_value: String,
    pub browser_error: bool,
    pub system_browser_mode: String,
    pub factory_error: bool,
    pub factory_nil: bool,
    pub startup_error: bool,
    pub startup_relay: bool,
    pub close_error: bool,
    pub writer: String,
    pub stderr_writer: String,
    pub repository_failure: String,
    pub default_failure: String,
    pub default_fail_nth: usize,
    pub file_failure: String,
    pub identity: String,
    pub user_id: i64,
    pub display_name: String,
    pub creator_id: String,
    pub empty_identity_fields: bool,
    pub canceled: bool,
    pub cancel_on_request: bool,
    pub inject_session: bool,
    pub options_error: bool,
    pub release: bool,
    pub update_mode: String,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WriteAttempt {
    pub bytes: String,
    pub n: usize,
    pub error: String,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct State {
    pub config: Option<String>,
    pub database: bool,
    pub user_version: i64,
    pub application_id: i64,
    pub rows: Vec<Value>,
    pub modes: BTreeMap<String, String>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub exit: i32,
    pub stdout: String,
    pub stderr: String,
    pub output_writes: Vec<WriteAttempt>,
    pub error_writes: Vec<WriteAttempt>,
    pub trace: Vec<String>,
    pub requests: Vec<Value>,
    pub stdin_reads: usize,
    pub stdin_bytes: usize,
    pub before: State,
    pub after: State,
    pub socket_denied: bool,
    pub exec_denied: bool,
}
