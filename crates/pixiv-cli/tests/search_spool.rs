use chrono::DateTime;
use pixiv_cli_rs::{
    DetailOutput,
    search::{SearchOptions, artwork_search},
};
use pixiv_sdk::{
    Client, Result,
    transport::{Request, Response, Transport},
};
use serde_json::Value;
use std::{
    io::{self, Write},
    path::PathBuf,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

struct Fixture {
    bodies: Vec<Value>,
    directory: PathBuf,
    requests: Arc<AtomicUsize>,
    cancel: bool,
}
impl Transport for Fixture {
    async fn send(&self, request: Request) -> Result<Response> {
        assert_eq!(request.method.as_str(), "GET");
        assert_eq!(request.url, "https://app-api.pixiv.net/v1/search/illust");
        let index = self.requests.fetch_add(1, Ordering::SeqCst);
        let files = std::fs::read_dir(&self.directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(
            files.len(),
            1,
            "a private spool must exist before every fetch"
        );
        let bytes = std::fs::read(&files[0]).unwrap();
        assert!(bytes.starts_with(b"{\n  \"illusts\": ["));
        if index > 0 {
            assert!(bytes.len() > 1000, "previous pages must already be on disk");
        }
        if self.cancel && index == 1 {
            std::future::pending::<()>().await;
        }
        Ok(Response {
            status: 200,
            retry_after: None,
            body: self.bodies[index].clone(),
        })
    }
}
struct Output {
    bytes: Vec<u8>,
    fail: bool,
}
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.fail {
            return Err(io::Error::other("fixture write failed"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn search_json_spool_is_private_page_bounded_and_removed_on_success_failure_and_cancellation() {
    for scenario in ["success", "upstream", "writer", "cancel"] {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("pixiv-spool-test-{}-{stamp}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "search_json_spool_child",
                "--ignored",
                "--nocapture",
            ])
            .env("TEMP", &directory)
            .env("TMP", &directory)
            .env("TMPDIR", &directory)
            .env("PIXIV_SPOOL_SCENARIO", scenario)
            .env("PIXIV_SPOOL_DIRECTORY", &directory)
            .output()
            .unwrap();
        let remaining = std::fs::read_dir(&directory).unwrap().count();
        std::fs::remove_dir(&directory).unwrap();
        assert!(
            output.status.success(),
            "{scenario}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(remaining, 0);
    }
}

#[tokio::test]
#[ignore]
async fn search_json_spool_child() {
    let scenario = std::env::var("PIXIV_SPOOL_SCENARIO").unwrap();
    let directory = PathBuf::from(std::env::var("PIXIV_SPOOL_DIRECTORY").unwrap());
    let source: Value = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/search-pages.json"
    ))
    .unwrap();
    let row = if scenario == "upstream" { 73 } else { 1 };
    let bodies = source[row]["bodies"].as_array().unwrap().clone();
    let requests = Arc::new(AtomicUsize::new(0));
    let client = Client::with_transport(
        "fixture-access",
        Fixture {
            bodies,
            directory: directory.clone(),
            requests: requests.clone(),
            cancel: scenario == "cancel",
        },
    );
    let options = SearchOptions {
        limit: Some(0),
        ..Default::default()
    };
    let request = options
        .request(
            "cat",
            DateTime::parse_from_rfc3339("2024-02-29T12:00:00+09:00").unwrap(),
        )
        .unwrap();
    let mut out = Output {
        bytes: vec![],
        fail: scenario == "writer",
    };
    let result = tokio::time::timeout(
        if scenario == "cancel" {
            std::time::Duration::from_millis(100)
        } else {
            std::time::Duration::from_secs(10)
        },
        artwork_search(&client, request, &options, DetailOutput::Json, &mut out),
    )
    .await;
    match scenario.as_str() {
        "success" => {
            result.unwrap().unwrap();
            assert_eq!(
                serde_json::from_slice::<Value>(&out.bytes).unwrap()["illusts"]
                    .as_array()
                    .unwrap()
                    .len(),
                9
            );
            assert_eq!(requests.load(Ordering::SeqCst), 3);
        }
        "cancel" => {
            assert!(result.is_err());
            assert!(out.bytes.is_empty());
            assert_eq!(requests.load(Ordering::SeqCst), 2);
        }
        _ => {
            assert!(result.unwrap().is_err());
            assert!(out.bytes.is_empty());
            assert_eq!(
                requests.load(Ordering::SeqCst),
                if scenario == "writer" { 3 } else { 2 }
            );
        }
    }
    assert_eq!(std::fs::read_dir(directory).unwrap().count(), 0);
}
