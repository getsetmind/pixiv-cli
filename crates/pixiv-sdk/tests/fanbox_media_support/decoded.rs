use base64::{Engine, engine::general_purpose::STANDARD};
use pixiv_sdk::fanbox::transport::{BodyFuture, ExternalError, RawBody, RawRead};
use serde_json::{Value, json};
use std::{
    io,
    sync::{Arc, Mutex},
};

pub struct Source {
    spec: Value,
    wire: Vec<u8>,
    position: usize,
    pub ownership: Arc<Mutex<Value>>,
}
impl Source {
    pub fn new(spec: &Value) -> Self {
        Self {
            spec: spec.clone(),
            wire: match spec["wire"].as_str() {
                Some(wire) => STANDARD.decode(wire).unwrap(),
                None if spec["nil_body"] == true => Vec::new(),
                None => panic!("only the frozen nil-body row can have a null wire"),
            },
            position: 0,
            ownership: Arc::new(Mutex::new(json!({"bytes_read":0,"reads":[],"closes":0}))),
        }
    }
}
impl RawBody for Source {
    fn read<'a>(&'a mut self, output: &'a mut [u8]) -> BodyFuture<'a, RawRead> {
        Box::pin(async move {
            let chunk = self.spec["chunk"].as_u64().unwrap() as usize;
            let count = output
                .len()
                .min(self.wire.len() - self.position)
                .min(if chunk == 0 { usize::MAX } else { chunk });
            output[..count].copy_from_slice(&self.wire[self.position..self.position + count]);
            self.position += count;
            let terminal = self.position == self.wire.len()
                && (count == 0 || self.spec["error_with_last_bytes"] == true);
            let kind = self.spec["read_error"].as_str().unwrap();
            let error = (terminal && !kind.is_empty()).then(|| -> ExternalError {
                match kind {
                    "canceled" => Box::new(pixiv_sdk::context::ContextError::Canceled),
                    "deadline" => Box::new(pixiv_sdk::context::ContextError::DeadlineExceeded),
                    _ => Box::new(io::Error::other("synthetic transport/body failure canary")),
                }
            });
            let eof = terminal && kind.is_empty();
            let mut ownership = self.ownership.lock().unwrap();
            ownership["bytes_read"] = json!(self.position);
            ownership["reads"].as_array_mut().unwrap().push(json!({
                "requested":output.len(),"returned":count,"eof":eof,
                "error":error.as_ref().map(ToString::to_string).unwrap_or_default()
            }));
            RawRead { count, eof, error }
        })
    }
    fn close(&mut self) -> BodyFuture<'_, Result<(), ExternalError>> {
        Box::pin(async move {
            let mut ownership = self.ownership.lock().unwrap();
            ownership["closes"] = json!(ownership["closes"].as_u64().unwrap() + 1);
            if self.spec["close_error"] == true {
                Err(
                    Box::new(io::Error::other("synthetic transport/body failure canary"))
                        as ExternalError,
                )
            } else {
                Ok(())
            }
        })
    }
}

pub fn ownership_projection(ownership: &Value) -> Value {
    json!({"bytes_read":ownership["bytes_read"],"read_calls":ownership["reads"].as_array().unwrap().len(),"closes":ownership["closes"]})
}

pub async fn action(body: &mut dyn RawBody, action: &Value) -> (Vec<u8>, String) {
    match action["kind"].as_str().unwrap() {
        "read" => {
            let mut output = vec![0; action["size"].as_u64().unwrap() as usize];
            let read = body.read(&mut output).await;
            assert!(read.count <= output.len());
            output.truncate(read.count);
            (
                output,
                read.error
                    .map(|error| error.to_string())
                    .unwrap_or_else(|| {
                        if read.eof {
                            "EOF".into()
                        } else {
                            String::new()
                        }
                    }),
            )
        }
        "read_all" => {
            let mut bytes = Vec::new();
            loop {
                let mut output = [0; 512];
                let read = body.read(&mut output).await;
                assert!(read.count <= output.len());
                bytes.extend_from_slice(&output[..read.count]);
                if let Some(error) = read.error {
                    return (bytes, error.to_string());
                }
                if read.eof {
                    return (bytes, String::new());
                }
                assert!(read.count > 0, "fixture decoder stalled");
            }
        }
        "close" => (
            Vec::new(),
            body.close()
                .await
                .err()
                .map(|error| error.to_string())
                .unwrap_or_default(),
        ),
        other => panic!("unsupported action {other}"),
    }
}

pub fn raw_action_error(operations: &[Value], index: &mut usize, kind: &str) -> String {
    if kind == "mutate_dependency_metadata" {
        return String::new();
    }
    loop {
        let operation = &operations[*index];
        *index += 1;
        let error = operation["go_only_dependency_error"]["message"]
            .as_str()
            .unwrap();
        if kind != "read_all" || !error.is_empty() {
            return if kind == "read_all" && error == "EOF" {
                String::new()
            } else {
                error.into()
            };
        }
    }
}
