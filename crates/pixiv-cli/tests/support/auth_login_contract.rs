use pixiv_app::login_bridge::{BridgeError, LoginBridgeHooks, RelayCleanup};
use pixiv_sdk::transport::{Request, Response, Transport};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    sync::{Arc, Mutex},
    thread::JoinHandle,
    time::Duration,
};

#[derive(Default)]
pub struct Observed {
    pub diagnostics: String,
    pub output: String,
    pub events: Vec<String>,
    pub login_url: String,
    pub address: String,
    pub session_url: String,
    pub final_status: u16,
    pub final_page: Vec<u8>,
    pub prompt_count: usize,
    pub clients: Vec<JoinHandle<()>>,
}
#[derive(Clone)]
pub struct Hooks {
    pub observed: Arc<Mutex<Observed>>,
    pub case: Value,
    pub fixture: Value,
}
impl Hooks {
    pub fn new(case: Value, fixture: Value) -> Self {
        Self {
            observed: Arc::new(Mutex::new(Observed::default())),
            case,
            fixture,
        }
    }
    pub fn join(&self) {
        let clients = std::mem::take(&mut self.observed.lock().unwrap().clients);
        for client in clients {
            client.join().unwrap();
        }
    }
}
impl LoginBridgeHooks for Hooks {
    fn diagnostic(&self, message: &str) {
        let mut observed = self.observed.lock().unwrap();
        observed.diagnostics.push_str(message);
        if let Some(url) = message.strip_prefix("Open this Pixiv login URL:\n") {
            observed.login_url = url.trim().into();
            assert_authorization_url(&observed.login_url, &self.fixture);
        }
        if let Some(page) = message.strip_prefix("Manual fallback page: ") {
            let url = url::Url::parse(page.trim()).unwrap();
            observed.address = format!("{}:{}", url.host_str().unwrap(), url.port().unwrap());
            let submission = self.case["submission"].as_str().unwrap();
            if matches!(submission, "callback" | "manual") {
                let address = observed.address.clone();
                let state = authorization_parameter(&observed.login_url, "state");
                let code = self.fixture["code"].as_str().unwrap().to_owned();
                let submission = submission.to_owned();
                let captured = self.observed.clone();
                observed.clients.push(std::thread::spawn(move || {
                    let callback = format!("pixiv://account/login?code={code}");
                    let response = if submission == "callback" {
                        let query = url::form_urlencoded::Serializer::new(String::new())
                            .append_pair("code", &code)
                            .append_pair("state", &state)
                            .finish();
                        request(&address, "GET", &format!("/callback?{query}"), "")
                    } else {
                        let body = url::form_urlencoded::Serializer::new(String::new())
                            .append_pair("login_result", &callback)
                            .finish();
                        request(&address, "POST", "/manual", &body)
                    };
                    let mut captured = captured.lock().unwrap();
                    captured.final_status = response.status;
                    captured.final_page = response.body;
                }));
            }
        }
        if let Some(address) = message.strip_prefix("Remote Pixiv login relay is listening on ") {
            observed.address = address.trim().trim_end_matches('.').into();
        }
        if let Some(session) = message.strip_prefix("Open remote Pixiv login session:\n") {
            observed.session_url = session.trim().into();
            if self.case["submission"] == "relay" {
                let address = observed.address.clone();
                let session = observed.session_url.clone();
                let captured = self.observed.clone();
                let fixture = self.fixture.clone();
                observed.clients.push(std::thread::spawn(move || {
                    remote_submission(address, session, fixture, captured)
                }));
            }
        }
    }
    fn open_browser(&self, url: &str) -> Result<(), BridgeError> {
        let mut observed = self.observed.lock().unwrap();
        if url.starts_with("https://accounts.pixiv.net/post-redirect?") {
            observed.events.push("open_relay".into());
        } else {
            assert_eq!(url, observed.login_url);
            observed.events.push("open".into());
        }
        if self.case["hook_errors"] == true {
            Err(std::io::Error::other("synthetic open failure").into())
        } else {
            Ok(())
        }
    }
    fn ensure_scheme_relay(&self, _: &pixiv_app::lifecycle::Context) -> Result<(), BridgeError> {
        self.observed.lock().unwrap().events.push("ensure".into());
        if self.case["hook_errors"] == true {
            Err(std::io::Error::other("synthetic ensure failure").into())
        } else {
            Ok(())
        }
    }
    fn install_scheme_relay(
        &self,
        _: &pixiv_app::lifecycle::Context,
        callback: &str,
    ) -> Result<Option<RelayCleanup>, BridgeError> {
        assert!(callback.starts_with("http://127.0.0.1:"));
        assert!(callback.ends_with("/callback"));
        self.observed.lock().unwrap().events.push("install".into());
        if self.case["hook_errors"] == true {
            return Err(std::io::Error::other("synthetic install failure").into());
        }
        let observed = self.observed.clone();
        Ok(Some(Box::new(move || {
            observed
                .lock()
                .unwrap()
                .events
                .push("temporary_cleanup".into())
        })))
    }
    fn can_prompt(&self) -> bool {
        self.observed
            .lock()
            .unwrap()
            .events
            .push("can_prompt".into());
        matches!(
            self.case["submission"].as_str(),
            Some("terminal" | "terminal_relay")
        )
    }
    fn prompt_input(&self, message: &str, default: &str) -> Result<String, BridgeError> {
        assert_eq!(
            message,
            "Paste the returned Pixiv sign-in address, relay address, or value"
        );
        assert_eq!(default, "");
        let mut observed = self.observed.lock().unwrap();
        observed.events.push("prompt".into());
        observed.prompt_count += 1;
        if self.case["submission"] == "terminal_relay" && observed.prompt_count == 1 {
            let start = format!(
                "https://app-api.pixiv.net/web/v1/users/auth/pixiv/start?code_challenge={}",
                authorization_parameter(&observed.login_url, "code_challenge")
            );
            let query = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("return_to", &start)
                .finish();
            Ok(format!("https://accounts.pixiv.net/post-redirect?{query}"))
        } else {
            Ok(self.fixture["code"].as_str().unwrap().into())
        }
    }
    fn output(&self, message: &str) {
        self.observed.lock().unwrap().output.push_str(message);
    }
}
pub struct OAuth {
    pub hooks: Hooks,
}
impl Transport for OAuth {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        let mut observed = self.hooks.observed.lock().unwrap();
        observed.events.push("oauth".into());
        assert_eq!(request.method.as_str(), "POST");
        assert_eq!(request.url, "https://oauth.secure.pixiv.net/auth/token");
        assert_eq!(request.operation, "Complete");
        let field = |name| {
            request
                .parameters
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
                .unwrap()
        };
        assert_eq!(request.parameters.len(), 7);
        assert_eq!(field("grant_type"), "authorization_code");
        assert_eq!(field("code"), self.hooks.fixture["code"].as_str().unwrap());
        assert_eq!(field("include_policy"), "true");
        let verifier = field("code_verifier");
        assert_eq!(verifier.len(), 86);
        assert_eq!(
            encode64(&Sha256::digest(verifier.as_bytes())),
            authorization_parameter(&observed.login_url, "code_challenge")
        );
        assert!(
            request
                .headers
                .iter()
                .all(|(name, _)| !name.eq_ignore_ascii_case("authorization"))
        );
        let status = self.hooks.case["oauth_status"].as_u64().unwrap() as u16;
        let body = serde_json::from_str(
            self.hooks.fixture[if status == 200 {
                "oauth_response"
            } else {
                "oauth_failure"
            }]
            .as_str()
            .unwrap(),
        )
        .unwrap();
        Ok(Response {
            status,
            retry_after: None,
            body,
        })
    }
}
fn encode64(bytes: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut result = String::new();
    let (mut buffer, mut bits) = (0u32, 0u32);
    for byte in bytes {
        buffer = (buffer << 8) | u32::from(*byte);
        bits += 8;
        while bits >= 6 {
            bits -= 6;
            result.push(CHARS[((buffer >> bits) & 63) as usize] as char);
        }
    }
    if bits > 0 {
        result.push(CHARS[((buffer << (6 - bits)) & 63) as usize] as char);
    }
    result
}
pub fn authorization_parameter(url: &str, name: &str) -> String {
    url::Url::parse(url)
        .unwrap()
        .query_pairs()
        .find(|(key, _)| key == name)
        .unwrap()
        .1
        .into_owned()
}
pub fn assert_authorization_url(url: &str, fixture: &Value) {
    let parsed = url::Url::parse(url).unwrap();
    assert_eq!(
        parsed.origin().ascii_serialization(),
        "https://app-api.pixiv.net"
    );
    assert_eq!(parsed.path(), "/web/v1/login");
    let keys = parsed
        .query_pairs()
        .map(|(key, _)| key.into_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        keys,
        ["client", "code_challenge", "code_challenge_method", "state"]
    );
    assert_eq!(authorization_parameter(url, "client"), "pixiv-android");
    assert_eq!(
        authorization_parameter(url, "code_challenge_method"),
        "S256"
    );
    for name in ["code_challenge", "state"] {
        assert_eq!(authorization_parameter(url, name).len(), 43);
    }
    let normalized = url
        .replace(
            &authorization_parameter(url, "code_challenge"),
            "<CHALLENGE>",
        )
        .replace(&authorization_parameter(url, "state"), "<STATE>");
    assert_eq!(normalized, fixture["authorization_url"].as_str().unwrap());
}

pub struct HttpReply {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}
fn connection(
    address: &str,
    method: &str,
    path: &str,
    body: &str,
) -> (BufReader<TcpStream>, HttpReply) {
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(stream,"{method} {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{body}",body.len()).unwrap();
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let status = line.split_whitespace().nth(1).unwrap().parse().unwrap();
    let mut headers = BTreeMap::new();
    loop {
        line.clear();
        reader.read_line(&mut line).unwrap();
        if line == "\r\n" {
            break;
        }
        assert!(!line.is_empty());
        let (key, value) = line.trim_end().split_once(':').unwrap();
        headers.insert(key.to_ascii_lowercase(), value.trim().into());
    }
    (
        reader,
        HttpReply {
            status,
            headers,
            body: Vec::new(),
        },
    )
}
fn read_body(reader: &mut BufReader<TcpStream>, reply: &mut HttpReply) {
    if reply
        .headers
        .get("transfer-encoding")
        .is_some_and(|value| value == "chunked")
    {
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let size = usize::from_str_radix(line.trim().split(';').next().unwrap(), 16).unwrap();
            if size == 0 {
                break;
            }
            let mut body = vec![0; size + 2];
            reader.read_exact(&mut body).unwrap();
            reply.body.extend_from_slice(&body[..size]);
        }
    } else {
        reader.read_to_end(&mut reply.body).unwrap();
    }
}
pub fn request(address: &str, method: &str, path: &str, body: &str) -> HttpReply {
    let (mut reader, mut reply) = connection(address, method, path, body);
    read_body(&mut reader, &mut reply);
    reply
}
fn endpoint(raw: &str) -> String {
    let url = url::Url::parse(raw).unwrap();
    let path = url.path();
    let start = ["/session/", "/result/"]
        .into_iter()
        .find_map(|segment| path.find(segment))
        .unwrap();
    path[start..].into()
}
fn remote_submission(
    address: String,
    session: String,
    fixture: Value,
    captured: Arc<Mutex<Observed>>,
) {
    let page = request(&address, "GET", &endpoint(&session), "");
    assert_eq!(page.status, 303);
    let start = url::Url::parse(&page.headers["location"]).unwrap();
    assert_eq!(start.scheme(), "pixiv");
    assert_eq!(start.path(), "/remote-login");
    let pairs = start.query_pairs().into_owned().collect::<BTreeMap<_, _>>();
    let id = &pairs["session"];
    let proof = &pairs["access"];
    let body = serde_json::json!({"proof":proof}).to_string();
    let start = request(&address, "POST", &format!("/start/{id}"), &body);
    assert_eq!(start.status, 200);
    let body: Value = serde_json::from_slice(&start.body).unwrap();
    let login = body["authorization_url"].as_str().unwrap();
    assert_authorization_url(login, &fixture);
    captured.lock().unwrap().login_url = login.into();
    let body=serde_json::json!({"proof":proof,"callback_url":format!("pixiv://account/login?code={}",fixture["code"].as_str().unwrap())}).to_string();
    let (mut reader, mut callback) =
        connection(&address, "POST", &format!("/callback/{id}"), &body);
    assert_eq!(callback.status, 200);
    let result = request(
        &address,
        "GET",
        &endpoint(&callback.headers["x-pixiv-relay-result-url"]),
        "",
    );
    read_body(&mut reader, &mut callback);
    let final_result: Value = serde_json::from_slice(&callback.body).unwrap();
    assert_eq!(final_result["success"], fixture_status(result.status));
    let mut captured = captured.lock().unwrap();
    captured.final_status = result.status;
    captured.final_page = result.body;
}
fn fixture_status(status: u16) -> Value {
    (status == 200).into()
}

pub fn normalize(observed: &Observed, relay: &str) -> String {
    let mut text = observed.diagnostics.clone();
    if !observed.login_url.is_empty() {
        text = text.replace(&observed.login_url, "<LOGIN_URL>");
    }
    if !observed.session_url.is_empty() {
        text = text.replace(&observed.session_url, "<RELAY>/session/<SESSION>");
    }
    if !observed.address.is_empty() {
        let port = observed.address.rsplit(':').next().unwrap();
        text = text.replace(
            &format!("ssh -N -L {port}:127.0.0.1:{port}"),
            "ssh -N -L <PORT>:127.0.0.1:<PORT>",
        );
        text = text.replace(&observed.address, "<ADDR>");
    }
    text.replace(relay, "<RELAY>")
}
pub fn runtime(case: &Value, replace: &impl Fn(&str) -> String) -> String {
    let mut text = String::new();
    for (table, fields) in [
        (
            "login",
            vec![
                ("login_open_browser", "open_browser"),
                ("login_use_after_login", "use_after_login"),
                ("login_relay_public_url", "relay_public_url"),
                ("login_relay_listen_addr", "relay_listen_addr"),
                ("login_relay_tls_cert_file", "relay_tls_cert_file"),
                ("login_relay_tls_key_file", "relay_tls_key_file"),
            ],
        ),
        ("pixiv.network", vec![("pixiv_proxy", "proxy_url")]),
    ] {
        text.push_str(&format!("[{table}]\n"));
        for (key, field) in fields {
            if let Some(value) = case["runtime"].get(key) {
                let value = if let Some(value) = value.as_str() {
                    Value::String(replace(value))
                } else {
                    value.clone()
                };
                text.push_str(&format!("{field}={value}\n"));
            }
        }
    }
    if let Some(value) = case["runtime"].get("https_proxy") {
        text.push_str(&format!("[network]\nhttps_proxy={value}\n"));
    }
    text
}
