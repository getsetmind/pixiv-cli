use pixiv_sdk::{
    Result,
    transport::{Request, Response, Transport, encode_query_pairs},
};
use serde::Deserialize;
use serde_json::Value;
use std::{
    io::{self, Write},
    sync::{Arc, Mutex},
};

#[derive(Deserialize)]
pub struct Fixture {
    pub reference_commit: String,
    pub cases: Vec<Case>,
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
    pub configured_json: bool,
    pub writer: String,
    pub artwork_body: Value,
    pub metadata_body: Value,
    pub artwork_status: u16,
    pub metadata_status: u16,
    pub json_error: Option<String>,
    pub pool_error: Option<String>,
    pub missing: Option<String>,
    pub typed_metadata: Option<Value>,
}

#[derive(Deserialize)]
pub struct Expected {
    pub error: String,
    pub error_dto: Option<Value>,
    pub usage: bool,
    pub stdout: String,
    pub help_output: String,
    pub json_overrides: Vec<Option<bool>>,
    pub proxy_overrides: Vec<Option<String>>,
    pub requests: Vec<RequestObservation>,
    pub writes: Vec<WriteObservation>,
    pub callback_committed: Vec<bool>,
    pub input_reads: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct RequestObservation {
    pub method: String,
    pub uri: String,
    pub authorization: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct WriteObservation {
    pub input: String,
    pub count: usize,
    pub error: String,
}

pub fn cases() -> Vec<Case> {
    let fixture: Fixture =
        serde_json::from_str(include_str!("../fixtures/cli-ugoira-owner.json")).unwrap();
    assert_eq!(
        fixture.reference_commit,
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    assert_eq!(fixture.cases.len(), 97);
    fixture.cases
}

#[derive(Clone)]
pub struct ApiTransport {
    pub input: Input,
    pub requests: Arc<Mutex<Vec<RequestObservation>>>,
}
impl Transport for ApiTransport {
    async fn send(&self, request: Request) -> Result<Response> {
        let path = request
            .url
            .strip_prefix("https://app-api.pixiv.net")
            .expect("metadata command must use the genuine API host");
        let authorization = request
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
            .map(|(_, value)| value.clone())
            .unwrap_or_default();
        let query = encode_query_pairs(&request.parameters);
        self.requests.lock().unwrap().push(RequestObservation {
            method: request.method.to_string(),
            uri: format!("{path}?{query}"),
            authorization,
        });
        let (status, body) = match path {
            "/v1/illust/detail" => (self.input.artwork_status, &self.input.artwork_body),
            "/v1/ugoira/metadata" => (self.input.metadata_status, &self.input.metadata_body),
            _ => panic!("unexpected metadata command request: {path}"),
        };
        Ok(Response {
            status,
            retry_after: None,
            body: body.clone(),
        })
    }
}

pub struct ObservingWriter {
    mode: String,
    pub output: Vec<u8>,
    pub writes: Vec<WriteObservation>,
}
impl ObservingWriter {
    pub fn new(mode: &str) -> Self {
        Self {
            mode: mode.into(),
            output: vec![],
            writes: vec![],
        }
    }
}
impl Write for ObservingWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let call = self.writes.len() + 1;
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
            Some(_) => "fixture writer failure",
            None => "",
        };
        self.output.extend_from_slice(&bytes[..count]);
        self.writes.push(WriteObservation {
            input: String::from_utf8(bytes.into()).unwrap(),
            count,
            error: message.into(),
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

pub fn error_dto(error: &pixiv_sdk::Error) -> Value {
    serde_json::json!({
        "Product":error.product,"Operation":error.operation,"Reason":error.code.as_str(),
        "Detail":error.detail.as_deref().unwrap_or_default(),
        "HTTPStatus":error.http_status.unwrap_or_default(),
        "Transport":error.transport.map(|kind|serde_json::to_value(kind).unwrap().as_str().unwrap().to_owned()).unwrap_or_default(),
        "Retry":{"Safe":error.retry.safe,"HasAfter":error.retry.after.is_some(),
            "After":error.retry.after.map(|date|date.to_rfc3339()).unwrap_or_else(||"0001-01-01T00:00:00Z".into())}
    })
}
