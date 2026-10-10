use super::{Shared, context_marker, io::encode_hex, schema::Step, trace};
use pixiv_app::lifecycle::Context;
use pixiv_sdk::fanbox::transport::{
    BodyFuture, ExternalError, RawBody, RawRead, RawRequest, RawResponse, RawTransport,
    TransportFuture,
};
use serde_json::json;
use std::{
    io::{self, Cursor, Read},
    sync::Arc,
};

pub struct Transport {
    pub step: Step,
    pub observed: Shared,
    pub context: Context,
}
impl RawTransport for Transport {
    fn send(
        &self,
        request: RawRequest,
    ) -> TransportFuture<'_, Result<Option<RawResponse>, ExternalError>> {
        Box::pin(async move {
            let header = |name: &str| {
                request
                    .headers
                    .iter()
                    .find(|(key, _)| key.eq_ignore_ascii_case(name))
                    .and_then(|(_, values)| values.first())
                    .map(String::as_str)
                    .unwrap_or("")
            };
            {
                let mut observed = self.observed.lock().unwrap();
                observed.trace.push("identity.request".into());
                observed.requests.push(json!({
                    "method":request.method,"url":request.url,
                    "cookie_hex":encode_hex(header("Cookie").as_bytes()),
                    "user_agent":header("User-Agent"),"accept":header("Accept"),
                    "context":context_marker(request.context.as_ref()),
                }));
            }
            if self.step.cancel_on_request {
                self.context.cancel();
            }
            if self.step.identity == "transport" {
                return Err(
                    Box::new(io::Error::other("owned fixture transport failure")) as ExternalError,
                );
            }
            if let Some(error) = request.context.error() {
                return Err(Box::new(error) as ExternalError);
            }
            let id = if self.step.user_id == 0 {
                42
            } else {
                self.step.user_id
            };
            let name = if self.step.display_name.is_empty() && !self.step.empty_identity_fields {
                "  verified fixture  "
            } else {
                &self.step.display_name
            };
            let creator = if self.step.creator_id.is_empty() && !self.step.empty_identity_fields {
                "fixture-creator"
            } else {
                &self.step.creator_id
            };
            let metadata = match self.step.identity.as_str() {
                "zero" => "{\"context\":{\"user\":{\"userId\":0}}}".into(),
                "malformed" => "{".into(),
                _ => json!({"context":{"user":{"userId":id,"name":name,"creatorId":creator}}})
                    .to_string(),
            };
            let body = if self.step.identity == "missing" {
                "<html></html>".into()
            } else {
                format!("<html><head><meta name=\"metadata\" content='{metadata}'></head></html>")
            };
            let status = match self.step.identity.as_str() {
                "unauthorized" => 401,
                "forbidden" => 403,
                _ => 200,
            };
            Ok(Some(RawResponse {
                status,
                headers: std::collections::BTreeMap::from([(
                    "Content-Type".into(),
                    vec!["text/html".into()],
                )]),
                content_length: -1,
                body: Some(Box::new(Body {
                    bytes: Cursor::new(body.into_bytes()),
                    mode: self.step.identity.clone(),
                    observed: self.observed.clone(),
                })),
            }))
        })
    }
    fn close_idle_connections(&self) {
        trace(&self.observed, "identity.idle.close");
    }
}

struct Body {
    bytes: Cursor<Vec<u8>>,
    mode: String,
    observed: Shared,
}
impl RawBody for Body {
    fn read<'a>(&'a mut self, output: &'a mut [u8]) -> BodyFuture<'a, RawRead> {
        Box::pin(async move {
            if self.mode == "read" {
                return RawRead {
                    count: 0,
                    eof: false,
                    error: Some(Box::new(io::Error::other(
                        "owned fixture identity read failure",
                    ))),
                };
            }
            let count = self.bytes.read(output).unwrap();
            RawRead {
                count,
                eof: count == 0,
                error: None,
            }
        })
    }
    fn close(&mut self) -> BodyFuture<'_, Result<(), ExternalError>> {
        Box::pin(async move {
            trace(&self.observed, "identity.body.close");
            if self.mode == "close" {
                return Err(
                    Box::new(io::Error::other("owned fixture identity close failure"))
                        as ExternalError,
                );
            }
            Ok(())
        })
    }
}

pub fn injected_client(
    step: &Step,
    observed: &Shared,
    context: &Context,
    session: &str,
) -> pixiv_app::sessions::ClientOpen<pixiv_sdk::fanbox::Client> {
    trace(observed, "session.open.injected");
    let options = pixiv_sdk::fanbox::Options {
        user_agent: "fixture-auth-agent".into(),
        http_client: Some(Arc::new(Transport {
            step: step.clone(),
            observed: observed.clone(),
            context: context.clone(),
        })),
        ..Default::default()
    };
    match pixiv_sdk::fanbox::Client::open_with(
        pixiv_sdk::fanbox::SessionCredentials {
            fanbox_sessid: session.into(),
        },
        options,
    ) {
        Ok(client) => pixiv_app::sessions::ClientOpen {
            client: Some(Arc::new(client)),
            error: None,
        },
        Err(error) => pixiv_app::sessions::ClientOpen {
            client: None,
            error: Some(error.into()),
        },
    }
}
