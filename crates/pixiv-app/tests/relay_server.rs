use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use pixiv_app::{
    lifecycle::Context,
    login_bridge::{BridgeError, LoginBridgeHooks},
    relay_server::{
        RelayLoginResult, RelayServerOptions, wait_for_handoff_relay_login_code_on_listener,
    },
};
use reqwest::{Client, Response, redirect::Policy};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{net::TcpListener, sync::mpsc, task::JoinHandle, time::timeout};

const CALLBACK: &str = "pixiv://account/login?code=synthetic-relay-code";
const AUTHORIZATION: &str = "https://app-api.pixiv.net/web/v1/login?client=pixiv-android&code_challenge_method=S256&code_challenge=synthetic-challenge&state=synthetic-state";
#[derive(Deserialize)]
struct RequestCase {
    name: String,
    body: String,
}
#[derive(Deserialize)]
struct Fixture {
    rejected_start: Vec<RequestCase>,
    accepted_start: String,
    accepted_callback: String,
}
fn fixture() -> Fixture {
    serde_json::from_str(include_str!("fixtures/relay_server.json")).unwrap()
}
#[derive(Default)]
struct Hooks {
    output: Mutex<String>,
    ready: Mutex<Option<mpsc::UnboundedSender<String>>>,
}
impl LoginBridgeHooks for Hooks {
    fn diagnostic(&self, message: &str) {
        self.output.lock().unwrap().push_str(message);
        if let Some(url) = message.strip_prefix("Open remote Pixiv login session:\n")
            && let Some(sender) = self.ready.lock().unwrap().take()
        {
            sender.send(url.trim().to_owned()).unwrap();
        }
    }
    fn open_browser(&self, _: &str) -> Result<(), BridgeError> {
        panic!("relay server must not open browser")
    }
}
struct Relay {
    context: Context,
    base: String,
    session: String,
    proof: String,
    hooks: Arc<Hooks>,
    client: Client,
    task: JoinHandle<Result<RelayLoginResult, BridgeError>>,
}
async fn start() -> Relay {
    start_public("").await
}
async fn start_public(public_url: &str) -> Relay {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let context = Context::new();
    let child_context = context.clone();
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let hooks = Arc::new(Hooks {
        ready: Mutex::new(Some(sender)),
        ..Hooks::default()
    });
    let child_hooks = hooks.clone();
    let options = RelayServerOptions {
        public_url: if public_url.is_empty() {
            base.clone()
        } else {
            public_url.to_owned()
        },
        listen_addr: listener.local_addr().unwrap().to_string(),
        tls_cert_file: String::new(),
        tls_key_file: String::new(),
    };
    let task = tokio::spawn(async move {
        wait_for_handoff_relay_login_code_on_listener(
            &child_context,
            options,
            Some(Arc::new(|raw| raw == CALLBACK)),
            AUTHORIZATION,
            child_hooks,
            listener,
        )
        .await
    });
    let session_url = timeout(Duration::from_secs(5), receiver.recv())
        .await
        .unwrap()
        .unwrap();
    let client = Client::builder().redirect(Policy::none()).build().unwrap();
    let session_path = url::Url::parse(&session_url)
        .unwrap()
        .path_segments()
        .unwrap()
        .next_back()
        .unwrap()
        .to_owned();
    let response = client
        .get(format!("{base}/session/{session_path}"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 303);
    let deep = url::Url::parse(
        response
            .headers()
            .get("location")
            .unwrap()
            .to_str()
            .unwrap(),
    )
    .unwrap();
    let values: std::collections::HashMap<_, _> = deep
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if public_url.is_empty() {
        assert_eq!(values["origin"], base)
    } else {
        assert_eq!(
            values["origin"],
            "https://relay.example/~/%2A/%E6%97%A5%E6%9C%AC%20%E8%AA%9E"
        )
    };
    Relay {
        context,
        base,
        session: values["session"].clone(),
        proof: values["access"].clone(),
        hooks,
        client,
        task,
    }
}
impl Relay {
    async fn post(&self, segment: &str, body: &str) -> Response {
        self.client
            .post(format!("{}/{}/{}", self.base, segment, self.session))
            .header("content-type", "application/json")
            .body(body.to_owned())
            .send()
            .await
            .unwrap()
    }
    fn body(&self, template: &str) -> String {
        template
            .replace("$PROOF", &self.proof)
            .replace("$CALLBACK", CALLBACK)
    }
}
#[tokio::test]
async fn json_capability_and_callback_order_match_go() {
    let relay = start().await;
    for id in [&relay.session, &relay.proof] {
        assert_eq!(URL_SAFE_NO_PAD.decode(id).unwrap().len(), 32);
    }
    assert_ne!(relay.session, relay.proof);
    let output = relay.hooks.output.lock().unwrap().clone();
    assert!(!output.contains(&relay.proof));
    assert!(!output.contains(AUTHORIZATION));
    let fixture = fixture();
    for case in fixture.rejected_start {
        let response = relay.post("start", &relay.body(&case.body)).await;
        assert_eq!(response.status(), 401, "{}", case.name);
        assert_eq!(
            response.text().await.unwrap(),
            "invalid remote login session\n",
            "{}",
            case.name
        );
    }
    let callback = relay.body(&fixture.accepted_callback);
    let response = relay.post("callback", &callback).await;
    assert_eq!(response.status(), 409);
    assert_eq!(
        response.text().await.unwrap(),
        "remote login session is not ready\n"
    );
    for _ in 0..2 {
        let response = relay
            .post("start", &relay.body(&fixture.accepted_start))
            .await;
        assert_eq!(response.status(), 200);
        assert_eq!(
            response.text().await.unwrap(),
            format!(
                "{{\"authorization_url\":\"{}\"}}\n",
                AUTHORIZATION.replace('&', "\\u0026")
            )
        );
    }
    for (callback, message) in [
        (
            "https://example.invalid/?code=hidden",
            "invalid Pixiv login result",
        ),
        (
            "pixiv://account/login?code=other",
            "login result does not match this session",
        ),
    ] {
        let response = relay
            .post(
                "callback",
                &format!(
                    "{{\"proof\":\"{}\",\"callback_url\":\"{}\"}}",
                    relay.proof, callback
                ),
            )
            .await;
        assert_eq!(response.status(), 400);
        assert_eq!(response.text().await.unwrap(), format!("{message}\n"));
    }
    relay.context.cancel();
    let error = match timeout(Duration::from_secs(5), relay.task)
        .await
        .unwrap()
        .unwrap()
    {
        Ok(_) => panic!("unsubmitted relay returned code"),
        Err(error) => error,
    };
    assert_eq!(error.to_string(), "context canceled");
}
#[tokio::test]
async fn final_page_latch_and_single_claim_match_go() {
    for success in [true, false] {
        let relay = start().await;
        let fixture = fixture();
        let response = relay
            .post("start", &relay.body(&fixture.accepted_start))
            .await;
        assert_eq!(response.status(), 200);
        let _ = response.text().await.unwrap();
        let response = relay
            .post("callback", &relay.body(&fixture.accepted_callback))
            .await;
        assert_eq!(response.status(), 200);
        let result_url = response
            .headers()
            .get("x-pixiv-relay-result-url")
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let result_id = url::Url::parse(&result_url)
            .unwrap()
            .path_segments()
            .unwrap()
            .next_back()
            .unwrap()
            .to_owned();
        assert_eq!(URL_SAFE_NO_PAD.decode(&result_id).unwrap().len(), 32);
        assert_ne!(result_id, relay.session);
        assert_ne!(result_id, relay.proof);
        let replay = relay
            .post("callback", &relay.body(&fixture.accepted_callback))
            .await;
        assert_eq!(replay.status(), 409);
        assert_eq!(
            replay.text().await.unwrap(),
            "login result has already been received\n"
        );
        let replay = relay
            .post("start", &relay.body(&fixture.accepted_start))
            .await;
        assert_eq!(replay.status(), 409);
        let _ = replay.text().await.unwrap();
        let outcome = timeout(Duration::from_secs(5), relay.task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(outcome.code, CALLBACK);
        let server = outcome.server.clone();
        let mut notification = tokio::spawn(async move { server.notify_final(success).await });
        let mut body = tokio::spawn(async move { response.text().await.unwrap() });
        assert!(
            timeout(Duration::from_millis(30), &mut notification)
                .await
                .is_err()
        );
        assert!(!body.is_finished());
        let server = outcome.server.clone();
        let second_notification = tokio::spawn(async move { server.notify_final(!success).await });
        let page = relay.client.get(&result_url).send().await.unwrap();
        assert_eq!(page.status(), if success { 200 } else { 400 });
        let html = page.text().await.unwrap();
        let frozen_pages: serde_json::Value =
            serde_json::from_str(include_str!("fixtures/login_page.json")).unwrap();
        let expected = frozen_pages
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["name"] == if success { "success" } else { "failure" })
            .unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(html.as_bytes())),
            expected["sha256"].as_str().unwrap()
        );

        for secret in [CALLBACK, AUTHORIZATION, &relay.proof] {
            assert!(!html.contains(secret));
        }
        timeout(Duration::from_secs(5), notification)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            timeout(Duration::from_secs(5), &mut body)
                .await
                .unwrap()
                .unwrap(),
            format!("{{\"success\":{success}}}\n")
        );
        let page = relay.client.get(&result_url).send().await.unwrap();
        assert_eq!(page.status(), 409);
        assert_eq!(
            page.text().await.unwrap(),
            "login result has already been opened\n"
        );
        timeout(Duration::from_secs(5), second_notification)
            .await
            .unwrap()
            .unwrap();
        outcome.server.notify_final(!success).await;
        outcome.server.cleanup().await;
        outcome.server.cleanup().await;
    }
}

#[tokio::test]
async fn mux_paths_match_installed_go_toolchain() {
    let relay = start().await;
    for (method, path, location, status) in [
        ("GET", "/session".to_owned(), "/session/".to_owned(), 307),
        ("POST", "/start".to_owned(), "/start/".to_owned(), 307),
        (
            "GET",
            format!("//session/{}", relay.session),
            format!("/session/{}", relay.session),
            307,
        ),
        (
            "GET",
            format!("/session/./{}", relay.session),
            format!("/session/{}", relay.session),
            307,
        ),
        (
            "CONNECT",
            "/session".to_owned(),
            "/session/".to_owned(),
            307,
        ),
        (
            "CONNECT",
            format!("/session/./{}", relay.session),
            String::new(),
            404,
        ),
        ("GET", "/session/wrong".to_owned(), String::new(), 404),
        (
            "POST",
            format!("/session/{}", relay.session),
            String::new(),
            404,
        ),
        (
            "GET",
            format!("/start/{}", relay.session),
            String::new(),
            404,
        ),
        (
            "GET",
            format!("/session/{}/", relay.session),
            String::new(),
            404,
        ),
    ] {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut stream =
            tokio::net::TcpStream::connect(relay.base.strip_prefix("http://").unwrap())
                .await
                .unwrap();
        stream.write_all(format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: 0\r\n\r\n").as_bytes()).await.unwrap();
        let mut bytes = Vec::new();
        timeout(Duration::from_secs(5), stream.read_to_end(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        let response = String::from_utf8(bytes).unwrap();
        assert_eq!(
            response
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap(),
            status.to_string(),
            "{path}"
        );
        let actual = response
            .lines()
            .find_map(|line| {
                line.split_once(':')
                    .filter(|(name, _)| name.eq_ignore_ascii_case("location"))
                    .map(|(_, value)| value.trim())
            })
            .unwrap_or("");
        assert_eq!(actual, location, "{path}");
    }
    relay.context.cancel();
    assert!(
        timeout(Duration::from_secs(5), relay.task)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
}

#[tokio::test]
async fn abandoned_callback_releases_final_latch() {
    let relay = start().await;
    let fixture = fixture();
    let response = relay
        .post("start", &relay.body(&fixture.accepted_start))
        .await;
    let _ = response.text().await.unwrap();
    let response = relay
        .post("callback", &relay.body(&fixture.accepted_callback))
        .await;
    assert_eq!(response.status(), 200);
    let outcome = timeout(Duration::from_secs(5), relay.task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    drop(response);
    timeout(Duration::from_secs(5), outcome.server.notify_final(true))
        .await
        .unwrap();
    outcome.server.cleanup().await;
}

#[tokio::test]
async fn caller_deadline_and_tls_failure_are_redacted() {
    use pixiv_app::relay_server::wait_for_handoff_relay_login_code;
    let context = Context::with_deadline(std::time::Instant::now() + Duration::from_millis(30));
    let options = RelayServerOptions {
        public_url: "http://relay.example".into(),
        listen_addr: "127.0.0.1:0".into(),
        ..Default::default()
    };
    let result = timeout(
        Duration::from_secs(5),
        wait_for_handoff_relay_login_code(
            &context,
            options,
            None,
            AUTHORIZATION,
            Arc::new(Hooks::default()),
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        match result {
            Ok(_) => panic!("deadline returned callback"),
            Err(error) => error.to_string(),
        },
        "context deadline exceeded"
    );
    let directory = tempfile::tempdir().unwrap();
    let cert = directory.path().join("private-cert.pem");
    let key = directory.path().join("private-key.pem");
    std::fs::write(&cert, "synthetic invalid PEM").unwrap();
    std::fs::write(&key, "synthetic invalid PEM").unwrap();
    let options = RelayServerOptions {
        public_url: "https://relay.example".into(),
        listen_addr: "127.0.0.1:0".into(),
        tls_cert_file: cert.to_str().unwrap().into(),
        tls_key_file: key.to_str().unwrap().into(),
    };
    let hooks = Arc::new(Hooks::default());
    let result = timeout(
        Duration::from_secs(5),
        wait_for_handoff_relay_login_code(
            &Context::new(),
            options,
            None,
            AUTHORIZATION,
            hooks.clone(),
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        match result {
            Ok(_) => panic!("invalid TLS returned callback"),
            Err(error) => error.to_string(),
        },
        "remote login relay server failed; verify its listener and TLS configuration"
    );
    let output = hooks.output.lock().unwrap();
    assert!(!output.contains(directory.path().to_str().unwrap()));
    assert!(!output.contains(AUTHORIZATION));
}

#[test]
fn configured_options_precedence_and_validation_match_go() {
    use pixiv_app::{
        config::Snapshot,
        relay_server::{RelayServerFlagOverrides, configured_relay_server_options},
    };
    #[derive(Deserialize)]
    struct Case {
        name: String,
        public: String,
        listen: String,
        cert: String,
        key: String,
        error: String,
        enabled: bool,
        #[serde(default)]
        clear: Vec<String>,
    }
    #[derive(Deserialize)]
    struct Cases {
        options: Vec<Case>,
    }
    let fixture: Cases = serde_json::from_str(include_str!("fixtures/relay_server.json")).unwrap();
    for case in fixture.options {
        let mut runtime = Snapshot::parse("", std::collections::BTreeMap::new())
            .unwrap()
            .runtime()
            .unwrap();
        runtime.login_relay_public_url = case.public.clone();
        runtime.login_relay_listen_addr = case.listen.clone();
        runtime.login_relay_tls_cert_file = case.cert.clone();
        runtime.login_relay_tls_key_file = case.key.clone();
        let mut flags = RelayServerFlagOverrides::default();
        for flag in &case.clear {
            match flag.as_str() {
                "relay-public-url" => flags.public_url = Some(String::new()),
                "relay-listen-addr" => flags.listen_addr = Some(String::new()),
                "relay-tls-cert-file" => flags.tls_cert_file = Some(String::new()),
                "relay-tls-key-file" => flags.tls_key_file = Some(String::new()),
                _ => panic!("unknown fixture flag"),
            }
        }
        let result = configured_relay_server_options(&flags, &runtime);
        if case.error.is_empty() {
            let result = result.unwrap();
            assert_eq!(result.is_some(), case.enabled, "{}", case.name);
            if let Some(options) = result {
                assert_eq!(options.public_url, case.public);
                assert_eq!(options.listen_addr, case.listen);
                assert_eq!(options.tls_cert_file, case.cert);
                assert_eq!(options.tls_key_file, case.key)
            }
        } else {
            assert_eq!(result.unwrap_err().to_string(), case.error, "{}", case.name)
        }
    }
}

#[test]
fn deep_link_query_bytes_match_go() {
    #[derive(Deserialize)]
    struct Case {
        origin: String,
        session: String,
        proof: String,
        expected: String,
    }
    #[derive(Deserialize)]
    struct Cases {
        deep_links: Vec<Case>,
    }
    let fixture: Cases = serde_json::from_str(include_str!("fixtures/relay_server.json")).unwrap();
    for case in fixture.deep_links {
        assert_eq!(
            pixiv_app::relay_server::handoff_relay_deep_link(
                &case.origin,
                &case.session,
                &case.proof
            ),
            case.expected
        )
    }
}

#[tokio::test]
async fn claimed_result_disconnect_releases_final_latch() {
    use tokio::io::AsyncWriteExt;
    let relay = start().await;
    let fixture = fixture();
    let response = relay
        .post("start", &relay.body(&fixture.accepted_start))
        .await;
    let _ = response.text().await.unwrap();
    let response = relay
        .post("callback", &relay.body(&fixture.accepted_callback))
        .await;
    let result_url = response
        .headers()
        .get("x-pixiv-relay-result-url")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let outcome = timeout(Duration::from_secs(5), relay.task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let mut page = tokio::net::TcpStream::connect(relay.base.strip_prefix("http://").unwrap())
        .await
        .unwrap();
    let path = url::Url::parse(&result_url).unwrap().path().to_owned();
    page.write_all(
        format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").as_bytes(),
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    drop(page);
    timeout(Duration::from_secs(5), outcome.server.notify_final(true))
        .await
        .unwrap();
    assert_eq!(response.text().await.unwrap(), "{\"success\":true}\n");
    outcome.server.cleanup().await;
}

#[tokio::test]
async fn synthetic_trusted_tls_serves_session_and_start() {
    let directory = tempfile::tempdir().unwrap();
    let cert = directory.path().join("relay.pem");
    let key = directory.path().join("relay.key");
    std::fs::write(&cert, include_bytes!("fixtures/relay_server_cert.pem")).unwrap();
    std::fs::write(&key, include_bytes!("fixtures/relay_server_key.pem")).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("https://{}", listener.local_addr().unwrap());
    let context = Context::new();
    let child_context = context.clone();
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let hooks = Arc::new(Hooks {
        ready: Mutex::new(Some(sender)),
        ..Default::default()
    });
    let options = RelayServerOptions {
        public_url: base.clone(),
        listen_addr: listener.local_addr().unwrap().to_string(),
        tls_cert_file: cert.to_str().unwrap().into(),
        tls_key_file: key.to_str().unwrap().into(),
    };
    let task = tokio::spawn(async move {
        wait_for_handoff_relay_login_code_on_listener(
            &child_context,
            options,
            None,
            AUTHORIZATION,
            hooks,
            listener,
        )
        .await
    });
    let session_url = timeout(Duration::from_secs(5), receiver.recv())
        .await
        .unwrap()
        .unwrap();
    let client = Client::builder()
        .redirect(Policy::none())
        .add_root_certificate(
            reqwest::Certificate::from_pem(include_bytes!("fixtures/relay_server_cert.pem"))
                .unwrap(),
        )
        .build()
        .unwrap();
    let response = client.get(session_url).send().await.unwrap();
    assert_eq!(response.status(), 303);
    let deep = url::Url::parse(
        response
            .headers()
            .get("location")
            .unwrap()
            .to_str()
            .unwrap(),
    )
    .unwrap();
    let values: std::collections::HashMap<_, _> = deep
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    assert_eq!(values["origin"], base);
    let response = client
        .post(format!("{base}/start/{}", values["session"]))
        .body(format!("{{\"proof\":\"{}\"}}", values["access"]))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.text().await.unwrap(),
        format!(
            "{{\"authorization_url\":\"{}\"}}\n",
            AUTHORIZATION.replace('&', "\\u0026")
        )
    );
    context.cancel();
    let result = timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        match result {
            Ok(_) => panic!("TLS session produced callback"),
            Err(error) => error.to_string(),
        },
        "context canceled"
    );
}

#[tokio::test]
async fn native_special_origin_canonicalizes_before_deep_link() {
    let relay = start_public(" HTTPS://RELAY.EXAMPLE//~/*/日本 語/// ").await;
    relay.context.cancel();
    assert!(
        timeout(Duration::from_secs(5), relay.task)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
}

#[tokio::test]
async fn empty_port_binds_ephemeral_without_option_rejection() {
    use pixiv_app::relay_server::wait_for_handoff_relay_login_code;
    let context = Context::new();
    let child_context = context.clone();
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let hooks = Arc::new(Hooks {
        ready: Mutex::new(Some(sender)),
        ..Default::default()
    });
    let child_hooks = hooks.clone();
    let options = RelayServerOptions {
        public_url: "http://relay.example".into(),
        listen_addr: "127.0.0.1:".into(),
        ..Default::default()
    };
    let task = tokio::spawn(async move {
        wait_for_handoff_relay_login_code(&child_context, options, None, AUTHORIZATION, child_hooks)
            .await
    });
    let session_url = timeout(Duration::from_secs(5), receiver.recv())
        .await
        .unwrap()
        .unwrap();
    let log = hooks.output.lock().unwrap().clone();
    let address = log
        .lines()
        .next()
        .unwrap()
        .strip_prefix("Remote Pixiv login relay is listening on ")
        .unwrap()
        .strip_suffix('.')
        .unwrap();
    let socket: std::net::SocketAddr = address.parse().unwrap();
    assert!(socket.ip().is_loopback());
    assert_ne!(socket.port(), 0);
    let session = url::Url::parse(&session_url)
        .unwrap()
        .path_segments()
        .unwrap()
        .next_back()
        .unwrap()
        .to_owned();
    let client = Client::builder().redirect(Policy::none()).build().unwrap();
    let response = client
        .get(format!("http://{address}/session/{session}"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 303);
    context.cancel();
    let result = timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    assert!(result.is_err());
}

#[test]
fn explicit_flags_override_configuration() {
    use pixiv_app::{
        config::Snapshot,
        relay_server::{RelayServerFlagOverrides, configured_relay_server_options},
    };
    #[derive(Deserialize)]
    struct Values {
        public: String,
        listen: String,
        cert: String,
        key: String,
    }
    #[derive(Deserialize)]
    struct Priority {
        config: Values,
        flags: Values,
    }
    #[derive(Deserialize)]
    struct Fixture {
        flag_priority: Priority,
    }
    let fixture: Fixture =
        serde_json::from_str(include_str!("fixtures/relay_server.json")).unwrap();
    let config = fixture.flag_priority.config;
    let flags = fixture.flag_priority.flags;
    let mut runtime = Snapshot::parse("", std::collections::BTreeMap::new())
        .unwrap()
        .runtime()
        .unwrap();
    runtime.login_relay_public_url = config.public;
    runtime.login_relay_listen_addr = config.listen;
    runtime.login_relay_tls_cert_file = config.cert;
    runtime.login_relay_tls_key_file = config.key;
    let overrides = RelayServerFlagOverrides {
        public_url: Some(flags.public.clone()),
        listen_addr: Some(flags.listen.clone()),
        tls_cert_file: Some(flags.cert.clone()),
        tls_key_file: Some(flags.key.clone()),
    };
    let options = configured_relay_server_options(&overrides, &runtime)
        .unwrap()
        .unwrap();
    assert_eq!(options.public_url, flags.public);
    assert_eq!(options.listen_addr, flags.listen);
    assert_eq!(options.tls_cert_file, flags.cert);
    assert_eq!(options.tls_key_file, flags.key);
}
