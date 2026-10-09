use pixiv_sdk::{
    Reason,
    environment_proxy::ProxyEnvironment,
    oauth::LoginSession,
    transport::{HttpTransport, Request, Transport},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::TcpListener,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

#[derive(Deserialize, Serialize)]
struct Case {
    name: String,
    url: String,
    env: BTreeMap<String, String>,
    #[serde(default)]
    after_build: BTreeMap<String, String>,
    #[serde(default)]
    after_first: BTreeMap<String, String>,
    route: String,
    #[serde(default)]
    error: String,
    #[serde(default)]
    login: bool,
    #[serde(default)]
    disabled: bool,
    #[serde(default)]
    requests: usize,
}

#[test]
fn shared_proxy_policy_keeps_existing_go_selector_contract() {
    #[derive(Deserialize)]
    struct PolicyCase {
        name: String,
        url: String,
        env: BTreeMap<String, String>,
        proxy: String,
        error: String,
    }
    #[derive(Deserialize)]
    struct Fixture {
        proxy: Vec<PolicyCase>,
    }
    let fixture: Fixture = serde_json::from_str(include_str!(
        "../../pixiv-app/tests/fixtures/handoff_client.json"
    ))
    .unwrap();
    assert_eq!(fixture.proxy.len(), 21);
    for row in fixture.proxy {
        let result = ProxyEnvironment::from_values(&row.env).proxy_for_url(&row.url);
        if row.error.is_empty() {
            assert_eq!(
                result.unwrap().unwrap_or_default(),
                row.proxy,
                "{}",
                row.name
            );
        } else {
            assert_eq!(result.unwrap_err().to_string(), row.error, "{}", row.name);
        }
    }
}

#[test]
fn native_environment_transport_matches_isolated_go_login_default() {
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("fixtures/login_environment_proxy.json")).unwrap();
    assert_eq!(cases.len(), 16);
    for row in cases {
        let mut child = Command::new(std::env::current_exe().unwrap());
        child.args([
            "--exact",
            "--ignored",
            "isolated_environment_transport_child",
            "--test-threads=1",
        ]);
        for key in [
            "HTTP_PROXY",
            "http_proxy",
            "HTTPS_PROXY",
            "https_proxy",
            "NO_PROXY",
            "no_proxy",
            "ALL_PROXY",
            "all_proxy",
            "REQUEST_METHOD",
        ] {
            child.env_remove(key);
        }
        child.env(
            "PIXIV_MIGRATION_LOGIN_PROXY_CASE",
            serde_json::to_string(&row).unwrap(),
        );
        let output = child.output().unwrap();
        assert!(
            output.status.success(),
            "{}: {}\n{}",
            row.name,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
}

struct Servers {
    stop: Arc<AtomicBool>,
    workers: Vec<thread::JoinHandle<()>>,
}
impl Drop for Servers {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        for worker in self.workers.drain(..) {
            worker.join().unwrap();
        }
    }
}
impl Servers {
    fn start(
        &mut self,
        route: &'static str,
        login: bool,
        routes: mpsc::Sender<&'static str>,
    ) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let stop = self.stop.clone();
        self.workers.push(thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                let mut socket = match listener.accept() {
                    Ok((socket, _)) => socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(error) => panic!("synthetic accept: {error}"),
                };
                socket.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                let mut head = Vec::new();
                while !head.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    socket.read_exact(&mut byte).unwrap();
                    head.push(byte[0]);
                    assert!(head.len() < 65536);
                }
                let head = String::from_utf8(head).unwrap();
                let line = head.lines().next().unwrap();
                let wanted = match (route, login) {
                    ("direct", _) => "GET /resource HTTP/1.1",
                    (_, true) => "CONNECT oauth.secure.pixiv.net:443 HTTP/1.1",
                    _ => "GET http://public.example/resource HTTP/1.1",
                };
                assert_eq!(line, wanted);
                routes.send(route).unwrap();
                if login {
                    socket
                        .write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .unwrap();
                } else {
                    let body = format!("{{\"route\":\"{route}\"}}");
                    write!(
                        socket,
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body,
                    )
                    .unwrap();
                }
            }
        }));
        format!("http://{address}")
    }
}

fn set_environment(values: &BTreeMap<String, String>, replace: &impl Fn(&str) -> String) {
    for (key, value) in values {
        // The isolated child runs one test, and its synthetic server workers never read environment values.
        unsafe { std::env::set_var(key, replace(value)) };
    }
}

#[test]
#[ignore = "child-only environment helper"]
fn isolated_environment_transport_child() {
    let Ok(raw) = std::env::var("PIXIV_MIGRATION_LOGIN_PROXY_CASE") else {
        return;
    };
    let row: Case = serde_json::from_str(&raw).unwrap();
    let (routes, received) = mpsc::channel();
    let mut servers = Servers {
        stop: Arc::new(AtomicBool::new(false)),
        workers: Vec::new(),
    };
    let upper = servers.start("upper", row.login, routes.clone());
    let lower = servers.start("lower", row.login, routes.clone());
    let direct = servers.start("direct", false, routes);
    let replace = |value: &str| {
        value
            .replace("<UPPER_FTP>", &upper.replacen("http:", "ftp:", 1))
            .replace("<UPPER_SOCKS4>", &upper.replacen("http:", "socks4:", 1))
            .replace("<UPPER>", &upper)
            .replace("<LOWER>", &lower)
            .replace("<DIRECT>", &direct)
    };
    set_environment(&row.env, &replace);
    let transport = if row.disabled {
        HttpTransport::new(None)
    } else {
        HttpTransport::new_with_environment_proxy()
    }
    .unwrap();
    set_environment(&row.after_build, &replace);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    for index in 0..row.requests.max(1) {
        runtime.block_on(async {
            let result = tokio::time::timeout(Duration::from_secs(3), async {
                if row.login {
                    LoginSession::begin()
                        .unwrap()
                        .complete(&transport, "synthetic-code")
                        .await
                        .map(|_| serde_json::Value::Null)
                } else {
                    transport
                        .send(Request {
                            method: reqwest::Method::GET,
                            url: replace(&row.url),
                            headers: Vec::new(),
                            parameters: Vec::new(),
                            operation: "environment_request",
                        })
                        .await
                        .map(|response| response.body)
                }
            })
            .await
            .expect("synthetic request timed out");
            if row.error.is_empty() {
                assert_eq!(result.unwrap()["route"], row.route);
            } else {
                let error = result.unwrap_err();
                assert_eq!(error.code, Reason::UpstreamUnavailable);
                assert_eq!(
                    error.detail.as_deref(),
                    Some(if row.error == "missing_proxy_port" {
                        "transport: connection_refused"
                    } else {
                        "transport: unknown"
                    })
                );
                assert_eq!(error.transport, Some(pixiv_sdk::error::TransportKind::Http));
                assert!(!error.to_string().contains("synthetic-code"));
            }
        });
        if !row.route.is_empty() {
            assert_eq!(
                received.recv_timeout(Duration::from_secs(1)).unwrap(),
                row.route
            );
        }
        if index == 0 {
            set_environment(&row.after_first, &replace);
        }
    }
    assert!(received.try_recv().is_err());
}
