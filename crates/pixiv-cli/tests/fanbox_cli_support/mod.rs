use pixiv_sdk::fanbox::transport::{
    BodyFuture, ExternalError, RawBody, RawRead, RawRequest, RawResponse, RawTransport,
    TransportFuture,
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

pub struct Transport {
    pub replies: Vec<Value>,
    pub requests: Mutex<Vec<Value>>,
    index: Mutex<usize>,
    pub closes: Arc<Mutex<usize>>,
}
impl Transport {
    pub fn new(replies: Vec<Value>) -> Self {
        Self {
            replies,
            requests: Mutex::new(vec![]),
            index: Mutex::new(0),
            closes: Arc::new(Mutex::new(0)),
        }
    }
}
impl RawTransport for Transport {
    fn send(
        &self,
        request: RawRequest,
    ) -> TransportFuture<'_, Result<Option<RawResponse>, ExternalError>> {
        Box::pin(async move {
            self.requests.lock().unwrap().push(json!({"method":request.method,"url":request.url,"headers":request.headers,"context_error":request.context.error().map(|e|e.to_string()).unwrap_or_default()}));
            let reply = if request.url == "https://www.fanbox.cc/" {
                json!({"body":"<html><head><meta name=\"metadata\" content='{\"context\":{\"user\":{\"userId\":42,\"name\":\"owned user\"}}}'></head></html>"})
            } else {
                let mut index = self.index.lock().unwrap();
                let reply = self.replies.get(*index).cloned().ok_or_else(|| {
                    Box::new(std::io::Error::other("owned response sequence exhausted"))
                        as ExternalError
                })?;
                *index += 1;
                reply
            };
            Ok(Some(RawResponse {
                status: reply["status"].as_u64().filter(|v| *v != 0).unwrap_or(200) as u16,
                headers: [("Content-Type".into(), vec!["application/json".into()])].into(),
                content_length: -1,
                body: Some(Box::new(Body {
                    data: reply["body"]
                        .as_str()
                        .unwrap_or_default()
                        .as_bytes()
                        .to_vec(),
                    offset: 0,
                    closes: self.closes.clone(),
                })),
            }))
        })
    }
}
struct Body {
    data: Vec<u8>,
    offset: usize,
    closes: Arc<Mutex<usize>>,
}
impl RawBody for Body {
    fn read<'a>(&'a mut self, output: &'a mut [u8]) -> BodyFuture<'a, RawRead> {
        Box::pin(async move {
            let count = output.len().min(self.data.len() - self.offset);
            output[..count].copy_from_slice(&self.data[self.offset..self.offset + count]);
            self.offset += count;
            RawRead {
                count,
                eof: count == 0,
                error: None,
            }
        })
    }
    fn close(&mut self) -> BodyFuture<'_, Result<(), ExternalError>> {
        Box::pin(async move {
            *self.closes.lock().unwrap() += 1;
            Ok(())
        })
    }
}
