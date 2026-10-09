use pixiv_app::login_bridge::{BridgeError, LoginBridgeHooks, RelayCleanup};
use pixiv_sdk::transport::{Request, Response, Transport};
use std::{
    collections::VecDeque,
    io::{Read, Write},
    net::TcpStream,
    sync::{Arc, Mutex},
    thread::JoinHandle,
    time::Duration,
};

#[derive(Default)]
pub struct Observation {
    pub diagnostics: String,
    pub output: String,
    pub events: Vec<String>,
    pub login_url: String,
    pub address: String,
    pub final_status: Option<u16>,
    pub http: Vec<u8>,
    pub clients: Vec<JoinHandle<()>>,
}
#[derive(Clone)]
pub struct Hooks {
    pub observed: Arc<Mutex<Observation>>,
    pub submit: bool,
    pub prompt: bool,
    pub inputs: Arc<Mutex<VecDeque<String>>>,
    pub fail_ensure: bool,
    pub fail_install: bool,
    pub fail_open: bool,
}
impl Hooks {
    pub fn new(submit: bool) -> Self {
        Self {
            observed: Arc::new(Mutex::new(Observation::default())),
            submit,
            prompt: false,
            inputs: Arc::new(Mutex::new(VecDeque::new())),
            fail_ensure: false,
            fail_install: false,
            fail_open: false,
        }
    }
    pub fn join_clients(&self) {
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
        }
        if let Some(page) = message.strip_prefix("Manual fallback page: ") {
            let page = url::Url::parse(page.trim()).unwrap();
            let address = format!("{}:{}", page.host_str().unwrap(), page.port().unwrap());
            observed.address = address.clone();
            if self.submit {
                let state = url::Url::parse(&observed.login_url)
                    .unwrap()
                    .query_pairs()
                    .find(|(key, _)| key == "state")
                    .unwrap()
                    .1
                    .into_owned();
                let query = url::form_urlencoded::Serializer::new(String::new())
                    .append_pair("code", "synthetic-code")
                    .append_pair("state", &state)
                    .finish();
                let captured = self.observed.clone();
                observed.clients.push(std::thread::spawn(move || {
                    let response = request(&address, "GET", &format!("/callback?{query}"), "");
                    let status = String::from_utf8_lossy(&response)
                        .split_whitespace()
                        .nth(1)
                        .unwrap()
                        .parse()
                        .unwrap();
                    let mut captured = captured.lock().unwrap();
                    captured.final_status = Some(status);
                    captured.http = response;
                }));
            }
        }
    }
    fn open_browser(&self, _: &str) -> Result<(), BridgeError> {
        self.observed.lock().unwrap().events.push("open".into());
        if self.fail_open {
            Err(std::io::Error::other("synthetic opener failure").into())
        } else {
            Ok(())
        }
    }
    fn ensure_scheme_relay(&self, _: &pixiv_app::lifecycle::Context) -> Result<(), BridgeError> {
        self.observed.lock().unwrap().events.push("ensure".into());
        if self.fail_ensure {
            Err(std::io::Error::other("synthetic persistent failure").into())
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
        if self.fail_install {
            return Err(std::io::Error::other("synthetic temporary failure").into());
        }
        let observed = self.observed.clone();
        Ok(Some(Box::new(move || {
            observed.lock().unwrap().events.push("cleanup".into())
        })))
    }
    fn can_prompt(&self) -> bool {
        self.prompt
    }
    fn prompt_input(&self, message: &str, default: &str) -> Result<String, BridgeError> {
        assert_eq!(
            message,
            "Paste the returned Pixiv sign-in address, relay address, or value"
        );
        assert_eq!(default, "");
        self.observed.lock().unwrap().events.push("prompt".into());
        self.inputs
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| std::io::Error::other("synthetic prompt exhausted").into())
    }
    fn output(&self, message: &str) {
        self.observed.lock().unwrap().output.push_str(message);
    }
}

pub fn request(address: &str, method: &str, path: &str, body: &str) -> Vec<u8> {
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(stream, "{method} {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    response
}

#[derive(Clone)]
pub struct OAuth {
    pub observed: Arc<Mutex<Observation>>,
    pub status: u16,
    pub delay: Duration,
}
impl Transport for OAuth {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        self.observed.lock().unwrap().events.push("oauth".into());
        assert_eq!(request.method.as_str(), "POST");
        assert_eq!(request.url, "https://oauth.secure.pixiv.net/auth/token");
        let field = |name| {
            request
                .parameters
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
        };
        assert_eq!(field("grant_type"), Some("authorization_code"));
        assert_eq!(field("code"), Some("synthetic-code"));
        assert_eq!(field("include_policy"), Some("true"));
        assert!(field("code_verifier").is_some_and(|value| !value.is_empty()));
        assert!(
            request
                .headers
                .iter()
                .all(|(name, _)| !name.eq_ignore_ascii_case("authorization"))
        );
        tokio::time::sleep(self.delay).await;
        Ok(Response {
            status: self.status,
            retry_after: None,
            body: if self.status == 200 {
                serde_json::json!({"access_token":"synthetic-access", "refresh_token":"synthetic-refresh", "user":{"id":42,"name":"fixture-name"}})
            } else {
                serde_json::json!({"error":"synthetic-failure"})
            },
        })
    }
}
