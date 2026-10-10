use base64::{Engine, engine::general_purpose::STANDARD};
use pixiv_sdk::{
    context::{Context, ContextError, ContextKey, RequestContext},
    error::{Cause, Error, is_canceled, is_deadline_exceeded},
    fanbox::{
        Client, Options, SessionCredentials,
        transport::{
            BodyFuture, ExternalError, RawBody, RawRead, RawRequest, RawResponse, RawTransport,
            TransportFuture,
        },
    },
};
use serde_json::{Value, json};
use std::{
    error::Error as StdError,
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

pub const SESSION: &str = "resource-session-secret-canary";
const SECRET: &str = "resource-raw-error-secret-canary";

#[derive(Debug)]
struct ExternalFailure(Option<Cause>);
impl fmt::Display for ExternalFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(SECRET)
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
            "canceled_then_deadline" => Some(Cause::Joined(vec![
                Cause::Canceled,
                Cause::DeadlineExceeded,
            ])),
            "deadline_then_canceled" => Some(Cause::Joined(vec![
                Cause::DeadlineExceeded,
                Cause::Canceled,
            ])),
            _ => None,
        })) as ExternalError
    })
}
pub fn error(error: Option<&(dyn StdError + 'static)>, eof: bool) -> Value {
    let mut result = json!({"message":if eof {"EOF".to_owned()} else {error.map(ToString::to_string).unwrap_or_default()},
        "reason":"", "canceled":error.is_some_and(|error|is_canceled(error)||error.downcast_ref::<ContextError>()==Some(&ContextError::Canceled)), "deadline_exceeded":error.is_some_and(|error|is_deadline_exceeded(error)||error.downcast_ref::<ContextError>()==Some(&ContextError::DeadlineExceeded)),
        "eof":eof, "exact_eof":eof, "unexpected_eof":false});
    if let Some(error) = error {
        let mut current = Some(error);
        while let Some(item) = current {
            assert!(
                !item.to_string().contains(SECRET),
                "external detail escaped redaction"
            );
            assert!(
                !item.to_string().contains(SESSION),
                "session escaped error redaction"
            );
            if let Some(classified) = item.downcast_ref::<Error>() {
                result["reason"] = json!(classified.code.as_str());
                result["sdk"] = json!({"product":classified.product,"operation":classified.operation,
                    "detail":classified.detail.as_deref().unwrap_or(""),"http_status":classified.http_status.unwrap_or(0),
                    "transport":classified.transport.map(|value|format!("{value:?}").to_ascii_lowercase()).unwrap_or_default(),
                    "retry_safe":classified.retry.safe,"retry_has_after":classified.retry.after.is_some()});
                break;
            }
            current = item.source();
        }
    }
    result
}
pub fn project(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.remove("go_only_error_tree");
            // Concrete source read sizes and call topology are an explicitly sealed Go-only projection.
            object.remove("read_calls");
            for value in object.values_mut() {
                project(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                project(value);
            }
        }
        _ => {}
    }
}
#[derive(Default)]
pub struct Record {
    name: String,
    bytes: usize,
    closes: usize,
}
struct FixtureBody {
    spec: Value,
    data: Vec<u8>,
    offset: usize,
    cancel: Context,
    record: Arc<Mutex<Record>>,
    trace: Arc<Mutex<Vec<String>>>,
}
impl RawBody for FixtureBody {
    fn read<'a>(&'a mut self, output: &'a mut [u8]) -> BodyFuture<'a, RawRead> {
        Box::pin(async move {
            let count = (self.data.len() - self.offset).min(output.len()).min(
                self.spec["chunk"]
                    .as_u64()
                    .filter(|value| *value > 0)
                    .map(|value| value as usize)
                    .unwrap_or(output.len()),
            );
            output[..count].copy_from_slice(&self.data[self.offset..self.offset + count]);
            self.offset += count;
            self.record.lock().unwrap().bytes += count;
            let last = self.offset == self.data.len()
                && (count == 0 || self.spec["error_with_last_bytes"] == true);
            let kind = if last {
                self.spec["read_error"].as_str().unwrap_or("")
            } else {
                ""
            };
            if self.spec["cancel_on_read"] == true {
                self.cancel.cancel();
            }
            RawRead {
                count,
                eof: last && (kind.is_empty() && count == 0 || kind == "EOF"),
                error: if kind == "EOF" { None } else { failure(kind) },
            }
        })
    }
    fn close(&mut self) -> BodyFuture<'_, std::result::Result<(), ExternalError>> {
        Box::pin(async move {
            let mut record = self.record.lock().unwrap();
            self.trace
                .lock()
                .unwrap()
                .push(format!("close:{}", record.name));
            record.closes += 1;
            drop(record);
            if self.spec["cancel_on_close"] == true {
                self.cancel.cancel();
            }
            match failure(self.spec["close_error"].as_str().unwrap_or("")) {
                Some(error) => Err(error),
                None => Ok(()),
            }
        })
    }
}
pub struct FixtureTransport {
    role: String,
    document: String,
    input: Value,
    pub requests: Mutex<Vec<Value>>,
    pub trace: Arc<Mutex<Vec<String>>>,
    bodies: Mutex<Vec<Arc<Mutex<Record>>>>,
    media_index: AtomicUsize,
    pub idle_calls: AtomicUsize,
    cancel: Context,
    context: Arc<dyn RequestContext>,
}
impl FixtureTransport {
    pub fn new(
        role: &str,
        document: &str,
        input: &Value,
        cancel: &Context,
        context: Arc<dyn RequestContext>,
    ) -> Self {
        let bodies = if input["_media_preowned"] == true {
            input["media"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
                .map(|(index, _)| {
                    Arc::new(Mutex::new(Record {
                        name: format!("{role}/media/{index}"),
                        bytes: 0,
                        closes: 0,
                    }))
                })
                .collect()
        } else {
            vec![]
        };
        Self {
            role: role.into(),
            document: document.into(),
            input: input.clone(),
            requests: Mutex::new(vec![]),
            trace: Arc::new(Mutex::new(vec![])),
            bodies: Mutex::new(bodies),
            media_index: AtomicUsize::new(0),
            idle_calls: AtomicUsize::new(0),
            cancel: cancel.clone(),
            context,
        }
    }
    pub fn client(self: &Arc<Self>) -> Client {
        self.client_with(SESSION, "resource-injected-agent")
    }
    pub fn client_with(self: &Arc<Self>, session: &str, agent: &str) -> Client {
        Client::open_with(
            SessionCredentials {
                fanbox_sessid: session.into(),
            },
            Options {
                http_client: Some(self.clone()),
                user_agent: agent.into(),
                ..Default::default()
            },
        )
        .unwrap()
    }
    pub fn ownership(&self) -> Vec<Value> {
        self.bodies
            .lock()
            .unwrap()
            .iter()
            .map(|record| {
                let record = record.lock().unwrap();
                json!({"name":record.name,"bytes_read":record.bytes,"close_calls":record.closes})
            })
            .collect()
    }
}
impl RawTransport for FixtureTransport {
    fn send(
        &self,
        request: RawRequest,
    ) -> TransportFuture<'_, std::result::Result<Option<RawResponse>, ExternalError>> {
        Box::pin(async move {
            self.trace
                .lock()
                .unwrap()
                .push(format!("resource:{}", request.url));
            let original = request
                .context
                .value(&ContextKey::new("resource-context-key"))
                .and_then(|value| value.downcast::<String>().ok())
                .map(|value| (*value).clone());
            assert_eq!(original.as_deref(), Some("resource-context"));
            // A deadline child is used only after resource generation.
            if self.context.error().is_none() && request.context.deadline().is_none() {
                assert!(
                    Arc::ptr_eq(&self.context, &request.context),
                    "caller context ownership"
                );
            }
            let target = url::Url::parse(&request.url).unwrap();
            let host = request
                .url
                .split_once("://")
                .unwrap()
                .1
                .split(['/', '?', '#'])
                .next()
                .unwrap();
            let context_error = request.context.error();
            self.requests.lock().unwrap().push(json!({"client":self.role,"method":request.method,"url":request.url,"host":host,
                "headers":request.headers,"has_body":request.body.is_some(),"content_length":request.content_length,
                "context_error":error(context_error.as_ref().map(|error|error as &dyn StdError),false),"context_value":original}));
            let metadata = target.host_str() == Some("api.fanbox.cc")
                && matches!(target.path(), "/creator.get" | "/post.info");
            if (!metadata
                || self.role == "consumer" && self.input["transport_error_at_metadata"] == true)
                && let Some(error) = failure(self.input["transport_error"].as_str().unwrap_or(""))
            {
                return Err(error);
            }
            let (mut spec, name) = if metadata {
                (
                    if self.role == "producer" {
                        json!({})
                    } else {
                        self.input["metadata"].clone()
                    },
                    format!("{}/metadata", self.role),
                )
            } else {
                let index = self.media_index.fetch_add(1, Ordering::SeqCst);
                let spec = self.input["media"]
                    .get(index)
                    .unwrap_or_else(|| panic!("unexpected media request {index}: {}", request.url))
                    .clone();
                (spec, format!("{}/media/{index}", self.role))
            };
            if metadata {
                spec["data"] = json!(self.document);
                if spec["status"].as_u64().unwrap_or(0) == 0 {
                    spec["status"] = json!(200);
                }
                if spec["header"].is_null() {
                    spec["header"] = json!({"Content-Type":["application/json"]});
                }
            }
            let data = spec["wire"]
                .as_str()
                .map(|wire| STANDARD.decode(wire).unwrap())
                .unwrap_or_else(|| spec["data"].as_str().unwrap_or("").as_bytes().to_vec());
            let existing = if !metadata && self.input["_media_preowned"] == true {
                self.bodies
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|record| record.lock().unwrap().name == name)
                    .cloned()
            } else {
                None
            };
            let record = existing.unwrap_or_else(|| {
                let record = Arc::new(Mutex::new(Record {
                    name,
                    bytes: 0,
                    closes: 0,
                }));
                if spec["nil_body"] != true {
                    self.bodies.lock().unwrap().push(record.clone());
                }
                record
            });
            let body = (spec["nil_body"] != true).then(|| {
                Box::new(FixtureBody {
                    spec: spec.clone(),
                    data,
                    offset: 0,
                    cancel: self.cancel.clone(),
                    record,
                    trace: self.trace.clone(),
                }) as Box<dyn RawBody>
            });
            Ok(Some(RawResponse {
                status: spec["status"].as_u64().unwrap_or(0) as u16,
                headers: serde_json::from_value(
                    spec["header"]
                        .as_object()
                        .map(|_| spec["header"].clone())
                        .unwrap_or_else(|| json!({})),
                )
                .unwrap(),
                content_length: spec["transport_content_length"].as_i64().unwrap_or(0),
                body,
            }))
        })
    }
    fn close_idle_connections(&self) {
        self.idle_calls.fetch_add(1, Ordering::SeqCst);
    }
}
pub fn ownership(producer: &FixtureTransport, consumer: &FixtureTransport) -> Value {
    json!(
        producer
            .ownership()
            .into_iter()
            .chain(consumer.ownership())
            .collect::<Vec<_>>()
    )
}
