#![cfg(target_os = "linux")]

use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    io::{self, Read, Write},
    net::{TcpListener, TcpStream},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

struct OwnedChild(Option<Child>);

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

struct FiniteProxy {
    address: String,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<Vec<String>>>,
}

impl FiniteProxy {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(6);
            let mut requests = Vec::new();
            while !stopped.load(Ordering::Acquire) && Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut connection, _)) => {
                        requests.push(read_proxy_request(&mut connection));
                        connection
                            .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                            .unwrap();
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => panic!("finite proxy accept: {error}"),
                }
            }
            requests
        });
        Self {
            address,
            stop,
            worker: Some(worker),
        }
    }

    fn finish(&mut self) -> Vec<String> {
        self.stop.store(true, Ordering::Release);
        self.worker.take().unwrap().join().unwrap()
    }
}

impl Drop for FiniteProxy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn read_proxy_request(connection: &mut TcpStream) -> String {
    connection
        .set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    let mut request = Vec::new();
    let mut byte = [0];
    while !request.ends_with(b"\r\n\r\n") {
        assert!(request.len() < 16_384, "unbounded dictionary proxy headers");
        assert_eq!(connection.read(&mut byte).unwrap(), 1);
        request.push(byte[0]);
    }
    String::from_utf8(request).unwrap()
}

fn fixture() -> Value {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/cli-dictionary-startup.json")).unwrap();
    assert_eq!(
        fixture["reference"],
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    assert_eq!(fixture["sources"].as_object().unwrap().len(), 22);
    assert_eq!(fixture["rows"].as_array().unwrap().len(), 79);
    fixture
}

fn assert_process(row: &Value) {
    let label = row["name"].as_str().unwrap();
    assert_ne!(row["comparison"], "go-only", "{label}");
    assert_eq!(row["stdin_reads"], 0, "{label}");
    assert_eq!(row["external_dials"], 0, "{label}");
    assert_eq!(row["account_calls"], 0, "{label}");
    assert_eq!(row["database"], false, "{label}");
    let home = tempfile::tempdir().unwrap();
    let data = home.path().join(".pixiv-cli");
    let config = data.join("config.toml");
    let temp = home.path().join("tmp");
    std::fs::create_dir(&temp).unwrap();
    if let Some(before) = row["before"].as_str() {
        std::fs::create_dir(&data).unwrap();
        std::fs::write(&config, before).unwrap();
    }
    let mut proxy = FiniteProxy::start();
    let mut command = Command::new(env!("CARGO_BIN_EXE_pixiv"));
    command
        .args(
            row["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|argument| argument.as_str().unwrap()),
        )
        .env_clear()
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("TMPDIR", &temp)
        .env("PATH", home.path())
        .env("HTTPS_PROXY", &proxy.address)
        .env("HTTP_PROXY", &proxy.address)
        .current_dir(home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in row["env"].as_object().unwrap() {
        command.env(key, value.as_str().unwrap());
    }
    let mut owned = OwnedChild(Some(command.spawn().unwrap()));
    let child = owned.0.as_mut().unwrap();
    let mut input = child.stdin.take().unwrap();
    if let Err(error) = input.write_all(row["input"].as_str().unwrap().as_bytes()) {
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe, "{label}: {error}");
    }
    // Open stdin proves the command does not wait for piped input or EOF.
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().unwrap().is_none() {
        assert!(
            Instant::now() < deadline,
            "{label} exceeded its process bound"
        );
        thread::sleep(Duration::from_millis(5));
    }
    drop(input);
    let output = owned.0.take().unwrap().wait_with_output().unwrap();
    let requests = proxy.finish();
    assert_eq!(
        requests.len(),
        row["requests"].as_array().unwrap().len(),
        "{label}: dictionary request count"
    );
    for (request, expected) in requests.iter().zip(row["requests"].as_array().unwrap()) {
        assert_eq!(expected["method"], "CONNECT", "{label}");
        assert_eq!(expected["uri"], "dic.pixiv.net:443", "{label}");
        assert_eq!(
            request.lines().next().unwrap(),
            "CONNECT dic.pixiv.net:443 HTTP/1.1",
            "{label}"
        );
        for line in request.lines() {
            let lower = line.to_ascii_lowercase();
            assert!(
                !lower.starts_with("authorization:")
                    && !lower.starts_with("proxy-authorization:")
                    && !lower.starts_with("cookie:"),
                "{label}: anonymous dictionary sent credentials"
            );
        }
    }
    let normalize_home = |bytes: &[u8]| {
        String::from_utf8(bytes.to_vec())
            .unwrap()
            .replace(home.path().to_str().unwrap(), "<HOME>")
    };
    assert_eq!(
        output.status.code(),
        Some(row["exit"].as_i64().unwrap() as i32),
        "{label}"
    );
    if row["comparison"] == "exact" {
        assert_eq!(
            normalize_home(&output.stdout),
            row["stdout"].as_str().unwrap(),
            "{label}"
        );
    } else {
        assert_eq!(row["comparison"], "startup-state", "{label}");
        assert!(!output.stdout.is_empty(), "{label} omitted help");
    }
    assert_eq!(
        normalize_home(&output.stderr),
        row["stderr"].as_str().unwrap(),
        "{label}"
    );
    assert_eq!(config.exists(), row["config"].as_bool().unwrap(), "{label}");
    for name in ["pixiv-cli.db", "pixiv-cli.db-wal", "pixiv-cli.db-shm"] {
        assert!(!data.join(name).exists(), "{label} created {name}");
    }
    if config.exists() {
        assert_eq!(
            std::fs::read_to_string(config).unwrap(),
            row["after"].as_str().unwrap(),
            "{label}"
        );
    }
}

#[test]
fn dictionary_startup_fixture_guards_the_frozen_go_root_and_anonymous_service() {
    let fixture = fixture();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for (path, digest) in fixture["sources"].as_object().unwrap() {
        let body = std::fs::read(root.join(path)).unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(body)),
            digest.as_str().unwrap(),
            "{path} differs from the frozen Go root/service source"
        );
    }
}

#[test]
fn dictionary_process_preserves_go_argument_configuration_and_anonymous_errors() {
    let fixture = fixture();
    let mut compared = 0;
    for row in fixture["rows"].as_array().unwrap() {
        if row["comparison"] == "exact" && row["transport"] == "" {
            assert_process(row);
            compared += 1;
        }
    }
    assert_eq!(compared, 46);
}

#[test]
fn dictionary_process_preserves_go_group_and_help_startup_state() {
    let fixture = fixture();
    let mut compared = 0;
    for row in fixture["rows"].as_array().unwrap() {
        if row["comparison"] == "startup-state" {
            assert_process(row);
            compared += 1;
        }
    }
    assert_eq!(compared, 7);
}

#[test]
fn dictionary_process_uses_environment_proxy_without_accounts_or_configured_pixiv_proxy() {
    let fixture = fixture();
    let mut compared = 0;
    for row in fixture["rows"].as_array().unwrap() {
        if row["comparison"] == "exact" && row["transport"] == "proxy-error" {
            assert_process(row);
            compared += 1;
        }
    }
    assert_eq!(compared, 9);
}
