use pixiv_app::{
    callback_handler::CallbackResult,
    download::{ArtworkFuture, DownloadSaveClient, SaveFuture},
    lifecycle::Context,
    scheduler::SchedulerError,
};
use pixiv_cli_rs::startup::StartupHooks;
use pixiv_sdk::{
    Client,
    resource::{ResourceRef, SavedResource},
    transport::{Request, Response, Transport},
};
use serde_json::{Value, json};
use std::{
    io::{self, Read, Write},
    path::PathBuf,
    process::{Child, Output},
    sync::{Arc, Mutex},
};

pub struct OwnedChild(Option<Child>);
impl OwnedChild {
    pub fn new(child: Child) -> Self {
        Self(Some(child))
    }
    pub fn child(&mut self) -> &mut Child {
        self.0.as_mut().unwrap()
    }
    pub fn output(mut self) -> io::Result<Output> {
        if self.child().try_wait()?.is_none() {
            return Err(io::Error::other(
                "owned child must terminate before collecting output",
            ));
        }
        self.0.take().unwrap().wait_with_output()
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub fn text<'a>(row: &'a Value, name: &str) -> &'a str {
    row[name].as_str().unwrap()
}

pub struct Reader {
    pub row: Value,
    pub reads: usize,
    pub bytes: usize,
    position: usize,
    failed: bool,
}
impl Reader {
    pub fn new(row: &Value) -> Self {
        Self {
            row: row.clone(),
            reads: 0,
            bytes: 0,
            position: 0,
            failed: false,
        }
    }
}
impl Read for Reader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.reads += 1;
        if self.row["read_error"] == true {
            return Err(io::Error::other("synthetic stdin must not be read"));
        }
        let failure = text(&self.row, "read_failure");
        if self.failed
            || failure == "classification"
            || failure == "record-before-line" && self.position > 0
        {
            return Err(io::Error::other("synthetic input failure"));
        }
        let input = text(&self.row, "input").as_bytes();
        let count = output.len().min(input.len() - self.position);
        output[..count].copy_from_slice(&input[self.position..self.position + count]);
        self.position += count;
        self.bytes += count;
        self.failed = failure == "classification-data"
            || matches!(failure, "record-tail" | "text-tail") && output.len() > 1;
        Ok(count)
    }
}

pub struct Writer {
    row: Value,
    stream: &'static str,
    writes: usize,
    pub bytes: Vec<u8>,
}
impl Writer {
    pub fn new(row: &Value, stream: &'static str) -> Self {
        Self {
            row: row.clone(),
            stream,
            writes: 0,
            bytes: vec![],
        }
    }
}
impl Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.writes += 1;
        let failure = text(&self.row, "writer_failure");
        if failure == format!("{}-always", self.stream)
            || failure == format!("{}-once", self.stream) && self.writes == 1
        {
            return Err(if text(&self.row, "writer_cause") == "pipe" {
                io::Error::from(io::ErrorKind::BrokenPipe)
            } else {
                io::Error::other("synthetic diagnostic writer failure")
            });
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub struct Hooks {
    pub row: Value,
    pub calls: Arc<Mutex<Vec<String>>>,
}
impl StartupHooks for Hooks {
    fn cleanup_pending_update(&self) -> CallbackResult<()> {
        self.calls.lock().unwrap().push("cleanup".into());
        let message = text(&self.row, "cleanup_error");
        if message.is_empty() {
            Ok(())
        } else {
            Err(Box::new(io::Error::other(message.to_owned())))
        }
    }
    fn automatic_supported(&self) -> bool {
        self.calls.lock().unwrap().push("supported".into());
        self.row["supported"].as_bool().unwrap()
    }
    fn ensure_if_needed(&self, _: &Context) -> CallbackResult<()> {
        self.calls.lock().unwrap().push("ensure".into());
        let message = text(&self.row, "ensure_error");
        if message.is_empty() {
            Ok(())
        } else {
            Err(Box::new(io::Error::other(message.to_owned())))
        }
    }
}

#[derive(Clone)]
pub struct FixtureTransport;
impl Transport for FixtureTransport {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        let body = if request.operation == "Open" {
            json!({"access_token":"owned-synthetic-access","refresh_token":"owned-synthetic-rotated","expires_in":3600,"user":{"id":42}})
        } else {
            assert_eq!(request.operation, "Artwork");
            let id: i64 = request
                .parameters
                .iter()
                .find(|(key, _)| key == "illust_id")
                .unwrap()
                .1
                .parse()
                .unwrap();
            json!({"illust":{"id":id,"type":"illust","title":"root","user":{"id":7,"name":"author"},"create_date":"2026-01-02T03:04:05+00:00","page_count":1,"tags":[],"image_urls":{"large":format!("https://i.pximg.net/{id}.png")},"meta_single_page":{"original_image_url":format!("https://i.pximg.net/{id}.png")}}})
        };
        Ok(Response {
            status: 200,
            retry_after: None,
            body,
        })
    }
}

pub struct SaveClient {
    pub client: Arc<Client<FixtureTransport>>,
    pub ids: Arc<Mutex<Vec<i64>>>,
    pub root: PathBuf,
    pub cancel_after: bool,
}
impl DownloadSaveClient for SaveClient {
    fn artwork(&self, _: Context, id: i64) -> Option<ArtworkFuture<'_>> {
        self.ids.lock().unwrap().push(id);
        Some(Box::pin(async move {
            self.client.artwork(id).await.map_err(Into::into)
        }))
    }
    fn save_ref(&self, context: Context, _: ResourceRef, path: PathBuf) -> SaveFuture<'_> {
        Box::pin(async move {
            assert!(
                path.starts_with(&self.root),
                "download escaped owned directory"
            );
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"payload").unwrap();
            if self.cancel_after {
                context.cancel();
            }
            Ok(SavedResource {
                path: path.to_string_lossy().into_owned(),
                size: 7,
                content_type: "image/png".into(),
            })
        })
    }
    fn save_url(&self, _: Context, _: String, _: PathBuf) -> SaveFuture<'_> {
        Box::pin(async {
            Err(SchedulerError::Message(
                "unexpected direct resource in root fixture".into(),
            ))
        })
    }
}

pub fn excluded_composition(row: &Value) -> Option<&'static str> {
    match text(row, "name") {
        "classification-zero-once" => {
            Some("Go non-EOF (0,nil) is not representable by Rust Read::Ok(0), which means EOF")
        }
        "manager-error-fail-fast-marker" => Some(
            "Go private DownloadManager batch-level error has no equivalent public Rust injection port",
        ),
        "fatal-pipeline-human"
        | "fatal-pipeline-json-envelope"
        | "fatal-pipe-ndjson-success"
        | "fatal-pipe-json-failure" => Some(
            "Go private Pooled FatalRecordPipeline injection is covered only by public finish_command presentation",
        ),
        _ => None,
    }
}
