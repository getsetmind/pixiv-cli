use super::{CallerContext, ExternalError, check_context, go_quote, join, message, wrap};
use crate::{
    handoff_client::go_trim,
    reverse_search::http::{HttpRequest, HttpTransport},
};
use futures_util::{FutureExt, StreamExt, stream::FuturesUnordered};
use pixiv_sdk::{
    context::{ContextError, ContextFuture, ContextKey, ContextValue, RequestContext},
    diagnostics::Scope,
    fanbox::transport::{Headers, RawBody},
    oauth::LoginUrl,
};
use serde_json::Value;
use std::{any::TypeId, collections::HashSet, fmt, sync::Arc, time::Instant};
use tokio_util::sync::CancellationToken;

pub const GITHUB_USER_AGENT: &str = "pixiv-cli";
const EMBEDDED_SOURCES: &[u8] =
    include_bytes!("../../../../internal/update/source/release_sources.txt");
const URL_PLACEHOLDER: &str = "{url}";
const QUERY_PLACEHOLDER: &str = "{url_query}";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseSource {
    id: String,
    api: SourceTemplate,
    asset: SourceTemplate,
}
impl ReleaseSource {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn api_url(&self, canonical: &str) -> Result<String, ExternalError> {
        if self.api.raw.is_empty() {
            return Err(message(format!(
                "release source {} does not support GitHub Releases API",
                go_quote(&self.id)
            )));
        }
        self.api.apply(canonical)
    }
    pub fn asset_url(&self, canonical: &str) -> Result<String, ExternalError> {
        self.asset.apply(canonical)
    }
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct SourceTemplate {
    raw: String,
    placeholder: &'static str,
}
impl SourceTemplate {
    fn parse(value: &str) -> Result<Self, ExternalError> {
        if value == "-" {
            return Ok(Self::default());
        }
        let mut placeholder = "";
        for candidate in [URL_PLACEHOLDER, QUERY_PLACEHOLDER] {
            let count = value.matches(candidate).count();
            if count == 0 {
                continue;
            }
            if count != 1 || !placeholder.is_empty() {
                return Err(message("must contain exactly one URL placeholder"));
            }
            placeholder = candidate;
        }
        if placeholder.is_empty() {
            return Err(message("must contain exactly one URL placeholder"));
        }
        let template = Self {
            raw: value.into(),
            placeholder,
        };
        template.apply("https://github.com/FlanChanXwO/pixiv-cli/releases")?;
        Ok(template)
    }
    fn apply(&self, canonical: &str) -> Result<String, ExternalError> {
        let parsed =
            parse_url(canonical).map_err(|error| wrap("parse canonical release URL", error))?;
        if !parsed.scheme().eq_ignore_ascii_case("https")
            || parsed.host().is_empty()
            || parsed.has_userinfo()
            || !parsed.fragment().is_empty()
        {
            return Err(message(format!(
                "canonical release URL {} must be an absolute HTTPS URL without userinfo or fragment",
                go_quote(canonical)
            )));
        }
        let replacement = if self.placeholder == QUERY_PLACEHOLDER {
            query_escape(canonical)
        } else {
            canonical.into()
        };
        let value = self.raw.replacen(self.placeholder, &replacement, 1);
        let transformed =
            parse_url(&value).map_err(|error| wrap("parse transformed release URL", error))?;
        if !matches!(
            transformed.scheme().to_ascii_lowercase().as_str(),
            "http" | "https"
        ) || transformed.host().is_empty()
            || transformed.has_userinfo()
            || !transformed.fragment().is_empty()
        {
            return Err(message(format!(
                "transformed release URL {} must be an absolute HTTP(S) URL without userinfo or fragment",
                go_quote(&value)
            )));
        }
        Ok(value)
    }
}
pub fn parse_release_sources(body: &[u8]) -> Result<Vec<ReleaseSource>, ExternalError> {
    let text = String::from_utf8_lossy(body);
    let mut sources = vec![];
    let mut seen = HashSet::new();
    for (index, line) in text.split('\n').enumerate() {
        let line = go_trim(line);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields = line.split('|').collect::<Vec<_>>();
        let number = index + 1;
        if fields.len() != 3 {
            return Err(message(format!(
                "release source line {number} must have id, API template, and asset template"
            )));
        }
        let id = go_trim(fields[0]);
        if !valid_id(id) {
            return Err(message(format!(
                "release source line {number} has invalid ID {}",
                go_quote(id)
            )));
        }
        if seen.contains(id) {
            return Err(message(format!(
                "release source line {number} repeats ID {}",
                go_quote(id)
            )));
        }
        let api = SourceTemplate::parse(go_trim(fields[1]))
            .map_err(|error| wrap(format!("release source line {number} API template"), error))?;
        let asset = SourceTemplate::parse(go_trim(fields[2])).map_err(|error| {
            wrap(
                format!("release source line {number} asset template"),
                error,
            )
        })?;
        if asset.raw.is_empty() {
            return Err(message(format!(
                "release source line {number} must provide an asset template"
            )));
        }
        seen.insert(id.to_owned());
        sources.push(ReleaseSource {
            id: id.into(),
            api,
            asset,
        });
    }
    if sources.is_empty() {
        return Err(message("release source list is empty"));
    }
    Ok(sources)
}
pub fn default_release_sources() -> Vec<ReleaseSource> {
    parse_release_sources(EMBEDDED_SOURCES).expect("valid embedded release sources")
}
pub fn first_release_source(sources: &[ReleaseSource]) -> Option<&ReleaseSource> {
    sources.first()
}
fn valid_id(value: &str) -> bool {
    value
        .bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b".-".contains(&byte))
}
fn query_escape(value: &str) -> String {
    let mut result = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(char::from(byte))
            }
            b' ' => result.push('+'),
            _ => result.push_str(&format!("%{byte:02X}")),
        }
    }
    result
}
fn parse_url(value: &str) -> Result<LoginUrl<'_>, ExternalError> {
    if let Some(parsed) = LoginUrl::parse(value) {
        return Ok(parsed);
    }
    let error = if value.bytes().any(|byte| byte < 32 || byte == 127) {
        "net/url: invalid control character in URL".into()
    } else {
        let (without_fragment, fragment) = value.split_once('#').unwrap_or((value, ""));
        let without_query = without_fragment
            .split_once('?')
            .map_or(without_fragment, |(before, _)| before);
        [without_query, fragment]
            .into_iter()
            .find_map(invalid_escape)
            .unwrap_or_else(|| "invalid URL".into())
    };
    Err(wrap(format!("parse {}", go_quote(value)), message(error)))
}
fn invalid_escape(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit()
            {
                return Some(format!(
                    "invalid URL escape {}",
                    go_quote(&String::from_utf8_lossy(
                        &bytes[index..(index + 3).min(bytes.len())]
                    ))
                ));
            }
            index += 2;
        }
        index += 1;
    }
    None
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReleaseSourceKind {
    API,
    Asset,
    Unknown(String),
}
impl fmt::Display for ReleaseSourceKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::API => "GitHub Releases API",
            Self::Asset => "release asset",
            Self::Unknown(value) => value,
        })
    }
}
pub struct ReleaseSourceSelector {
    sources: Vec<ReleaseSource>,
    transport: Arc<dyn HttpTransport>,
}
impl ReleaseSourceSelector {
    pub fn new(sources: Vec<ReleaseSource>, transport: Arc<dyn HttpTransport>) -> Self {
        Self { sources, transport }
    }
    pub fn default(transport: Arc<dyn HttpTransport>) -> Self {
        Self::new(default_release_sources(), transport)
    }
    pub async fn ordered(
        &self,
        context: CallerContext,
        kind: ReleaseSourceKind,
        canonical_url: &str,
    ) -> Result<Vec<ReleaseSource>, ExternalError> {
        check_context(&context, "select release source")?;
        let candidates = self
            .sources
            .iter()
            .filter(|source| {
                if kind == ReleaseSourceKind::API {
                    !source.api.raw.is_empty()
                } else {
                    !source.asset.raw.is_empty()
                }
            })
            .cloned()
            .collect::<Vec<_>>();
        if candidates.is_empty() {
            return Err(message(format!("no release source supports {kind}")));
        }
        let child = Arc::new(ProbeContext {
            parent: context.clone(),
            cancel: CancellationToken::new(),
        });
        let cancellation = CancelProbes(child.cancel.clone());
        let probe_context: CallerContext = child.clone();
        let mut probes = FuturesUnordered::new();
        for source in &candidates {
            let source = source.clone();
            let context = probe_context.clone();
            let kind = kind.clone();
            probes.push(async move {
                let result = self.probe(context, &source, &kind, canonical_url).await;
                (source, result)
            });
        }
        let mut errors = vec![];
        while !probes.is_empty() {
            let result = tokio::select! {biased;error=context.cancelled()=>{
             child.cancel.cancel();while let Some(Some(_)) = probes.next().now_or_never() {}drop(probes);return Err(wrap("select release source",Box::new(error)));
            },result=probes.next()=>result};
            let Some((source, result)) = result else {
                break;
            };
            match result {
                Ok(()) => {
                    child.cancel.cancel();
                    while let Some(Some(_)) = probes.next().now_or_never() {}
                    drop(probes);
                    let mut ordered = vec![source.clone()];
                    ordered.extend(
                        candidates
                            .into_iter()
                            .filter(|candidate| candidate.id != source.id),
                    );
                    return Ok(ordered);
                }
                Err(error) => errors.push(wrap(
                    format!("release source {}", go_quote(&source.id)),
                    error,
                )),
            }
        }
        drop(cancellation);
        Err(wrap(
            format!("no release source returned a valid {kind} response"),
            join(errors),
        ))
    }
    async fn probe(
        &self,
        context: CallerContext,
        source: &ReleaseSource,
        kind: &ReleaseSourceKind,
        canonical: &str,
    ) -> Result<(), ExternalError> {
        let url = if *kind == ReleaseSourceKind::API {
            source.api_url(canonical)?
        } else {
            source.asset_url(canonical)?
        };
        let url = parse_url(&url)
            .map_err(|error| wrap("create probe request", error))?
            .with_fragment("");
        let request = HttpRequest {
            method: "GET".into(),
            url,
            logical_host: None,
            headers: Headers::from([("User-Agent".into(), vec![GITHUB_USER_AGENT.into()])]),
            body: None,
            content_length: 0,
            context: context.clone(),
        };
        let response = tokio::select! {biased;result=self.transport.send(request)=>result,error=context.cancelled()=>Err(Box::new(error) as ExternalError)};
        let mut response = response
            .map_err(|error| wrap("request probe URL", error))?
            .ok_or_else(|| {
                wrap(
                    "request probe URL",
                    message("HTTP transport returned no response"),
                )
            })?;
        if response.status != 200 {
            if let Some(body) = response.body.as_mut() {
                let _ = body.close().await;
            }
            let reason = reqwest::StatusCode::from_u16(response.status)
                .ok()
                .and_then(|code| code.canonical_reason())
                .unwrap_or("");
            return Err(message(format!(
                "probe URL returned HTTP {} {reason}",
                response.status
            )));
        }
        let Some(mut body) = response.body.take() else {
            return if *kind == ReleaseSourceKind::API {
                Err(wrap("decode GitHub Releases JSON", message("EOF")))
            } else {
                Ok(())
            };
        };
        let result = if *kind == ReleaseSourceKind::API {
            read_probe_array(body.as_mut(), &context).await
        } else {
            drain_body(body.as_mut(), &context).await
        };
        let _ = body.close().await;
        result
    }
}
struct CancelProbes(CancellationToken);
impl Drop for CancelProbes {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
#[derive(Debug)]
struct ProbeContext {
    parent: CallerContext,
    cancel: CancellationToken,
}
impl RequestContext for ProbeContext {
    fn error(&self) -> Option<ContextError> {
        self.parent
            .error()
            .or_else(|| self.cancel.is_cancelled().then_some(ContextError::Canceled))
    }
    fn deadline(&self) -> Option<Instant> {
        self.parent.deadline()
    }
    fn cancelled(&self) -> ContextFuture<'_> {
        Box::pin(async move {
            if let Some(error) = self.error() {
                return error;
            }
            tokio::select! {error=self.parent.cancelled()=>error,_=self.cancel.cancelled()=>self.parent.error().unwrap_or(ContextError::Canceled)}
        })
    }
    fn value(&self, key: &ContextKey) -> Option<ContextValue> {
        self.parent.value(key)
    }
    fn extension(&self, type_id: TypeId) -> Option<ContextValue> {
        self.parent.extension(type_id)
    }
    fn scope(&self) -> Option<&Scope> {
        self.parent.scope()
    }
}
async fn read_body(
    body: &mut dyn RawBody,
    context: &CallerContext,
    buffer: &mut [u8],
) -> pixiv_sdk::fanbox::transport::RawRead {
    let read = tokio::select! {biased;result=body.read(buffer)=>result,error=context.cancelled()=>pixiv_sdk::fanbox::transport::RawRead{count:0,eof:false,error:Some(Box::new(error))}};
    if read.count > buffer.len() {
        return pixiv_sdk::fanbox::transport::RawRead {
            count: 0,
            eof: false,
            error: Some(message("invalid body read count")),
        };
    }
    read
}
async fn drain_body(body: &mut dyn RawBody, context: &CallerContext) -> Result<(), ExternalError> {
    let mut buffer = [0; 32768];
    loop {
        let read = read_body(body, context, &mut buffer).await;
        if let Some(error) = read.error {
            return Err(wrap("read probe response", error));
        }
        if read.eof {
            return Ok(());
        }
    }
}
async fn read_probe_array(
    body: &mut dyn RawBody,
    context: &CallerContext,
) -> Result<(), ExternalError> {
    let mut bytes = vec![];
    let mut buffer = vec![0; 64];
    let mut capacity = 64;
    let mut ended = false;
    let offset = loop {
        let mut stream = serde_json::Deserializer::from_slice(&bytes).into_iter::<Value>();
        let parse = stream.next();
        match parse {
            Some(Ok(value)) => {
                validate_probe_array(&value)
                    .map_err(|error| wrap("decode GitHub Releases JSON", error))?;
                break stream.byte_offset();
            }
            Some(Err(error)) if !error.is_eof() => {
                return Err(wrap(
                    "decode GitHub Releases JSON",
                    json_error(&bytes, &error),
                ));
            }
            _ if ended => {
                return Err(wrap(
                    "decode GitHub Releases JSON",
                    message(if bytes.iter().all(u8::is_ascii_whitespace) {
                        "EOF"
                    } else {
                        "unexpected EOF"
                    }),
                ));
            }
            _ => {}
        }
        let read = read_body(body, context, &mut buffer).await;
        bytes.extend_from_slice(&buffer[..read.count]);
        ended = read.eof && read.count == 0;
        if read.count == 0
            && let Some(error) = read.error
        {
            return Err(wrap("decode GitHub Releases JSON", error));
        }
        if bytes.len() >= capacity {
            capacity *= 2;
        }
        buffer.resize(capacity - bytes.len(), 0);
    };
    let mut trailing = bytes[offset..].to_vec();
    loop {
        let mut stream = serde_json::Deserializer::from_slice(&trailing).into_iter::<Value>();
        match stream.next() {
            Some(Ok(_)) => {
                return Err(message(
                    "decode GitHub Releases JSON: contains more than one JSON value",
                ));
            }
            Some(Err(error)) if !error.is_eof() => {
                return Err(wrap(
                    "decode GitHub Releases JSON trailing data",
                    json_error(&trailing, &error),
                ));
            }
            _ if ended => {
                return if trailing.iter().all(u8::is_ascii_whitespace) {
                    Ok(())
                } else {
                    Err(wrap(
                        "decode GitHub Releases JSON trailing data",
                        message("unexpected EOF"),
                    ))
                };
            }
            _ => {}
        }
        let read = read_body(body, context, &mut buffer).await;
        trailing.extend_from_slice(&buffer[..read.count]);
        ended = read.eof && read.count == 0;
        if read.count == 0
            && let Some(error) = read.error
        {
            return Err(wrap("decode GitHub Releases JSON trailing data", error));
        }
    }
}
fn validate_probe_array(value: &Value) -> Result<(), ExternalError> {
    if value.is_null() {
        return Err(message("expected an array"));
    }
    let Some(array) = value.as_array() else {
        return Err(message(format!(
            "json: cannot unmarshal {} into Go value of type []source.releaseProbeWire",
            json_type(value)
        )));
    };
    for (index, element) in array.iter().enumerate() {
        if element.is_null() {
            continue;
        }
        let Some(object) = element.as_object() else {
            return Err(message(format!(
                "json: cannot unmarshal {} into .{index} of type source.releaseProbeWire",
                json_type(element)
            )));
        };
        for (name, value) in object {
            let expected = if name.eq_ignore_ascii_case("tag_name") {
                Some(("tag_name", "string", value.is_string()))
            } else if name.eq_ignore_ascii_case("draft") {
                Some(("draft", "bool", value.is_boolean()))
            } else if name.eq_ignore_ascii_case("prerelease") {
                Some(("prerelease", "bool", value.is_boolean()))
            } else {
                None
            };
            if let Some((name, kind, valid)) = expected
                && !value.is_null()
                && !valid
            {
                return Err(message(format!(
                    "json: cannot unmarshal {} into Go struct field .{index}.{name} of type {kind}",
                    json_type(value)
                )));
            }
        }
    }
    Ok(())
}
fn json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}
fn json_error(bytes: &[u8], error: &serde_json::Error) -> ExternalError {
    let first = bytes
        .iter()
        .copied()
        .find(|byte| !byte.is_ascii_whitespace());
    if let Some(byte) = first
        && !b"[{\"tfn-0123456789".contains(&byte)
    {
        return message(format!(
            "invalid character '{}' looking for beginning of value",
            char::from(byte)
        ));
    }
    message(error.to_string())
}
