pub mod aggregate;
pub mod facade;
pub mod source;

use pixiv_app::reverse_search::{CallerContext, Error};
use pixiv_sdk::context::{Context, ContextError};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Deserialize)]
pub struct Row {
    pub name: String,
    pub operation: String,
    pub input: Value,
    pub output: Value,
}
#[derive(Deserialize)]
pub struct Fixture {
    pub reference: String,
    pub environment: String,
    pub cases: Vec<Row>,
    pub boundaries: Value,
}
pub fn fixture() -> Fixture {
    serde_json::from_str(include_str!(
        "../../../pixiv-cli/tests/fixtures/reverse-search-core.json"
    ))
    .unwrap()
}
pub fn string<'a>(input: &'a Value, key: &str) -> &'a str {
    input[key].as_str().unwrap_or("")
}
pub fn boolean(input: &Value, key: &str) -> bool {
    input[key].as_bool().unwrap_or(false)
}
pub fn bytes(hex: &str) -> Vec<u8> {
    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
pub fn context(mode: &str) -> (Arc<Context>, CallerContext) {
    assert_ne!(
        mode, "nil",
        "Go nil context cannot be supplied to the Rust non-null context port"
    );
    let context = Arc::new(if mode == "deadline" {
        Context::with_deadline(Instant::now() - Duration::from_secs(1))
    } else {
        Context::new()
    });
    if mode == "canceled" {
        context.cancel();
    }
    (context.clone(), context)
}
pub fn error(error: Option<&Error>) -> Value {
    match error {
        None => json!({"message":"","code":"","canceled":false,"deadline":false}),
        Some(error) => {
            json!({"message":error.to_string(),"code":error.code().as_str(),"canceled":error.contains_context(ContextError::Canceled),"deadline":error.contains_context(ContextError::DeadlineExceeded)})
        }
    }
}
pub fn external(message: &str) -> Error {
    Error::external(std::io::Error::other(message.to_owned()))
}
pub fn read_snapshot(snapshot: &pixiv_app::reverse_search::Snapshot) -> String {
    let mut bytes = Vec::new();
    snapshot.open().unwrap().read_to_end(&mut bytes).unwrap();
    hex(&bytes)
}
pub fn count_files(path: &Path) -> usize {
    std::fs::read_dir(path).map_or(0, |entries| entries.count())
}
pub struct BlockedSnapshot {
    path: PathBuf,
    backup: PathBuf,
    restored: bool,
}
impl BlockedSnapshot {
    pub fn replace(dir: &Path) -> Self {
        let files: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(files.len(), 1);
        assert!(files[0].is_file());
        let path = files[0].clone();
        let backup = dir.parent().unwrap().join("owned-snapshot-backup.bin");
        std::fs::rename(&path, &backup).unwrap();
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("owned-blocker.bin"), b"owned").unwrap();
        Self {
            path,
            backup,
            restored: false,
        }
    }
    pub fn restore(&mut self) {
        if self.restored {
            return;
        }
        std::fs::remove_file(self.path.join("owned-blocker.bin")).unwrap();
        std::fs::remove_dir(&self.path).unwrap();
        std::fs::rename(&self.backup, &self.path).unwrap();
        self.restored = true;
    }
}
impl Drop for BlockedSnapshot {
    fn drop(&mut self) {
        self.restore();
    }
}
pub fn assert_errno(error: &Error, errno: i32) {
    let mut cause: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(current) = cause {
        if let Some(io) = current.downcast_ref::<std::io::Error>() {
            assert_eq!(io.raw_os_error(), Some(errno));
            return;
        }
        cause = current.source();
    }
    panic!("no native Rust I/O cause retained");
}
