use chrono::{DateTime, FixedOffset, TimeZone, Timelike};
use pixiv_sdk::diagnostics::Event;
use serde::Deserialize;
use std::{
    fmt, io,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

#[derive(Deserialize)]
pub struct Fixture {
    pub schema_version: u32,
    pub frozen_go_ref: String,
    pub render_cases: Vec<Render>,
    #[serde(default)]
    pub writer_cases: Vec<WriterCase>,
    #[serde(default)]
    pub default_cases: Vec<DefaultCase>,
    #[serde(default)]
    pub go_only_cases: Vec<GoOnly>,
    #[serde(default)]
    pub concurrent_cases: Vec<Concurrent>,
}

pub fn fixture() -> Fixture {
    serde_json::from_str(include_str!("../fixtures/diagnostics-presenter.json")).unwrap()
}
pub fn extra_fixture() -> Fixture {
    serde_json::from_str(include_str!("../fixtures/diagnostics-presenter-extra.json")).unwrap()
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ClockInput {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub nanosecond: u32,
    pub offset_seconds: i32,
}
impl ClockInput {
    pub fn value(&self) -> DateTime<FixedOffset> {
        FixedOffset::east_opt(self.offset_seconds)
            .unwrap()
            .with_ymd_and_hms(
                self.year,
                self.month,
                self.day,
                self.hour,
                self.minute,
                self.second,
            )
            .single()
            .unwrap()
            .with_nanosecond(self.nanosecond)
            .unwrap()
    }
}

#[derive(Clone, Deserialize)]
pub struct Input {
    pub module: String,
    pub kind: String,
    pub operation: String,
    pub resource: String,
    pub route: String,
    pub target: String,
    pub proxy: String,
    pub user_agent: String,
    pub reason: String,
    pub status: i64,
    pub count: i64,
    pub request_id: u64,
    pub duration_ns: i64,
}
impl Input {
    pub fn event(&self) -> Event {
        Event {
            module: self.module.clone(),
            kind: self.kind.clone(),
            operation: self.operation.clone(),
            resource: self.resource.clone(),
            route: self.route.clone(),
            target: self.target.clone(),
            proxy: self.proxy.clone(),
            user_agent: self.user_agent.clone(),
            reason: self.reason.clone(),
            status: self.status,
            count: self.count,
            request_id: self.request_id,
            duration_ns: self.duration_ns,
        }
    }
}

#[derive(Deserialize)]
pub struct Output {
    pub output_base64: String,
    pub output_is_utf8: bool,
    #[serde(default)]
    pub output_text: String,
}
impl Output {
    pub fn bytes(&self) -> Vec<u8> {
        let mut bits = 0u32;
        let mut length = 0;
        let mut result = Vec::new();
        for byte in self.output_base64.bytes().take_while(|byte| *byte != b'=') {
            let digit = match byte {
                b'A'..=b'Z' => byte - b'A',
                b'a'..=b'z' => byte - b'a' + 26,
                b'0'..=b'9' => byte - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                _ => panic!("invalid fixture base64"),
            };
            bits = (bits << 6) | u32::from(digit);
            length += 6;
            if length >= 8 {
                length -= 8;
                result.push((bits >> length) as u8);
                bits &= (1 << length) - 1;
            }
        }
        assert_eq!(std::str::from_utf8(&result).is_ok(), self.output_is_utf8);
        if self.output_is_utf8 {
            assert_eq!(result, self.output_text.as_bytes());
        }
        result
    }
}

#[derive(Deserialize)]
pub struct Render {
    pub name: String,
    pub compatibility: String,
    pub constructor: String,
    pub format: String,
    pub clock: ClockInput,
    pub event: Input,
    pub output: Output,
    pub clock_calls: usize,
    pub error: String,
    #[serde(default)]
    pub nil_writer: bool,
}
#[derive(Clone, Deserialize)]
pub struct Step {
    pub count: String,
    pub error: String,
}
#[derive(Deserialize)]
pub struct WriteCall {
    pub method: String,
    pub record: Output,
    pub returned_n: i64,
    pub error: String,
}
#[derive(Deserialize)]
pub struct ErrorState {
    pub error: String,
    pub is_first_error_identity: bool,
    pub is_later_error_identity: bool,
}
#[derive(Deserialize)]
pub struct WriterCase {
    pub name: String,
    pub compatibility: String,
    pub format: String,
    pub clock: ClockInput,
    pub implements_string_writer: bool,
    pub steps: Vec<Step>,
    pub events: Vec<Input>,
    pub calls: Vec<WriteCall>,
    pub stored_output: Output,
    pub error_after_each_emit: Vec<ErrorState>,
    pub clock_calls: usize,
}
#[derive(Deserialize)]
pub struct DefaultCase {
    pub name: String,
    pub constructor: String,
    pub format: String,
    pub nil_writer: bool,
    pub nil_clock: bool,
    pub event: Input,
    pub normalized_output: String,
    pub clock_in_call_window: bool,
    pub error: String,
}
#[derive(Deserialize)]
pub struct GoOnly {
    pub name: String,
    pub compatibility: String,
    pub panic: String,
    pub error: String,
    pub clock_calls: usize,
    pub writes: usize,
}
#[derive(Deserialize)]
pub struct Concurrent {
    pub name: String,
    pub format: String,
    pub clock: ClockInput,
    pub events: Vec<Input>,
    pub sorted_records: Vec<Output>,
    pub clock_calls: usize,
    pub max_concurrent_clock_calls: usize,
    pub max_concurrent_writer_calls: usize,
    pub intact_single_records: bool,
    pub error: String,
}

#[derive(Clone, Default)]
pub struct Capture(pub Arc<Mutex<Vec<Vec<u8>>>>);
impl io::Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().push(bytes.to_vec());
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        panic!("Presenter must not flush")
    }
}

#[derive(Clone, Debug)]
pub struct Marker {
    pub token: Arc<()>,
    pub text: &'static str,
}
impl fmt::Display for Marker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.text)
    }
}
impl std::error::Error for Marker {}
#[derive(Default)]
pub struct WriterState {
    pub calls: Vec<(Vec<u8>, usize, String)>,
    pub stored: Vec<u8>,
}
pub struct ScriptedWriter {
    pub steps: Vec<Step>,
    pub state: Arc<Mutex<WriterState>>,
    pub first: Marker,
    pub later: Marker,
}
impl io::Write for ScriptedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut state = self.state.lock().unwrap();
        let step = &self.steps[state.calls.len()];
        let n = match step.count.as_str() {
            "all" => bytes.len(),
            "zero" => 0,
            "short" => bytes.len() - 1,
            other => panic!("Go-only invalid io.Writer count: {other}"),
        };
        let error = match step.error.as_str() {
            "" => None,
            "first" => Some(self.first.clone()),
            "later" => Some(self.later.clone()),
            other => panic!("unknown writer step: {other}"),
        };
        state.stored.extend_from_slice(&bytes[..n]);
        state.calls.push((
            bytes.to_vec(),
            n,
            error.as_ref().map(ToString::to_string).unwrap_or_default(),
        ));
        match error {
            Some(error) => Err(io::Error::new(io::ErrorKind::BrokenPipe, error)),
            None => Ok(n),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        panic!("Presenter must not flush")
    }
}

#[derive(Default)]
pub struct Activity {
    pub calls: AtomicUsize,
    pub active: AtomicUsize,
    pub maximum: AtomicUsize,
}
impl Activity {
    pub fn enter(&self) {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.maximum.fetch_max(active, Ordering::SeqCst);
    }
    pub fn exit(&self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
}
pub struct ConcurrentWriter {
    pub capture: Capture,
    pub activity: Arc<Activity>,
}
impl io::Write for ConcurrentWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.activity.enter();
        for _ in 0..8 {
            std::thread::yield_now();
        }
        self.capture.0.lock().unwrap().push(bytes.to_vec());
        self.activity.exit();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        panic!("Presenter must not flush")
    }
}
