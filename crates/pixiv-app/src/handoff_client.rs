mod http;
mod json;

use crate::{
    handoff_protocol::{self, HandoffProtocolError, RELAY_RESULT_URL_HEADER, RemoteLoginStart},
    handoff_state::{ActiveRemoteLogin, HandoffState, HandoffStateError},
};
pub use http::{HandoffProxyEnvironment, HandoffProxyError, NativeHandoffTransport};
use std::{
    error::Error,
    fmt,
    future::Future,
    io,
    pin::Pin,
    sync::atomic::{AtomicBool, Ordering},
};
use tokio_util::sync::CancellationToken;

pub type HandoffFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub struct HandoffRequest {
    pub method: String,
    pub endpoint: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

pub struct HandoffRead {
    pub count: usize,
    pub error: Option<io::Error>,
}

pub trait HandoffBody: Send + Sync {
    fn read<'a>(&'a self, buffer: &'a mut [u8]) -> HandoffFuture<'a, HandoffRead>;
    fn close(&self);
}

pub struct HandoffResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Box<dyn HandoffBody>,
}

#[derive(Debug, Clone, Copy)]
pub struct HandoffTransportError;
impl fmt::Display for HandoffTransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("could not contact remote Pixiv login relay")
    }
}
impl Error for HandoffTransportError {}

pub trait HandoffTransport: Send + Sync {
    fn send<'a>(
        &'a self,
        request: HandoffRequest,
        cancellation: CancellationToken,
    ) -> HandoffFuture<'a, Result<HandoffResponse, HandoffTransportError>>;
}

#[derive(Debug)]
pub enum HandoffClientError {
    Protocol(HandoffProtocolError),
    State(HandoffStateError),
    Message(&'static str),
}
impl fmt::Display for HandoffClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Protocol(error) => error.fmt(f),
            Self::State(error) => error.fmt(f),
            Self::Message(message) => f.write_str(message),
        }
    }
}
impl Error for HandoffClientError {}
impl From<HandoffProtocolError> for HandoffClientError {
    fn from(error: HandoffProtocolError) -> Self {
        Self::Protocol(error)
    }
}
impl From<HandoffStateError> for HandoffClientError {
    fn from(error: HandoffStateError) -> Self {
        Self::State(error)
    }
}

pub struct HandoffClient<T> {
    transport: T,
    state: HandoffState,
}
impl<T: HandoffTransport> HandoffClient<T> {
    pub fn new(transport: T, state: HandoffState) -> Self {
        Self { transport, state }
    }

    pub async fn start(
        &self,
        start: &RemoteLoginStart,
        cancellation: &CancellationToken,
    ) -> Result<String, HandoffClientError> {
        let origin = handoff_protocol::canonical_relay_origin(&start.origin)
            .map_err(|_| HandoffClientError::Message("invalid remote login start request"))?;
        if go_trim(&start.session_id).is_empty() || go_trim(&start.proof).is_empty() {
            return Err(HandoffClientError::Message(
                "invalid remote login start request",
            ));
        }
        let endpoint = handoff_protocol::relay_endpoint_url(&origin, "start", &start.session_id)?;
        let body = format!("{{\"proof\":{}}}", json_quote(&start.proof)).into_bytes();
        let response = self
            .transport
            .send(request(endpoint, body), cancellation.clone())
            .await
            .map_err(|_| {
                HandoffClientError::Message("could not contact remote Pixiv login relay")
            })?;
        let body = OwnedBody::new(response.body);
        if response.status != 200 {
            return Err(HandoffClientError::Message(
                "remote Pixiv login relay rejected login handoff",
            ));
        }
        let invalid = || {
            HandoffClientError::Message(
                "remote Pixiv login relay returned an invalid sign-in address",
            )
        };
        let mut decoder = json::Decoder::new(&body);
        let first = decoder
            .next()
            .await
            .map_err(|_| invalid())?
            .ok_or_else(invalid)?;
        let address = json::authorization(&first).map_err(|_| invalid())?;
        if decoder.next().await.map_err(|_| invalid())?.is_some() {
            return Err(invalid());
        }
        handoff_protocol::validate_authorization_url(&address)?;
        self.state.save(&ActiveRemoteLogin {
            version: 1,
            origin,
            session_id: start.session_id.clone(),
            proof: start.proof.clone(),
        })?;
        Ok(address)
    }

    pub async fn forward_callback(
        &self,
        callback: &str,
        cancellation: &CancellationToken,
    ) -> Result<RemoteCallbackSession, HandoffClientError> {
        if !handoff_protocol::is_allowed_pixiv_callback_url(callback) {
            return Err(HandoffClientError::Message(
                "this Pixiv login link cannot be used for remote sign-in",
            ));
        }
        let active = self.state.load()?;
        let endpoint =
            handoff_protocol::relay_endpoint_url(&active.origin, "callback", &active.session_id)?;
        let body = format!(
            "{{\"callback_url\":{},\"proof\":{}}}",
            json_quote(callback),
            json_quote(&active.proof)
        )
        .into_bytes();
        let response = self
            .transport
            .send(request(endpoint, body), cancellation.clone())
            .await
            .map_err(|_| {
                HandoffClientError::Message("could not contact remote Pixiv login relay")
            })?;
        let body = OwnedBody::new(response.body);
        if response.status != 200 {
            return Err(HandoffClientError::Message(
                "remote Pixiv login relay rejected the login result",
            ));
        }
        let result_url = response
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(RELAY_RESULT_URL_HEADER))
            .map(|(_, value)| go_trim(value))
            .unwrap_or("")
            .to_owned();
        handoff_protocol::validate_relay_result_url(&active.origin, &result_url)?;
        self.state.clear_if_matches(&active).map_err(|_| {
            HandoffClientError::Message("could not clear active remote login handoff")
        })?;
        Ok(RemoteCallbackSession {
            result_url,
            body: Some(body),
        })
    }
}

fn request(endpoint: String, body: Vec<u8>) -> HandoffRequest {
    HandoffRequest {
        method: "POST".into(),
        endpoint,
        headers: vec![("Content-Type".into(), "application/json".into())],
        body,
    }
}

#[derive(Default)]
pub struct RemoteCallbackSession {
    pub result_url: String,
    body: Option<OwnedBody>,
}
impl RemoteCallbackSession {
    pub async fn complete(&self) -> Result<(), HandoffClientError> {
        let body = self.body.as_ref().ok_or(HandoffClientError::Message(
            "remote Pixiv login relay session is unavailable",
        ))?;
        let _close = CloseOnDrop(body);
        let mut decoder = json::Decoder::new(body);
        let first = decoder
            .next()
            .await
            .map_err(|_| {
                HandoffClientError::Message(
                    "remote Pixiv login relay did not return a final result",
                )
            })?
            .ok_or(HandoffClientError::Message(
                "remote Pixiv login relay did not return a final result",
            ))?;
        let success = json::completion(&first).map_err(|_| {
            HandoffClientError::Message("remote Pixiv login relay did not return a final result")
        })?;
        if decoder
            .next()
            .await
            .map_err(|_| {
                HandoffClientError::Message(
                    "remote Pixiv login relay returned an invalid final result",
                )
            })?
            .is_some()
        {
            return Err(HandoffClientError::Message(
                "remote Pixiv login relay returned an invalid final result",
            ));
        }
        if !success {
            return Err(HandoffClientError::Message(
                "remote Pixiv login relay reported that login failed",
            ));
        }
        Ok(())
    }
    pub fn abort(&self) {
        if let Some(body) = &self.body {
            body.close();
        }
    }
}

struct OwnedBody {
    body: Box<dyn HandoffBody>,
    closed: AtomicBool,
}
impl OwnedBody {
    fn new(body: Box<dyn HandoffBody>) -> Self {
        Self {
            body,
            closed: AtomicBool::new(false),
        }
    }
    fn close(&self) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            self.body.close();
        }
    }
}
impl Drop for OwnedBody {
    fn drop(&mut self) {
        self.close();
    }
}
struct CloseOnDrop<'a>(&'a OwnedBody);
impl Drop for CloseOnDrop<'_> {
    fn drop(&mut self) {
        self.0.close();
    }
}

fn go_trim(value: &str) -> &str {
    value.trim_matches(|c| matches!(c, '\u{0009}'..='\u{000d}' | ' ' | '\u{0085}' | '\u{00a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}'))
}

fn json_quote(value: &str) -> String {
    crate::auth_bundle::escape_json_html(
        serde_json::to_string(value).expect("string JSON encoding is infallible"),
    )
}
