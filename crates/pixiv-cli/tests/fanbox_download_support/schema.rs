use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub args: Vec<String>,
    pub config: String,
    pub saved: String,
    #[serde(default)]
    pub db_failure: String,
    #[serde(default)]
    pub stdin: String,
    #[serde(default)]
    pub stdin_error: bool,
    pub replies: Vec<Reply>,
    pub output_root: String,
    pub seed_files: Vec<FileState>,
    #[serde(default)]
    pub writer: String,
    #[serde(default)]
    pub write_limit: usize,
    #[serde(default)]
    pub close_error: bool,
    #[serde(default)]
    pub root_close_error: bool,
    #[serde(default)]
    pub options_error: bool,
    #[serde(default)]
    pub factory: String,
    #[serde(default)]
    pub open: String,
    #[serde(default)]
    pub startup_error: bool,
    #[serde(default)]
    pub cancel: String,
    #[serde(default)]
    pub repeat: usize,
}
#[derive(Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub status: u16,
    #[serde(default)]
    pub transport_error: String,
    #[serde(default)]
    pub read_error: String,
    #[serde(default)]
    pub close_error: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub content_type: String,
    #[serde(default)]
    pub content_length: i64,
    #[serde(default)]
    pub chunk_size: usize,
    #[serde(default)]
    pub bytes_and_eof: bool,
    #[serde(default)]
    pub bytes_and_error: bool,
    #[serde(default)]
    pub cancel_at_read: usize,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileState {
    pub path: String,
    pub kind: String,
    pub mode: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bytes_hex: String,
    pub size: usize,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sha256: String,
}
#[derive(Default, Serialize)]
pub struct Observation {
    pub exits: Vec<i32>,
    pub stdout: String,
    pub stderr: String,
    pub errors: Vec<String>,
    pub reasons: Vec<String>,
    pub trace: Vec<String>,
    pub requests: Vec<Value>,
    pub options: Vec<Value>,
    pub proxy_overrides: Vec<Option<String>>,
    pub stdin_reads: usize,
    pub writes: Vec<usize>,
    pub body_closes: usize,
    pub lease_closes: usize,
    pub unique_clients: usize,
    pub output_is_tty: bool,
    pub auto_ndjson: bool,
    pub idle_closes: usize,
    pub db_before: String,
    pub db_after: String,
    pub db_rows_before: Vec<String>,
    pub db_rows_after: Vec<String>,
    pub config_after: String,
    pub remaining_temps: Vec<String>,
    pub socket_denied: bool,
    pub exec_denied: bool,
    pub files_before: Vec<FileState>,
    pub files_after: Vec<FileState>,
    pub body_reads: Vec<Value>,
    pub output_writes: Vec<Value>,
    pub reply_count: usize,
}
