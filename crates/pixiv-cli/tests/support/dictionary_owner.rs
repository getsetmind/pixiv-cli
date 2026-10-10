use pixiv_app::lifecycle::{Context, ContextError};
use pixiv_cli_rs::dictionary::service::{Response, Transport, TransportError};
use serde::Deserialize;
use std::{
    error::Error,
    fmt,
    io::{self, Write},
    sync::{Arc, Mutex},
    time::Instant,
};

#[derive(Deserialize)]
pub struct Fixture {
    pub reference: String,
    pub sources: std::collections::BTreeMap<String, String>,
    pub commands: Vec<serde_json::Value>,
    pub rows: Vec<Case>,
}

#[derive(Deserialize)]
pub struct Case {
    pub input: Input,
    pub result: Expected,
}

#[derive(Clone, Deserialize)]
pub struct Input {
    pub name: String,
    pub args: Vec<String>,
    pub input: String,
    pub configured_json: bool,
    pub json_error: String,
    pub missing_json_out: bool,
    pub reader_mode: String,
    pub writer: String,
    pub context: String,
    pub responses: Vec<WireResponse>,
}

#[derive(Clone, Deserialize)]
pub struct WireResponse {
    pub body: String,
    pub status: u16,
    pub error: String,
}

#[derive(Deserialize)]
pub struct Expected {
    pub error: String,
    pub selected: String,
    pub json_changed: bool,
    pub json_flag: bool,
    pub ndjson_flag: bool,
    pub error_broken_pipe: bool,
    pub usage: bool,
    pub dic_code: String,
    pub http_status: u16,
    pub sdk_reason: String,
    pub error_canceled: bool,
    pub error_deadline: bool,
    pub error_cause: bool,
    pub stdout: String,
    pub help_output: String,
    pub diagnostics: String,
    pub json_overrides: Vec<Option<bool>>,
    pub requests: Vec<RequestObservation>,
    pub events: Vec<String>,
    pub writes: Vec<WriteObservation>,
    pub input_reads: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct RequestObservation {
    pub url: String,
    pub accept: String,
    pub context_inherited: bool,
    pub context_error: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct WriteObservation {
    pub input: String,
    pub count: usize,
    pub error: String,
    pub requests_complete: usize,
}

#[derive(Default)]
pub struct Observed {
    pub requests: Vec<RequestObservation>,
    pub events: Vec<String>,
    pub json_overrides: Vec<Option<bool>>,
    pub writes: Vec<WriteObservation>,
}

pub fn fixture() -> Fixture {
    let fixture: Fixture =
        serde_json::from_str(include_str!("../fixtures/cli-dictionary-owner.json")).unwrap();
    assert_eq!(
        fixture.reference,
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    assert_eq!(fixture.rows.len(), 207);
    fixture
}

pub fn context(input: &str) -> Context {
    match input {
        "canceled" => {
            let context = Context::new();
            context.cancel();
            context
        }
        "deadline" => Context::with_deadline(Instant::now()),
        "active" => Context::new(),
        other => panic!("unexpected context fixture {other}"),
    }
}

#[derive(Clone)]
pub struct ApiTransport {
    pub responses: Vec<WireResponse>,
    pub context_identity: usize,
    pub observed: Arc<Mutex<Observed>>,
}

impl Transport for ApiTransport {
    async fn get(
        &self,
        context: Option<&Context>,
        raw_url: &str,
        accept: &str,
    ) -> Result<Response, TransportError> {
        assert!(raw_url.starts_with("https://dic.pixiv.net/"));
        let context = context.expect("dictionary owner must propagate the root context");
        let mut observed = self.observed.lock().unwrap();
        let index = observed.requests.len();
        observed.requests.push(RequestObservation {
            url: raw_url.into(),
            accept: accept.into(),
            context_inherited: context as *const Context as usize == self.context_identity,
            context_error: context
                .error()
                .map(|error| error.to_string())
                .unwrap_or_default(),
        });
        observed.events.push("get".into());
        let response = self
            .responses
            .get(index)
            .expect("unexpected dictionary request");
        match response.error.as_str() {
            "synthetic-cause" => Err(Box::new(SyntheticCause)),
            "canceled" => Err(Box::new(ContextError::Canceled)),
            "deadline" => Err(Box::new(ContextError::DeadlineExceeded)),
            "" => Ok(Response {
                status: response.status,
                body: response.body.as_bytes().to_vec(),
            }),
            other => panic!("unexpected transport error fixture {other}"),
        }
    }
}

#[derive(Debug)]
pub struct SyntheticCause;
impl fmt::Display for SyntheticCause {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("synthetic dictionary transport cause")
    }
}
impl Error for SyntheticCause {}

pub struct ObservingWriter {
    pub mode: String,
    pub output: Vec<u8>,
    pub observed: Arc<Mutex<Observed>>,
}

impl Write for ObservingWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut observed = self.observed.lock().unwrap();
        let call = observed.writes.len() + 1;
        let (count, error) = match self.mode.as_str() {
            "short-nil" => (bytes.len() / 2, None),
            "zero-nil" => (0, None),
            "fail-first" => (0, Some(io::ErrorKind::Other)),
            "partial-error" => (5.min(bytes.len()), Some(io::ErrorKind::Other)),
            "fail-fourth" if call == 4 => (0, Some(io::ErrorKind::Other)),
            "broken-pipe" => (0, Some(io::ErrorKind::BrokenPipe)),
            _ => (bytes.len(), None),
        };
        let message = match error {
            Some(io::ErrorKind::BrokenPipe) => "broken pipe",
            Some(_) => "synthetic dictionary writer failure",
            None => "",
        };
        self.output.extend_from_slice(&bytes[..count]);
        let requests_complete = observed.requests.len();
        observed.events.push("write".into());
        observed.writes.push(WriteObservation {
            input: String::from_utf8(bytes.to_vec()).unwrap(),
            count,
            error: message.into(),
            requests_complete,
        });
        match error {
            Some(kind) => Err(io::Error::new(kind, message)),
            None => Ok(count),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub fn source_is<E: Error + 'static>(error: &(dyn Error + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(error) = current {
        if error.is::<E>() {
            return true;
        }
        current = error.source();
    }
    false
}

pub fn context_source(error: &(dyn Error + 'static)) -> Option<ContextError> {
    let mut current = Some(error);
    while let Some(error) = current {
        if let Some(context) = error.downcast_ref::<ContextError>() {
            return Some(*context);
        }
        current = error.source();
    }
    None
}

pub fn source<'a, E: Error + 'static>(error: &'a (dyn Error + 'static)) -> Option<&'a E> {
    let mut current = Some(error);
    while let Some(error) = current {
        if let Some(classified) = error.downcast_ref::<E>() {
            return Some(classified);
        }
        current = error.source();
    }
    None
}
