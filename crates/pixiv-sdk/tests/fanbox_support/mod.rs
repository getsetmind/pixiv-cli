use pixiv_sdk::{
    context::{Context, ContextKey, RequestContext},
    error::{Cause, Error, is_canceled, is_deadline_exceeded},
    fanbox::transport::{
        BodyFuture, ExternalError, RawBody, RawRead, RawRequest, RawResponse, RawTransport,
        TransportFuture,
    },
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    error::Error as StdError,
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

pub const SESSION: &str = "synthetic-contract-session";
pub fn body_bytes(step: &Value) -> Vec<u8> {
    let bytes = match step["body_hex"].as_str() {
        Some(hex) => {
            assert_eq!(hex.len() % 2, 0, "raw body hex length");
            assert!(
                hex.bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
                "raw body hex must be lowercase"
            );
            hex.as_bytes()
                .chunks_exact(2)
                .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                .collect()
        }
        None => step["body"].as_str().unwrap_or("").as_bytes().to_vec(),
    };
    if let Some(length) = step["body_length"].as_u64() {
        assert_eq!(bytes.len() as u64, length, "raw body length");
    }
    if let Some(sha256) = step["body_sha256"].as_str() {
        assert_eq!(
            format!("{:x}", Sha256::digest(&bytes)),
            sha256,
            "raw body SHA256"
        );
    }
    bytes
}
#[derive(Debug)]
struct ExternalFailure(Option<Cause>);
impl fmt::Display for ExternalFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(
            "synthetic-external-detail https://example.invalid/private?token=synthetic-secret",
        )
    }
}
impl StdError for ExternalFailure {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.0.as_ref().map(|cause| cause as &dyn StdError)
    }
}
pub fn failure(kind: &str) -> Option<ExternalError> {
    (!kind.is_empty()).then(|| {
        Box::new(ExternalFailure(match kind {
            "canceled" => Some(Cause::Canceled),
            "deadline" => Some(Cause::DeadlineExceeded),
            _ => None,
        })) as ExternalError
    })
}
#[derive(Default)]
pub struct BodyRecord {
    pub bytes: usize,
    pub closes: usize,
    pub reads: usize,
}
struct FixtureBody {
    step: Value,
    bytes: Vec<u8>,
    offset: usize,
    cancel: Context,
    record: Arc<Mutex<BodyRecord>>,
}
impl RawBody for FixtureBody {
    fn read<'a>(&'a mut self, output: &'a mut [u8]) -> BodyFuture<'a, RawRead> {
        Box::pin(async move {
            if self.step["cancel_on_read"] == true {
                self.cancel.cancel();
            }
            let body = &self.bytes;
            let limit = self.step["chunk"]
                .as_u64()
                .filter(|count| *count > 0)
                .map(|count| count as usize)
                .unwrap_or(output.len());
            let count = (body.len() - self.offset).min(output.len()).min(limit);
            output[..count].copy_from_slice(&body[self.offset..self.offset + count]);
            self.offset += count;
            let mut record = self.record.lock().unwrap();
            record.bytes += count;
            record.reads += 1;
            let kind = if self.offset == body.len() {
                self.step["read_error"].as_str().unwrap_or("")
            } else {
                ""
            };
            RawRead {
                count,
                eof: kind == "eof" || count == 0 && kind.is_empty(),
                error: if kind == "eof" { None } else { failure(kind) },
            }
        })
    }
    fn close(&mut self) -> BodyFuture<'_, std::result::Result<(), ExternalError>> {
        Box::pin(async move {
            self.record.lock().unwrap().closes += 1;
            if self.step["cancel_on_close"] == true {
                self.cancel.cancel();
            }
            match failure(self.step["close_error"].as_str().unwrap_or("")) {
                Some(error) => Err(error),
                None => Ok(()),
            }
        })
    }
}
pub struct FixtureTransport {
    pub steps: Vec<Value>,
    pub requests: Mutex<Vec<Value>>,
    pub contexts: Mutex<Vec<Arc<dyn RequestContext>>>,
    pub bodies: Mutex<Vec<(bool, Arc<Mutex<BodyRecord>>)>>,
    pub idle_calls: AtomicUsize,
    pub cancel: Context,
}
impl FixtureTransport {
    pub fn new(steps: Vec<Value>, cancel: Context) -> Self {
        Self {
            steps,
            requests: Mutex::new(vec![]),
            contexts: Mutex::new(vec![]),
            bodies: Mutex::new(vec![]),
            idle_calls: AtomicUsize::new(0),
            cancel,
        }
    }
    pub fn body_projection(&self) -> Value {
        Value::Array(self.bodies.lock().unwrap().iter().map(|(injected, record)| {
            let record = record.lock().unwrap();
            json!({"injected_body":injected,"bytes_read":record.bytes,"close_calls":record.closes})
        }).collect())
    }
}
impl RawTransport for FixtureTransport {
    fn send(
        &self,
        request: RawRequest,
    ) -> TransportFuture<'_, std::result::Result<Option<RawResponse>, ExternalError>> {
        Box::pin(async move {
            let mut headers = request.headers.clone();
            for (name, values) in &mut headers {
                if name.eq_ignore_ascii_case("Cookie") {
                    for value in values {
                        *value = value.replace(SESSION, "[SYNTHETIC_SESSION]");
                    }
                }
            }
            let value = request
                .context
                .value(&ContextKey::new("fanbox-fixture"))
                .and_then(|value| value.downcast::<String>().ok())
                .map(|value| (*value).clone());
            let index = {
                let mut requests = self.requests.lock().unwrap();
                let index = requests.len();
                requests.push(json!({"method":request.method,"url":request.url,"headers":headers,
                    "context_value":value,"context_canceled":request.context.error().is_some(),
                    "context_has_deadline":request.context.deadline().is_some(),"request_has_body":request.body.is_some()}));
                index
            };
            self.contexts.lock().unwrap().push(request.context);
            let step = self
                .steps
                .get(index)
                .unwrap_or_else(|| panic!("unexpected transport call {index}"));
            let kind = step["transport_error"].as_str().unwrap_or("");
            if kind == "nil_response" {
                return Ok(None);
            }
            if let Some(error) = failure(kind) {
                return Err(error);
            }
            let injected = step["nil_body"] != true;
            let record = Arc::new(Mutex::new(BodyRecord::default()));
            self.bodies.lock().unwrap().push((injected, record.clone()));
            let mut headers: BTreeMap<String, Vec<String>> = step
                .get("headers")
                .map(|value| serde_json::from_value(value.clone()).unwrap())
                .unwrap_or_default();
            if let Some(location) = step["location"].as_str().filter(|value| !value.is_empty()) {
                headers.insert("Location".into(), vec![location.into()]);
            }
            Ok(Some(RawResponse {
                status: step["status"].as_u64().unwrap_or(0) as u16,
                headers,
                content_length: step["content_length"].as_i64().unwrap_or(0),
                body: injected.then(|| {
                    Box::new(FixtureBody {
                        step: step.clone(),
                        bytes: body_bytes(step),
                        offset: 0,
                        cancel: self.cancel.clone(),
                        record,
                    }) as Box<dyn RawBody>
                }),
            }))
        })
    }
    fn close_idle_connections(&self) {
        self.idle_calls.fetch_add(1, Ordering::SeqCst);
    }
}
pub fn error_projection(error: Option<&Error>) -> Value {
    match error {
        None => json!({"message":"","reason":"","canceled":false,"deadline":false}),
        Some(error) => {
            json!({"message":error.to_string(),"reason":error.code.as_str(),"canceled":is_canceled(error),"deadline":is_deadline_exceeded(error),
            "sdk":{"product":error.product,"operation":error.operation,"reason":error.code.as_str(),
            "detail":error.detail.clone().unwrap_or_default(),"http_status":error.http_status.unwrap_or(0),
            "transport":error.transport.map(|kind| format!("{kind:?}").to_lowercase()).unwrap_or_default(),
            "retry_safe":error.retry.safe,"retry_has_after":error.retry.after.is_some()}})
        }
    }
}
pub fn expected_error(value: &Value) -> Value {
    let mut result = json!({"message":value["message"],"reason":value["reason"],"canceled":value["canceled"],"deadline":value["deadline"]});
    if let Some(sdk) = value.get("sdk") {
        let mut sdk = sdk.clone();
        sdk.as_object_mut()
            .unwrap()
            .remove("go_only_matches_reason");
        result["sdk"] = sdk;
    }
    result
}
pub fn context_for(input: &Value) -> Context {
    let context = match input["context"].as_str().unwrap_or("") {
        "deadline" => {
            Context::with_deadline(std::time::Instant::now() - std::time::Duration::from_secs(1))
        }
        "future_deadline" => {
            Context::with_deadline(std::time::Instant::now() + std::time::Duration::from_secs(3600))
        }
        _ => Context::new(),
    }
    .with_value(
        ContextKey::new("fanbox-fixture"),
        Arc::new("synthetic-context".to_owned()),
    );
    if input["context"] == "canceled" {
        context.cancel();
    }
    context
}
