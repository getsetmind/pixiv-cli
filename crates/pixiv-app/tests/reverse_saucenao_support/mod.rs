use base64::{Engine, engine::general_purpose::STANDARD};
use pixiv_app::reverse_search::{
    ReverseFuture,
    http::{HttpRequest, HttpTransport},
};
use pixiv_sdk::{
    context::{Context, ContextKey},
    fanbox::transport::{BodyFuture, ExternalError, RawBody, RawRead, RawResponse},
};
use serde_json::{Value, json};
use std::{
    io::Read,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

pub fn bytes(value: &Value) -> Vec<u8> {
    STANDARD.decode(value.as_str().unwrap()).unwrap()
}
pub fn caller_key() -> ContextKey {
    ContextKey::new("SauceNAO fixture caller")
}

#[derive(Default)]
pub struct BodyObservation {
    pub bytes: usize,
    pub reads: usize,
    pub closes: usize,
}
pub struct Transport {
    pub input: Value,
    pub expected_requests: Vec<Value>,
    pub requests: Mutex<Vec<Value>>,
    pub bodies: Mutex<Vec<Arc<Mutex<BodyObservation>>>>,
    pub idle: AtomicUsize,
    pub context: Arc<Context>,
}
impl HttpTransport for Transport {
    fn send(
        &self,
        mut request: HttpRequest,
    ) -> ReverseFuture<'_, Result<Option<RawResponse>, ExternalError>> {
        Box::pin(async move {
            let mut wire = Vec::new();
            let read_error = request.body.as_mut().unwrap().read_to_end(&mut wire).err();
            let index = self.requests.lock().unwrap().len();
            let expected = &self.expected_requests[index];
            let content_type = request.headers.remove("Content-Type").unwrap();
            assert_eq!(content_type.len(), 1);
            let boundary = content_type[0]
                .strip_prefix("multipart/form-data; boundary=")
                .unwrap();
            assert_eq!(boundary.len(), 60);
            assert!(
                boundary
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            );
            let parts = expected["content_type"]["parts"].as_array().unwrap();
            let mut exact = Vec::new();
            for (index, part) in parts.iter().enumerate() {
                if index != 0 {
                    exact.extend_from_slice(b"\r\n");
                }
                exact.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
                for (name, values) in part["headers"].as_object().unwrap() {
                    for value in values.as_array().unwrap() {
                        exact.extend_from_slice(
                            format!("{name}: {}\r\n", value.as_str().unwrap()).as_bytes(),
                        );
                    }
                }
                exact.extend_from_slice(b"\r\n");
                exact.extend_from_slice(&bytes(&part["payload"]));
            }
            exact.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
            assert_eq!(wire, exact, "complete Go multipart wire framing");
            assert_eq!(
                wire.len(),
                expected["content_type"]["wire_length"].as_u64().unwrap() as usize
            );
            let expected_read = &expected["content_type"]["read_error"];
            assert_eq!(
                read_error.as_ref().map(ToString::to_string),
                expected_read["message"].as_str().map(str::to_owned)
            );
            if let Some(error) = &read_error {
                let code = error
                    .get_ref()
                    .and_then(|error| error.downcast_ref::<pixiv_app::reverse_search::Error>())
                    .map_or("unknown", |error| error.code().as_str());
                assert_eq!(code, expected_read["code"]);
            }
            assert_eq!(request.method, expected["method"]);
            assert_eq!(request.url, expected["url"]);
            assert_eq!(
                request.content_length,
                expected["content_length"].as_i64().unwrap()
            );
            assert_eq!(
                serde_json::to_value(request.headers).unwrap(),
                expected["headers_without_generated_content_type"]
            );
            assert!(Arc::ptr_eq(
                &request.context,
                &(self.context.clone() as Arc<dyn pixiv_sdk::context::RequestContext>)
            ));
            let value = request
                .context
                .value(&caller_key())
                .unwrap()
                .downcast::<String>()
                .unwrap();
            assert_eq!(value.as_str(), expected["context_value"]);
            assert!(request.context.error().is_none());
            assert!(expected["context_error"].is_null());
            let parsed = url::Url::parse(&request.url).unwrap();
            assert_eq!(parsed.host_str().unwrap(), expected["host"]);
            let request_uri = format!(
                "{}{}",
                parsed.path(),
                parsed
                    .query()
                    .map_or(String::new(), |query| format!("?{query}"))
            );
            assert_eq!(request_uri, expected["request_uri"]);
            assert_eq!(expected["get_body_available"], false);
            assert!(expected["transfer_encoding"].is_null());
            self.requests
                .lock()
                .unwrap()
                .push(json!({"wire_length": wire.len()}));
            if self.input["cancel_at"] == "transport" {
                self.context.cancel();
            }
            if self.input["transport_error"] != "" {
                return Err(std::io::Error::other("synthetic-upstream-error-marker").into());
            }
            let body = if self.input["nil_body"] == true {
                None
            } else {
                let observation = Arc::new(Mutex::new(BodyObservation::default()));
                self.bodies.lock().unwrap().push(observation.clone());
                let data = if self.input["response_bytes"].is_string() {
                    bytes(&self.input["response_bytes"])
                } else {
                    self.input["response"].as_str().unwrap().as_bytes().to_vec()
                };
                Some(Box::new(Body {
                    input: self.input.clone(),
                    data,
                    offset: 0,
                    observation,
                    context: self.context.clone(),
                }) as Box<dyn RawBody>)
            };
            Ok(Some(RawResponse {
                status: self.input["status"].as_u64().unwrap() as u16,
                headers: Default::default(),
                content_length: -1,
                body,
            }))
        })
    }
    fn close_idle_connections(&self) {
        self.idle.fetch_add(1, Ordering::SeqCst);
    }
}
struct Body {
    input: Value,
    data: Vec<u8>,
    offset: usize,
    observation: Arc<Mutex<BodyObservation>>,
    context: Arc<Context>,
}
impl RawBody for Body {
    fn read<'a>(&'a mut self, output: &'a mut [u8]) -> BodyFuture<'a, RawRead> {
        Box::pin(async move {
            let chunk = self.input["response_chunk"].as_u64().unwrap() as usize;
            let count = (self.data.len() - self.offset)
                .min(output.len())
                .min(if chunk == 0 { usize::MAX } else { chunk });
            output[..count].copy_from_slice(&self.data[self.offset..self.offset + count]);
            self.offset += count;
            let mut observation = self.observation.lock().unwrap();
            observation.reads += 1;
            observation.bytes += count;
            drop(observation);
            if self.input["cancel_at"] == "response_read" {
                self.context.cancel();
            }
            let at_end = self.offset == self.data.len()
                && (count == 0 || self.input["error_with_last_bytes"] == true);
            let fault = self.input["response_read_error"].as_str().unwrap();
            RawRead {
                count,
                eof: at_end && (fault == "EOF" || (fault.is_empty() && count == 0)),
                error: (at_end && !fault.is_empty() && fault != "EOF")
                    .then(|| std::io::Error::other("synthetic-upstream-error-marker").into()),
            }
        })
    }
    fn close(&mut self) -> BodyFuture<'_, Result<(), ExternalError>> {
        Box::pin(async {
            self.observation.lock().unwrap().closes += 1;
            if self.input["cancel_at"] == "response_close" {
                self.context.cancel();
            }
            if self.input["response_close_error"] == true {
                return Err(std::io::Error::other("synthetic-upstream-error-marker").into());
            }
            Ok(())
        })
    }
}

pub struct StreamingTransport {
    pub image_size: usize,
    pub bytes_seen: AtomicUsize,
}
impl HttpTransport for StreamingTransport {
    fn send(
        &self,
        mut request: HttpRequest,
    ) -> ReverseFuture<'_, Result<Option<RawResponse>, ExternalError>> {
        Box::pin(async move {
            let content_type = &request.headers["Content-Type"][0];
            let boundary = content_type
                .strip_prefix("multipart/form-data; boundary=")
                .unwrap();
            let fixture: Value = serde_json::from_str(include_str!(
                "../../../pixiv-cli/tests/fixtures/reverse-saucenao.json"
            ))
            .unwrap();
            let row = fixture["cases"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["name"] == "multipart/default_endpoint")
                .unwrap();
            let mut prefix = Vec::new();
            for (index, part) in row["observation"]["requests"][0]["content_type"]["parts"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
            {
                if index != 0 {
                    prefix.extend_from_slice(b"\r\n");
                }
                prefix.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
                for (name, values) in part["headers"].as_object().unwrap() {
                    for value in values.as_array().unwrap() {
                        prefix.extend_from_slice(
                            format!("{name}: {}\r\n", value.as_str().unwrap()).as_bytes(),
                        );
                    }
                }
                prefix.extend_from_slice(b"\r\n");
                if index != 3 {
                    prefix.extend_from_slice(&bytes(&part["payload"]));
                }
            }
            let suffix = format!("\r\n--{boundary}--\r\n").into_bytes();
            let mut buffer = [0; 32 * 1024];
            let mut offset = 0;
            loop {
                let count = request.body.as_mut().unwrap().read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                for (index, byte) in buffer[..count].iter().enumerate() {
                    let position = offset + index;
                    let expected = if position < prefix.len() {
                        prefix[position]
                    } else if position < prefix.len() + self.image_size {
                        b'x'
                    } else {
                        suffix[position - prefix.len() - self.image_size]
                    };
                    assert_eq!(*byte, expected, "streaming multipart at offset {position}");
                }
                offset += count;
            }
            assert_eq!(offset, prefix.len() + self.image_size + suffix.len());
            self.bytes_seen.store(offset, Ordering::SeqCst);
            let mut input = row["input"].clone();
            input["response"] = json!(
                "{\"header\":{\"status\":0,\"short_remaining\":3,\"long_remaining\":97,\"short_limit\":4,\"long_limit\":100},\"results\":[]}"
            );
            let data = input["response"].as_str().unwrap().as_bytes().to_vec();
            Ok(Some(RawResponse {
                status: 200,
                headers: Default::default(),
                content_length: data.len() as i64,
                body: Some(Box::new(Body {
                    input,
                    data,
                    offset: 0,
                    observation: Arc::new(Mutex::new(BodyObservation::default())),
                    context: Arc::new(Context::new()),
                })),
            }))
        })
    }
}
