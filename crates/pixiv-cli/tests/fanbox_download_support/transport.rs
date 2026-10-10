use super::{
    Shared,
    schema::{Input, Reply},
    trace,
};
use pixiv_app::lifecycle::Context;
use pixiv_sdk::fanbox::transport::{
    BodyFuture, ExternalError, RawBody, RawRead, RawRequest, RawResponse, RawTransport,
    TransportFuture,
};
use serde_json::json;
use std::{io, sync::Mutex};

pub struct Transport {
    pub input: Input,
    pub observed: Shared,
    pub context: Context,
    pub index: Mutex<usize>,
}
impl RawTransport for Transport {
    fn send(
        &self,
        request: RawRequest,
    ) -> TransportFuture<'_, Result<Option<RawResponse>, ExternalError>> {
        Box::pin(async move {
            self.observed.lock().unwrap().requests.push(json!({"method":request.method,"url":request.url,"headers":request.headers,"context_error":request.context.error().map(|error| error.to_string()).unwrap_or_default()}));
            trace(&self.observed, format!("request:{}", request.url));
            let url = url::Url::parse(&request.url).unwrap();
            let cookie = request
                .headers
                .get("Cookie")
                .and_then(|values| values.first())
                .map(String::as_str)
                .unwrap_or_default();
            match url.host_str().unwrap() {
                "api.fanbox.cc" | "downloads.fanbox.cc" => assert!(
                    matches!(
                        cookie,
                        "FANBOXSESSID=owned-session-42" | "FANBOXSESSID=owned-session-7"
                    ),
                    "unexpected selected session"
                ),
                "i.pximg.net" | "pixiv.pximg.net" => {
                    assert_eq!(cookie, "", "credential reached public media")
                }
                host => panic!("unexpected owned download destination: {host}"),
            }
            let reply = {
                let mut index = self.index.lock().unwrap();
                let reply = self
                    .input
                    .replies
                    .get(*index)
                    .cloned()
                    .expect("owned response sequence exhausted");
                *index += 1;
                self.observed.lock().unwrap().reply_count = *index;
                reply
            };
            if !reply.url.is_empty() {
                assert_eq!(request.url, reply.url, "owned expected request");
            }
            if self.input.cancel == "transport" {
                self.context.cancel();
                return Err(external(request.context.error().unwrap().to_string()));
            }
            if !reply.transport_error.is_empty() {
                return Err(external(&reply.transport_error));
            }
            let content_type = if reply.content_type.is_empty() {
                "application/json"
            } else {
                &reply.content_type
            };
            let mut headers = std::collections::BTreeMap::from([(
                "Content-Type".into(),
                vec![content_type.to_owned()],
            )]);
            if reply.content_length != 0 {
                headers.insert(
                    "Content-Length".into(),
                    vec![reply.content_length.to_string()],
                );
            }
            Ok(Some(RawResponse {
                status: if reply.status == 0 { 200 } else { reply.status },
                headers,
                content_length: reply.content_length,
                body: Some(Box::new(Body {
                    data: reply.body.as_bytes().to_vec(),
                    offset: 0,
                    reads: 0,
                    failed: false,
                    reply,
                    url: request.url,
                    observed: self.observed.clone(),
                    context: self.context.clone(),
                })),
            }))
        })
    }
    fn close_idle_connections(&self) {
        self.observed.lock().unwrap().idle_closes += 1;
        trace(&self.observed, "transport.idle-close");
    }
}
fn external(message: impl Into<String>) -> ExternalError {
    Box::new(io::Error::other(message.into()))
}
struct Body {
    data: Vec<u8>,
    offset: usize,
    reads: usize,
    failed: bool,
    reply: Reply,
    url: String,
    observed: Shared,
    context: Context,
}
impl RawBody for Body {
    fn read<'a>(&'a mut self, output: &'a mut [u8]) -> BodyFuture<'a, RawRead> {
        Box::pin(async move {
            let capacity = output.len();
            self.reads += 1;
            let limit = if self.reply.chunk_size == 0 {
                capacity
            } else {
                capacity.min(self.reply.chunk_size)
            };
            let failure = !self.reply.read_error.is_empty() && !self.failed;
            if failure {
                self.failed = true;
            }
            let count = if failure && !self.reply.bytes_and_error {
                0
            } else {
                limit.min(self.data.len() - self.offset)
            };
            output[..count].copy_from_slice(&self.data[self.offset..self.offset + count]);
            self.offset += count;
            let eof = !failure
                && (count == 0 || self.reply.bytes_and_eof && self.offset == self.data.len());
            let message = if failure {
                self.reply.read_error.clone()
            } else if eof {
                "EOF".into()
            } else {
                String::new()
            };
            self.observed
                .lock()
                .unwrap()
                .body_reads
                .push(json!({"url":self.url,"capacity":capacity,"n":count,"error":message}));
            trace(&self.observed, format!("body.read:{}", self.url));
            if self.reply.cancel_at_read == self.reads {
                self.context.cancel();
            }
            RawRead {
                count,
                eof,
                error: if failure {
                    Some(external(&self.reply.read_error))
                } else {
                    None
                },
            }
        })
    }
    fn close(&mut self) -> BodyFuture<'_, Result<(), ExternalError>> {
        Box::pin(async move {
            self.observed.lock().unwrap().body_closes += 1;
            trace(&self.observed, format!("body.close:{}", self.url));
            if self.reply.close_error.is_empty() {
                Ok(())
            } else {
                Err(external(&self.reply.close_error))
            }
        })
    }
}
