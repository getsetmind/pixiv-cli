use chrono::{DateTime, Datelike, FixedOffset, Local, Timelike};
use pixiv_sdk::diagnostics::{Event, Sink};
use serde::Serialize;
use std::{
    io::{self, Write},
    sync::{Arc, Mutex, MutexGuard},
};

pub type Clock = Arc<dyn Fn() -> DateTime<FixedOffset> + Send + Sync>;

pub struct Presenter {
    json: bool,
    now: Clock,
    state: Mutex<State>,
}

struct State {
    writer: Box<dyn Write + Send>,
    error: Option<Arc<io::Error>>,
}

impl Presenter {
    pub fn new(writer: Option<Box<dyn Write + Send>>) -> Self {
        Self::with_format(writer, "text", None)
    }

    pub fn with_clock(writer: Option<Box<dyn Write + Send>>, now: Option<Clock>) -> Self {
        Self::with_format(writer, "text", now)
    }

    pub fn with_format(
        writer: Option<Box<dyn Write + Send>>,
        format: impl Into<String>,
        now: Option<Clock>,
    ) -> Self {
        Self {
            json: format.into() == "json",
            now: now.unwrap_or_else(|| Arc::new(|| Local::now().fixed_offset())),
            state: Mutex::new(State {
                writer: writer.unwrap_or_else(|| Box::new(io::sink())),
                error: None,
            }),
        }
    }

    pub fn error(&self) -> Option<Arc<io::Error>> {
        self.state().error.clone()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn render(&self, event: &Event) -> io::Result<Vec<u8>> {
        let now = (self.now)();
        if self.json {
            let record = JsonRecord {
                time: timestamp(now),
                level: "DEBUG",
                module: safe_text(&event.module),
                kind: safe_text(&event.kind),
                operation: safe_text(&event.operation),
                resource: safe_text(&event.resource),
                route: safe_text(&event.route),
                proxy: safe_address(&event.proxy),
                reason: safe_text(&event.reason),
                status: event.status,
                count: event.count,
                request_id: event.request_id,
                duration_ms: event.duration_ns / 1_000_000,
            };
            let encoded = serde_json::to_string(&record).map_err(io::Error::other)?;
            let mut body = crate::go_json_escape(encoded).into_bytes();
            body.push(b'\n');
            Ok(body)
        } else {
            let mut body = format!(
                "[{}] {:02}:{:02}:{:02} ",
                event.module,
                now.hour(),
                now.minute(),
                now.second()
            )
            .into_bytes();
            body.extend(narrative(event));
            body.push(b'\n');
            Ok(body)
        }
    }
}

impl Sink for Presenter {
    fn emit(&self, event: Event) {
        let rendered = self.render(&event);
        let mut state = self.state();
        let error = match rendered {
            Ok(record) => state.writer.write(&record).err(),
            Err(error) => Some(error),
        };
        if state.error.is_none() {
            state.error = error.map(Arc::new);
        }
    }
}

#[derive(Serialize)]
struct JsonRecord {
    time: String,
    level: &'static str,
    module: String,
    kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    operation: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    resource: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    route: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    proxy: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    reason: String,
    #[serde(skip_serializing_if = "is_zero")]
    status: i64,
    #[serde(skip_serializing_if = "is_zero")]
    count: i64,
    #[serde(skip_serializing_if = "is_zero")]
    request_id: u64,
    #[serde(skip_serializing_if = "is_zero")]
    duration_ms: i64,
}

fn is_zero<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

fn timestamp(value: DateTime<FixedOffset>) -> String {
    let year = value.year();
    let year = if year < 0 {
        format!("-{:04}", -i64::from(year))
    } else {
        format!("{year:04}")
    };
    let mut output = format!(
        "{year}-{:02}-{:02}T{:02}:{:02}:{:02}",
        value.month(),
        value.day(),
        value.hour(),
        value.minute(),
        value.second()
    );
    let nanos = value.nanosecond();
    if nanos != 0 {
        output.push('.');
        output.push_str(format!("{nanos:09}").trim_end_matches('0'));
    }
    let offset = value.offset().local_minus_utc();
    if offset == 0 {
        output.push('Z');
    } else {
        let minutes = offset / 60;
        output.push(if minutes < 0 { '-' } else { '+' });
        let minutes = minutes.unsigned_abs();
        output.push_str(&format!("{:02}:{:02}", minutes / 60, minutes % 60));
    }
    output
}

fn operation_or<'a>(operation: &'a str, fallback: &'a str) -> &'a str {
    if operation.is_empty() {
        fallback
    } else {
        operation
    }
}

fn capitalize(value: &str) -> Vec<u8> {
    let Some((&first, rest)) = value.as_bytes().split_first() else {
        return Vec::new();
    };
    let mut output = if first.is_ascii() {
        vec![first.to_ascii_uppercase()]
    } else {
        "\u{fffd}".as_bytes().to_vec()
    };
    output.extend_from_slice(rest);
    output
}

fn capitalized_sentence(value: &str, suffix: &str) -> Vec<u8> {
    let mut output = capitalize(value);
    output.extend_from_slice(suffix.as_bytes());
    output
}

fn duration(value: i64) -> String {
    let seconds = (value / 1_000_000_000) as f64 + (value % 1_000_000_000) as f64 / 1_000_000_000.0;
    format!("{seconds:.1} seconds")
}

fn failure(reason: &str) -> &'static str {
    match reason {
        "command failed" => ": the command returned an error",
        "tool failed" => ": the tool returned an error",
        "challenge required" => ": a challenge is still required",
        "solver unavailable" => ": the solver is unavailable",
        "solver failed" => ": the solver failed",
        "malformed solver response" => ": the solver response is malformed",
        "account frozen" => ": the selected account is frozen",
        "account pool exhausted" => ": the account pool is exhausted",
        "configuration failed" => ": configuration failed",
        "authentication failed" => ": authentication failed",
        _ => "",
    }
}

fn narrative(event: &Event) -> Vec<u8> {
    let operation = safe_text(&event.operation);
    let resource = safe_text(&event.resource);
    let route = safe_text(&event.route);
    let request = event.request_id;
    let text = match event.kind.as_str() {
        "started" => {
            let operation = operation_or(&operation, "the operation");
            if request != 0 {
                format!("Started request {request} for {operation}.")
            } else {
                format!("Started {operation}.")
            }
        }
        "completed" => {
            if request != 0 {
                if event.duration_ns > 0 {
                    format!(
                        "Request {request} completed successfully in {}.",
                        duration(event.duration_ns)
                    )
                } else {
                    format!("Request {request} completed successfully.")
                }
            } else if event.duration_ns > 0 {
                format!(
                    "{} completed successfully in {}.",
                    operation_or(&operation, "The operation"),
                    duration(event.duration_ns)
                )
            } else {
                return capitalized_sentence(
                    operation_or(&operation, "the operation"),
                    " completed successfully.",
                );
            }
        }
        "failed" => {
            let reason = failure(&event.reason);
            if request != 0 {
                format!(
                    "Request {request} failed during {}{reason}.",
                    operation_or(&operation, "the operation")
                )
            } else {
                return capitalized_sentence(
                    operation_or(&operation, "the operation"),
                    &format!(" failed{reason}."),
                );
            }
        }
        "network_request" => {
            let mut verb = operation_or(&operation, "the request").to_owned();
            if !resource.is_empty() {
                verb.push(' ');
                verb.push_str(&resource);
            }
            if event.status > 0 {
                verb = format!("received HTTP {} while {verb}", event.status);
            }
            if !route.is_empty() {
                verb.push_str(" through the ");
                verb.push_str(&route);
            }
            let proxy = safe_address(&event.proxy);
            if !proxy.is_empty() {
                verb.push_str(" via proxy ");
                verb.push_str(&proxy);
            }
            if request != 0 {
                format!("Request {request} is {verb}.")
            } else {
                format!("Request is {verb}.")
            }
        }
        "challenge" => {
            if event.status > 0 && request != 0 {
                format!(
                    "Cloudflare challenged request {request} with HTTP {}.",
                    event.status
                )
            } else if request != 0 {
                format!("Cloudflare challenged request {request}.")
            } else {
                "Cloudflare issued a challenge.".into()
            }
        }
        "solver_started" => {
            if request != 0 {
                format!("Request {request} requires fresh Cloudflare clearance.")
            } else {
                "Fresh Cloudflare clearance is required.".into()
            }
        }
        "solver_completed" => {
            if request != 0 {
                format!("Clearance was acquired; request {request} will be replayed natively.")
            } else {
                "Clearance was acquired for native replay.".into()
            }
        }
        "replay" => {
            if request != 0 && !route.is_empty() {
                format!("Request {request} is replaying through the {route}.")
            } else if request != 0 {
                format!("Request {request} is replaying natively.")
            } else {
                "The request is replaying natively.".into()
            }
        }
        "download" => {
            let mut output = if operation.is_empty() {
                b"Download operation".to_vec()
            } else {
                capitalize(&operation)
            };
            if event.count > 0 {
                output
                    .extend_from_slice(format!(" discovered {} resources", event.count).as_bytes());
            }
            let target = safe_text(&event.target);
            if !target.is_empty() {
                output.extend_from_slice(b" at ");
                output.extend_from_slice(target.as_bytes());
            }
            output.push(b'.');
            return output;
        }
        "account" => {
            let mut message = operation_or(&operation, "Account pool").to_owned();
            if !resource.is_empty() {
                message.push(' ');
                message.push_str(&resource);
            }
            if event.count > 0 {
                message.push_str(&format!(" found {} accounts", event.count));
            }
            return capitalized_sentence(&message, ".");
        }
        "configuration" => {
            return capitalized_sentence(operation_or(&operation, "Configuration"), ".");
        }
        _ => "A diagnostic event occurred.".into(),
    };
    text.into_bytes()
}

fn safe_text(value: &str) -> String {
    if value.is_empty() || value.contains(['\r', '\n', '\0']) {
        return String::new();
    }
    if let Some(index) = value.find(['?', '#']) {
        return parse_address(value)
            .map(|address| address.text)
            .unwrap_or_else(|| value[..index].to_owned());
    }
    value.to_owned()
}

fn safe_address(value: &str) -> String {
    parse_address(value)
        .filter(|address| address.has_hostname)
        .map(|address| address.text)
        .unwrap_or_else(|| safe_text(value))
}

struct Address {
    text: String,
    has_hostname: bool,
}

fn parse_address(value: &str) -> Option<Address> {
    let (main, fragment) = value.split_once('#').unwrap_or((value, ""));
    unescape(fragment, Component::Path)?;
    if main.bytes().any(|byte| byte < 32 || byte == 127) {
        return None;
    }
    let mut scheme_end = None;
    for (index, byte) in main.bytes().enumerate() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' => {}
            b'0'..=b'9' | b'+' | b'-' | b'.' if index != 0 => {}
            b':' if index != 0 => {
                scheme_end = Some(index);
                break;
            }
            b':' => return None,
            _ => break,
        }
    }
    let (scheme, remainder) = match scheme_end {
        Some(index) => (main[..index].to_ascii_lowercase(), &main[index + 1..]),
        None => (String::new(), main),
    };
    let mut path = remainder
        .split_once('?')
        .map_or(remainder, |(path, _)| path);
    let mut output = if scheme.is_empty() {
        String::new()
    } else {
        format!("{scheme}:")
    };
    if !scheme.is_empty() && !path.starts_with('/') && !path.is_empty() {
        output.push_str(path);
        return Some(Address {
            text: output,
            has_hostname: false,
        });
    }
    if scheme.is_empty() && path.split('/').next()?.contains(':') {
        return None;
    }
    let mut host = Vec::new();
    let mut omit_host = false;
    if path.starts_with("//") && (!scheme.is_empty() || !path.starts_with("///")) {
        let authority = &path[2..];
        let boundary = authority.find('/').unwrap_or(authority.len());
        path = &authority[boundary..];
        let authority = &authority[..boundary];
        let raw_host = if let Some((userinfo, host)) = authority.rsplit_once('@') {
            if !userinfo
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-._:~!$&'()*+,;=%@".contains(&byte))
            {
                return None;
            }
            unescape(userinfo, Component::Path)?;
            host
        } else {
            authority
        };
        host = parse_host(raw_host, &scheme)?;
    } else if !scheme.is_empty() && path.starts_with('/') {
        omit_host = true;
    }
    let decoded = unescape(path, Component::Path)?;
    let escaped_path = if path
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@/[]%".contains(&byte))
    {
        path.to_owned()
    } else {
        escape(&decoded, Component::Path)
    };
    if (!scheme.is_empty() || !host.is_empty()) && !omit_host {
        if !host.is_empty() || !decoded.is_empty() {
            output.push_str("//");
        }
        output.push_str(&escape(&host, Component::Host));
    }
    if omit_host && escaped_path.starts_with("//") {
        output.push_str("%2F");
        output.push_str(&escaped_path[1..]);
    } else {
        if !escaped_path.is_empty() && !escaped_path.starts_with('/') && !host.is_empty() {
            output.push('/');
        }
        if output.is_empty() && escaped_path.split('/').next()?.contains(':') {
            output.push_str("./");
        }
        output.push_str(&escaped_path);
    }
    let mut hostname = host.as_slice();
    if let Some(index) = hostname.iter().rposition(|byte| *byte == b':')
        && hostname[index + 1..].iter().all(u8::is_ascii_digit)
    {
        hostname = &hostname[..index];
    }
    if hostname.starts_with(b"[") && hostname.ends_with(b"]") {
        hostname = &hostname[1..hostname.len() - 1];
    }
    Some(Address {
        text: output,
        has_hostname: !hostname.is_empty(),
    })
}

#[derive(Clone, Copy)]
enum Component {
    Path,
    Host,
    Zone,
}

fn allowed(byte: u8, component: Component) -> bool {
    byte.is_ascii_alphanumeric()
        || match component {
            Component::Path => b"-._~$&+,/:;=@".contains(&byte),
            Component::Host | Component::Zone => b"-._~!$&'()*+,;=:[]<>\"".contains(&byte),
        }
}

fn unescape(value: &str, component: Component) -> Option<Vec<u8>> {
    let mut output = Vec::with_capacity(value.len());
    let mut bytes = value.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let high = (bytes.next()? as char).to_digit(16)?;
            let low = (bytes.next()? as char).to_digit(16)?;
            let decoded = (high * 16 + low) as u8;
            match component {
                Component::Host if decoded < 128 && decoded != b'%' => return None,
                Component::Zone
                    if decoded != b'%' && decoded != b' ' && !allowed(decoded, Component::Host) =>
                {
                    return None;
                }
                _ => {}
            }
            output.push(decoded);
        } else {
            if matches!(component, Component::Host | Component::Zone)
                && byte < 128
                && !allowed(byte, component)
            {
                return None;
            }
            output.push(byte);
        }
    }
    Some(output)
}

fn escape(value: &[u8], component: Component) -> String {
    use std::fmt::Write;
    let mut output = String::new();
    for &byte in value {
        if allowed(byte, component) {
            output.push(byte as char);
        } else {
            let _ = write!(output, "%{byte:02X}");
        }
    }
    output
}

fn valid_port(value: &str) -> bool {
    value.is_empty()
        || value
            .strip_prefix(':')
            .is_some_and(|port| port.bytes().all(|byte| byte.is_ascii_digit()))
}

fn parse_host(value: &str, scheme: &str) -> Option<Vec<u8>> {
    if let Some(bracketed) = value.strip_prefix('[') {
        if bracketed.contains('[') {
            return None;
        }
        let end = bracketed.rfind(']')?;
        let suffix = &bracketed[end + 1..];
        if !valid_port(suffix) {
            return None;
        }
        let address = &bracketed[..end];
        let address = if let Some(index) = address.find("%25") {
            let mut host = unescape(&address[..index], Component::Host)?;
            host.extend(unescape(&address[index..], Component::Zone)?);
            host
        } else {
            unescape(address, Component::Host)?
        };
        let ip_end = address
            .iter()
            .position(|byte| *byte == b'%')
            .unwrap_or(address.len());
        if ip_end < address.len() && ip_end + 1 == address.len() {
            return None;
        }
        std::str::from_utf8(&address[..ip_end])
            .ok()?
            .parse::<std::net::Ipv6Addr>()
            .ok()?;
        let mut host = vec![b'['];
        host.extend(address);
        host.push(b']');
        host.extend_from_slice(suffix.as_bytes());
        return Some(host);
    }
    if value.contains('[') {
        return None;
    }
    let colon = if matches!(scheme, "http" | "https") {
        value.find(':')
    } else {
        value.rfind(':')
    };
    if colon.is_some_and(|index| !valid_port(&value[index..])) {
        return None;
    }
    unescape(value, Component::Host)
}
