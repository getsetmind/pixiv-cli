use super::{
    CallerContext, Error, ErrorCode, Match, Provider, ProviderClient, ProviderResponse, Quota,
    ReverseFuture, Snapshot,
    http::{HttpRequest, HttpTransport, ReqwestTransport},
};
use crate::auth_bundle::{folded, normalize_strings};
use pixiv_sdk::fanbox::transport::{Headers, RawBody};
use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use std::{
    fmt,
    fs::File,
    io::{self, Cursor, Read},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::Notify;

const DEFAULT_ENDPOINT: &str = "https://saucenao.com/search.php";

#[derive(Default)]
pub struct Options {
    pub api_key: String,
    pub transport: Option<Arc<dyn HttpTransport>>,
    pub endpoint: String,
}
pub struct Client {
    api_key: String,
    transport: Arc<dyn HttpTransport>,
    endpoint: String,
    closed: AtomicBool,
}
impl Client {
    pub fn new(options: Options) -> Self {
        Self {
            api_key: options.api_key,
            transport: options.transport.unwrap_or_else(|| {
                Arc::new(ReqwestTransport::new("").expect("default HTTP configuration"))
            }),
            endpoint: if options.endpoint.is_empty() {
                DEFAULT_ENDPOINT.to_owned()
            } else {
                options.endpoint
            },
            closed: AtomicBool::new(false),
        }
    }
    pub async fn preflight(&self, context: Option<CallerContext>) -> Result<(), Error> {
        let context = context.ok_or_else(|| {
            Error::new(
                ErrorCode::InvalidRequest,
                "reverse search context is required",
                None,
            )
        })?;
        if let Some(error) = context.error() {
            return Err(error.into());
        }
        if self.api_key.trim().is_empty() {
            return Err(Error::new(
                ErrorCode::MissingCredential,
                "SauceNAO API key is required",
                None,
            ));
        }
        Ok(())
    }
    pub async fn close(&self) -> Result<(), Error> {
        if !self.closed.swap(true, Ordering::AcqRel) {
            self.transport.close_idle_connections();
        }
        Ok(())
    }
    pub async fn search(
        &self,
        context: Option<CallerContext>,
        snapshot: Option<Arc<Snapshot>>,
    ) -> Result<ProviderResponse, Error> {
        self.preflight(context.clone()).await?;
        let context = context.expect("preflight validated context");
        let snapshot = snapshot.ok_or_else(|| {
            Error::new(
                ErrorCode::InvalidRequest,
                "image snapshot is required",
                None,
            )
        })?;
        let (body, content_type, upload) =
            MultipartBody::new(context.clone(), snapshot, &self.api_key)?;
        if !valid_endpoint(&self.endpoint) {
            upload.abort();
            return Err(provider_failed("could not create SauceNAO request"));
        }
        let mut headers = Headers::new();
        headers.insert("Content-Type".to_owned(), vec![content_type]);
        let response = self
            .transport
            .send(HttpRequest {
                method: "POST".to_owned(),
                url: self.endpoint.clone(),
                logical_host: None,
                headers,
                body: Some(Box::new(body)),
                content_length: 0,
                context: context.clone(),
            })
            .await;
        let mut response = match response {
            Ok(Some(response)) => response,
            Ok(None) | Err(_) => {
                upload.abort();
                if let Some(error) = context.error() {
                    return Err(error.into());
                }
                return Err(provider_failed("SauceNAO request failed"));
            }
        };
        let write_failed = upload.finished().await;
        let result = if let Some(error) = context.error() {
            Err(error.into())
        } else if !(200..300).contains(&response.status) {
            Err(Error::new(
                ErrorCode::UpstreamHttpStatus,
                "SauceNAO returned an unsuccessful HTTP status",
                None,
            ))
        } else if write_failed {
            Err(provider_failed("could not upload image to SauceNAO"))
        } else {
            match response.body.as_deref_mut() {
                Some(body) => decode_response(body).await,
                None => Err(malformed()),
            }
        };
        if let Some(body) = response.body.as_mut() {
            let _ = body.close().await;
        }
        result
    }
}
impl ProviderClient for Client {
    fn preflight(&self, context: CallerContext) -> ReverseFuture<'_, Result<(), Error>> {
        Box::pin(Client::preflight(self, Some(context)))
    }
    fn search(
        &self,
        context: CallerContext,
        snapshot: Arc<Snapshot>,
    ) -> ReverseFuture<'_, Result<ProviderResponse, Error>> {
        Box::pin(Client::search(self, Some(context), Some(snapshot)))
    }
    fn close(&self) -> ReverseFuture<'_, Result<(), Error>> {
        Box::pin(Client::close(self))
    }
}
fn provider_failed(message: &str) -> Error {
    Error::new(ErrorCode::ProviderFailed, message, None)
}
fn malformed() -> Error {
    Error::new(
        ErrorCode::MalformedUpstreamResponse,
        "SauceNAO returned a malformed response",
        None,
    )
}
fn valid_endpoint(endpoint: &str) -> bool {
    if endpoint.bytes().any(|byte| byte < 32 || byte == 127) {
        return false;
    }
    if url::Url::parse(endpoint).is_err() {
        return false;
    }
    let path = endpoint.split(['?', '#']).next().unwrap_or("").as_bytes();
    let mut index = 0;
    while index < path.len() {
        if path[index] == b'%' {
            if path
                .get(index + 1..index + 3)
                .is_none_or(|pair| !pair.iter().all(u8::is_ascii_hexdigit))
            {
                return false;
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    true
}

#[derive(Default)]
struct UploadState {
    outcome: Mutex<Option<bool>>,
    notify: Notify,
    aborted: AtomicBool,
}
impl UploadState {
    fn complete(&self, failed: bool) {
        let mut outcome = self
            .outcome
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if outcome.is_none() {
            *outcome = Some(failed);
            self.notify.notify_one();
        }
    }
    fn abort(&self) {
        self.aborted.store(true, Ordering::Release);
        self.complete(true);
    }
    async fn finished(&self) -> bool {
        loop {
            let notified = self.notify.notified();
            if let Some(failed) = *self
                .outcome
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
            {
                return failed;
            }
            notified.await;
        }
    }
}
struct MultipartBody {
    prefix: Cursor<Vec<u8>>,
    suffix: Cursor<Vec<u8>>,
    snapshot: Arc<Snapshot>,
    source: Option<File>,
    opened: bool,
    source_done: bool,
    error: Option<io::Error>,
    context: CallerContext,
    state: Arc<UploadState>,
    done: bool,
}
impl MultipartBody {
    fn new(
        context: CallerContext,
        snapshot: Arc<Snapshot>,
        key: &str,
    ) -> Result<(Self, String, Arc<UploadState>), Error> {
        let mut random = [0; 30];
        getrandom::fill(&mut random)
            .map_err(|_| provider_failed("could not create SauceNAO request"))?;
        let boundary = random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let mut prefix = Vec::new();
        for (index, (name, value)) in [("api_key", key), ("output_type", "2"), ("db", "999")]
            .into_iter()
            .enumerate()
        {
            if index != 0 {
                prefix.extend_from_slice(b"\r\n");
            }
            prefix.extend_from_slice(
                format!(
                    "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}"
                )
                .as_bytes(),
            );
        }
        prefix.extend_from_slice(format!("\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"image\"\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes());
        let state = Arc::new(UploadState::default());
        let body = Self {
            prefix: Cursor::new(prefix),
            suffix: Cursor::new(format!("\r\n--{boundary}--\r\n").into_bytes()),
            snapshot,
            source: None,
            opened: false,
            source_done: false,
            error: None,
            context,
            state: state.clone(),
            done: false,
        };
        Ok((
            body,
            format!("multipart/form-data; boundary={boundary}"),
            state,
        ))
    }
}
impl Read for MultipartBody {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        if self.state.aborted.load(Ordering::Acquire) {
            self.done = true;
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "request body is closed",
            ));
        }
        let count = self.prefix.read(output)?;
        if count != 0 {
            return Ok(count);
        }
        if !self.opened {
            self.opened = true;
            match self.snapshot.open() {
                Ok(source) => self.source = Some(source),
                Err(error) => {
                    self.error = Some(io::Error::other(error));
                    self.source_done = true;
                }
            }
        }
        if !self.source_done {
            if let Some(error) = self.context.error() {
                self.error = Some(io::Error::other(error));
                self.source_done = true;
            } else if let Some(source) = self.source.as_mut() {
                match source.read(output) {
                    Ok(count) if count != 0 => return Ok(count),
                    Ok(_) => self.source_done = true,
                    Err(error) => {
                        self.error = Some(error);
                        self.source_done = true;
                    }
                }
            }
            if self.source_done {
                self.source = None;
            }
        }
        let count = self.suffix.read(output)?;
        if count != 0 {
            return Ok(count);
        }
        self.done = true;
        self.state.complete(self.error.is_some());
        match self.error.take() {
            Some(error) => Err(error),
            None => Ok(0),
        }
    }
}
impl Drop for MultipartBody {
    fn drop(&mut self) {
        self.state.complete(!self.done || self.error.is_some());
    }
}

#[derive(Clone, Default)]
struct WireResponse {
    header: Option<WireHeader>,
    results: Option<Vec<WireResult>>,
}
#[derive(Clone, Default)]
struct WireHeader {
    status: Option<i64>,
    short_remaining: Option<i64>,
    long_remaining: Option<i64>,
    short_limit: Option<i64>,
    long_limit: Option<i64>,
}
#[derive(Clone, Default)]
struct WireResult {
    header: Option<WireResultHeader>,
    data: Option<WireData>,
}
#[derive(Clone, Default)]
struct WireResultHeader {
    similarity: Option<f64>,
    index_id: Option<i64>,
    index_name: String,
}
#[derive(Clone, Default)]
struct WireData {
    external_urls: Vec<String>,
    title: String,
    pixiv_id: i64,
    member_name: String,
    author_name: String,
    member_id: i64,
}

enum Node {
    Null,
    String { raw: String, text: String },
    Number(String),
    Bool,
    Array(Vec<Node>),
    Object(Vec<(String, Node)>),
}
impl<'de> Deserialize<'de> for Node {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = <&serde_json::value::RawValue>::deserialize(deserializer)?.get();
        if matches!(raw.as_bytes().first(), Some(b'-' | b'0'..=b'9')) {
            return Ok(Self::Number(raw.to_owned()));
        }
        if raw.starts_with('"') {
            return serde_json::from_str::<String>(raw)
                .map(|text| Self::String {
                    raw: raw.to_owned(),
                    text,
                })
                .map_err(de::Error::custom);
        }
        struct Nodes;
        impl<'de> Visitor<'de> for Nodes {
            type Value = Node;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("JSON value")
            }
            fn visit_unit<E: de::Error>(self) -> Result<Node, E> {
                Ok(Node::Null)
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Node, E> {
                Ok(Node::Bool)
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut array: A) -> Result<Node, A::Error> {
                let mut nodes = Vec::new();
                while let Some(node) = array.next_element()? {
                    nodes.push(node);
                }
                Ok(Node::Array(nodes))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut object: A) -> Result<Node, A::Error> {
                let mut nodes = Vec::new();
                while let Some(key) = object.next_key()? {
                    nodes.push((key, object.next_value()?));
                }
                Ok(Node::Object(nodes))
            }
        }
        serde_json::Deserializer::from_str(raw)
            .deserialize_any(Nodes)
            .map_err(de::Error::custom)
    }
}
fn fields(node: &Node) -> Result<Option<&[(String, Node)]>, ()> {
    match node {
        Node::Null => Ok(None),
        Node::Object(fields) => Ok(Some(fields)),
        _ => Err(()),
    }
}
fn numeric_raw(node: &Node) -> Result<&str, ()> {
    match node {
        Node::String { raw, .. } => Ok(raw.trim_matches('"')),
        Node::Number(raw) => Ok(raw),
        _ => Err(()),
    }
}
fn integer(node: &Node) -> Result<i64, ()> {
    let text = numeric_raw(node)?;
    let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(());
    }
    text.parse().map_err(|_| ())
}
fn integer_pointer(target: &mut Option<i64>, node: &Node) -> Result<(), ()> {
    *target = if matches!(node, Node::Null) {
        None
    } else {
        Some(integer(node)?)
    };
    Ok(())
}
fn string(target: &mut String, node: &Node) -> Result<(), ()> {
    match node {
        Node::Null => Ok(()),
        Node::String { text, .. } => {
            target.clone_from(text);
            Ok(())
        }
        _ => Err(()),
    }
}
fn update_pointer<T: Default>(
    target: &mut Option<T>,
    node: &Node,
    update: fn(&mut T, &Node) -> Result<(), ()>,
) -> Result<(), ()> {
    if matches!(node, Node::Null) {
        *target = None;
        return Ok(());
    }
    update(target.get_or_insert_with(T::default), node)
}
impl WireResponse {
    fn update(&mut self, node: &Node) -> Result<(), ()> {
        if let Some(fields) = fields(node)? {
            for (key, value) in fields {
                if folded(key, "header") {
                    update_pointer(&mut self.header, value, WireHeader::update)?;
                } else if folded(key, "results") {
                    match value {
                        Node::Null => self.results = None,
                        Node::Array(values) => {
                            let results = self.results.get_or_insert_with(Vec::new);
                            for (index, value) in values.iter().enumerate() {
                                if results.len() <= index {
                                    results.push(WireResult::default());
                                }
                                results[index].update(value)?;
                            }
                            results.truncate(values.len());
                        }
                        _ => return Err(()),
                    }
                }
            }
        }
        Ok(())
    }
    fn response(self) -> Result<ProviderResponse, Error> {
        let header = self.header.ok_or_else(malformed)?;
        let status = header.status.ok_or_else(malformed)?;
        if status != 0 {
            return Err(provider_failed("SauceNAO rejected the query"));
        }
        let quota = Quota {
            short_remaining: header.short_remaining.ok_or_else(malformed)?,
            long_remaining: header.long_remaining.ok_or_else(malformed)?,
            short_limit: header.short_limit.ok_or_else(malformed)?,
            long_limit: header.long_limit.ok_or_else(malformed)?,
        };
        let mut matches = Vec::new();
        for (index, result) in self.results.ok_or_else(malformed)?.into_iter().enumerate() {
            let header = result.header.ok_or_else(malformed)?;
            let data = result.data.ok_or_else(malformed)?;
            if header.index_name.is_empty() {
                return Err(malformed());
            }
            matches.push(Match {
                rank: index as i64 + 1,
                similarity: header.similarity.ok_or_else(malformed)?,
                index_id: header.index_id.ok_or_else(malformed)?,
                index_name: header.index_name,
                title: data.title,
                author: if data.member_name.is_empty() {
                    data.author_name
                } else {
                    data.member_name
                },
                artwork_id: data.pixiv_id,
                user_id: data.member_id,
                external_urls: data.external_urls,
            });
        }
        Ok(ProviderResponse {
            provider: Provider::SauceNao,
            matches,
            quota: Some(quota),
        })
    }
}
impl WireHeader {
    fn update(&mut self, node: &Node) -> Result<(), ()> {
        if let Some(fields) = fields(node)? {
            for (key, value) in fields {
                if folded(key, "status") {
                    integer_pointer(&mut self.status, value)?;
                } else if folded(key, "short_remaining") {
                    integer_pointer(&mut self.short_remaining, value)?;
                } else if folded(key, "long_remaining") {
                    integer_pointer(&mut self.long_remaining, value)?;
                } else if folded(key, "short_limit") {
                    integer_pointer(&mut self.short_limit, value)?;
                } else if folded(key, "long_limit") {
                    integer_pointer(&mut self.long_limit, value)?;
                }
            }
        }
        Ok(())
    }
}
impl WireResult {
    fn update(&mut self, node: &Node) -> Result<(), ()> {
        if let Some(fields) = fields(node)? {
            for (key, value) in fields {
                if folded(key, "header") {
                    update_pointer(&mut self.header, value, WireResultHeader::update)?;
                } else if folded(key, "data") {
                    update_pointer(&mut self.data, value, WireData::update)?;
                }
            }
        }
        Ok(())
    }
}
impl WireResultHeader {
    fn update(&mut self, node: &Node) -> Result<(), ()> {
        if let Some(fields) = fields(node)? {
            for (key, value) in fields {
                if folded(key, "similarity") {
                    self.similarity = if matches!(value, Node::Null) {
                        None
                    } else {
                        Some(float(value)?)
                    };
                } else if folded(key, "index_id") {
                    integer_pointer(&mut self.index_id, value)?;
                } else if folded(key, "index_name") {
                    string(&mut self.index_name, value)?;
                }
            }
        }
        Ok(())
    }
}
impl WireData {
    fn update(&mut self, node: &Node) -> Result<(), ()> {
        if let Some(fields) = fields(node)? {
            for (key, value) in fields {
                if folded(key, "ext_urls") {
                    match value {
                        Node::Null => self.external_urls.clear(),
                        Node::Array(values) => {
                            for (index, value) in values.iter().enumerate() {
                                if self.external_urls.len() <= index {
                                    self.external_urls.push(String::new());
                                }
                                string(&mut self.external_urls[index], value)?;
                            }
                            self.external_urls.truncate(values.len());
                        }
                        _ => return Err(()),
                    }
                } else if folded(key, "title") {
                    string(&mut self.title, value)?;
                } else if folded(key, "pixiv_id") {
                    self.pixiv_id = integer(value)?;
                } else if folded(key, "member_name") {
                    string(&mut self.member_name, value)?;
                } else if folded(key, "author_name") {
                    string(&mut self.author_name, value)?;
                } else if folded(key, "member_id") {
                    self.member_id = integer(value)?;
                }
            }
        }
        Ok(())
    }
}

fn float(node: &Node) -> Result<f64, ()> {
    let text = numeric_raw(node)?;
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    let hex = unsigned.starts_with("0x") || unsigned.starts_with("0X");
    if !float_syntax(text, hex) {
        return Err(());
    }
    let cleaned = text.replace('_', "");
    let value = if hex {
        hex_float(&cleaned)?
    } else {
        cleaned.parse::<f64>().map_err(|_| ())?
    };
    if value.is_finite() {
        Ok(value)
    } else {
        Err(())
    }
}
fn float_syntax(text: &str, hex: bool) -> bool {
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    let bytes = unsigned.as_bytes();
    let start = if hex { 2 } else { 0 };
    let mut index = start;
    let mut digits = 0;
    let mut dot = false;
    let digit = |byte: u8| byte.is_ascii_digit() || (hex && byte.is_ascii_hexdigit());
    while index < bytes.len() {
        let byte = bytes[index];
        if digit(byte) {
            digits += 1;
        } else if byte == b'.' && !dot {
            dot = true;
        } else if byte == b'_' {
            if index == 0
                || !(digit(bytes[index - 1]) || (hex && index == 2))
                || bytes.get(index + 1).is_none_or(|byte| !digit(*byte))
            {
                return false;
            }
        } else {
            break;
        }
        index += 1;
    }
    if digits == 0 {
        return false;
    }
    let exponent = if hex { b'p' } else { b'e' };
    if bytes
        .get(index)
        .is_some_and(|byte| byte.to_ascii_lowercase() == exponent)
    {
        index += 1;
        if bytes
            .get(index)
            .is_some_and(|byte| matches!(byte, b'+' | b'-'))
        {
            index += 1;
        }
        if bytes.get(index).is_none_or(|byte| !byte.is_ascii_digit()) {
            return false;
        }
        while index < bytes.len() {
            if bytes[index] == b'_' {
                if !bytes[index - 1].is_ascii_digit()
                    || bytes
                        .get(index + 1)
                        .is_none_or(|byte| !byte.is_ascii_digit())
                {
                    return false;
                }
            } else if !bytes[index].is_ascii_digit() {
                return false;
            }
            index += 1;
        }
    } else if hex {
        return false;
    }
    index == bytes.len()
}
fn hex_float(text: &str) -> Result<f64, ()> {
    let negative = text.starts_with('-');
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    let (mantissa, exponent) = unsigned[2..].split_once(['p', 'P']).ok_or(())?;
    let exponent = exponent.parse::<i64>().unwrap_or_else(|_| {
        if exponent.starts_with('-') {
            -100_000
        } else {
            100_000
        }
    });
    let fractional = mantissa
        .split_once('.')
        .map_or(0, |(_, fraction)| fraction.len() as i64);
    let mut bits = Vec::new();
    for digit in mantissa.bytes().filter(|byte| *byte != b'.') {
        let value = (digit as char).to_digit(16).ok_or(())?;
        for shift in (0..4).rev() {
            bits.push((value >> shift) & 1 != 0);
        }
    }
    let first = bits.iter().position(|bit| *bit).unwrap_or(bits.len());
    let bits = &bits[first..];
    let sign = u64::from(negative) << 63;
    if bits.is_empty() {
        return Ok(f64::from_bits(sign));
    }
    let power = exponent.saturating_sub(fractional.saturating_mul(4));
    let mut high = power.saturating_add(bits.len() as i64 - 1);
    if high > 1023 {
        return Err(());
    }
    if high < -1075 {
        return Ok(f64::from_bits(sign));
    }
    let keep = if high >= -1022 {
        53
    } else {
        (bits.len() as i64 + power + 1074).max(0) as usize
    };
    let mut value = 0_u64;
    for bit in bits.iter().take(keep) {
        value = (value << 1) | u64::from(*bit);
    }
    if keep > bits.len() {
        value <<= keep - bits.len();
    }
    let rounding = bits.get(keep).copied().unwrap_or(false);
    let sticky = bits
        .get(keep + 1..)
        .is_some_and(|rest| rest.iter().any(|bit| *bit));
    if rounding && (sticky || value & 1 != 0) {
        value += 1;
    }
    if high >= -1022 {
        if value == 1 << 53 {
            value >>= 1;
            high += 1;
        }
        if high > 1023 {
            return Err(());
        }
        Ok(f64::from_bits(
            sign | ((high + 1023) as u64) << 52 | (value & ((1 << 52) - 1)),
        ))
    } else {
        Ok(f64::from_bits(sign | value))
    }
}

async fn decode_response(body: &mut dyn RawBody) -> Result<ProviderResponse, Error> {
    let mut decoder = Decoder {
        body,
        buffer: Vec::new(),
        capacity: 64,
        previous: 0,
        absolute: 0,
    };
    let node = decoder.value().await?.ok_or_else(malformed)?;
    let mut response = WireResponse::default();
    response.update(&node).map_err(|_| malformed())?;
    if decoder.value().await?.is_some() {
        return Err(malformed());
    }
    response.response()
}
struct Decoder<'a> {
    body: &'a mut dyn RawBody,
    buffer: Vec<u8>,
    capacity: usize,
    previous: usize,
    absolute: usize,
}
impl Decoder<'_> {
    async fn fetch(&mut self) -> Result<bool, Error> {
        let grow = (self.capacity <= 2048 && self.capacity < (self.absolute + self.previous) / 2)
            || (self.previous == 0 && self.buffer.len() >= 3 * self.capacity / 4);
        if grow {
            self.capacity = self.capacity.checked_mul(2).ok_or_else(malformed)?;
        }
        if self.previous != 0 {
            self.buffer.drain(..self.previous);
            self.absolute += self.previous;
            self.previous = 0;
        }
        loop {
            let mut output = vec![0; self.capacity - self.buffer.len()];
            let read = self.body.read(&mut output).await;
            if read.count > output.len() {
                return Err(malformed());
            }
            if read.count != 0 {
                self.buffer.extend_from_slice(&output[..read.count]);
                return Ok(true);
            }
            if read.eof {
                return Ok(false);
            }
            if read.error.is_some() {
                return Err(malformed());
            }
            tokio::task::yield_now().await;
        }
    }
    async fn value(&mut self) -> Result<Option<Node>, Error> {
        let mut ended = false;
        loop {
            let raw = &self.buffer[self.previous..];
            let normalized = normalize_strings(raw);
            let mut values = serde_json::Deserializer::from_str(&normalized)
                .into_iter::<&serde_json::value::RawValue>();
            match values.next() {
                Some(Ok(value)) => {
                    let consumed = values.byte_offset();
                    let number = matches!(value.get().as_bytes().first(), Some(b'-' | b'0'..=b'9'));
                    if number && !ended && consumed == normalized.len() {
                        ended = !self.fetch().await?;
                        continue;
                    }
                    let node = serde_json::from_str(value.get()).map_err(|_| malformed())?;
                    let original_end = raw_boundary(raw, consumed, &normalized);
                    self.previous += original_end;
                    return Ok(Some(node));
                }
                Some(Err(error)) if !error.is_eof() => return Err(malformed()),
                Some(Err(_)) if ended => return Err(malformed()),
                None if ended => return Ok(None),
                _ => {
                    ended = !self.fetch().await?;
                }
            }
        }
    }
}
fn raw_boundary(raw: &[u8], normalized_offset: usize, normalized: &str) -> usize {
    if raw.len() == normalized.len() {
        return normalized_offset;
    }
    let mut original = 0;
    let mut repaired = 0;
    while original < raw.len() {
        match std::str::from_utf8(&raw[original..]) {
            Ok(_) => return original + normalized_offset.saturating_sub(repaired),
            Err(error) => {
                let valid = error.valid_up_to();
                if normalized_offset <= repaired + valid {
                    return original + normalized_offset - repaired;
                }
                original += valid + 1;
                repaired += valid + 3;
                if normalized_offset <= repaired {
                    return original;
                }
            }
        }
    }
    raw.len()
}
