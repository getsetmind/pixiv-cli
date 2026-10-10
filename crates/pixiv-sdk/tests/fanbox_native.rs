use pixiv_sdk::{
    context::{Context, ContextError, RequestContext},
    fanbox::{
        native::{CertificateFailureKind, CertificateVerificationError, NativeTransport},
        transport::{RawRequest, RawTransport},
    },
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

const CASES: [&str; 6] = [
    "headers_sequential_idle",
    "response_header_stall",
    "caller_deadline",
    "untrusted_certificate",
    "invalid_hostname",
    "expired_certificate",
];

#[test]
fn fanbox_native_six_go_contracts() {
    if std::env::var_os("PIXIV_RUST_NATIVE_CHILD").is_some() {
        return;
    }
    assert_eq!(std::env::consts::OS, "linux");
    assert_eq!(std::env::consts::ARCH, "x86_64");
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let directory = std::env::var_os("PIXIV_NATIVE_WITNESS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!("pixiv-rust-native-{}", std::process::id()))
        });
    fs::create_dir_all(&directory).unwrap();
    let go = std::env::var_os("PIXIV_NATIVE_GO").unwrap_or_else(|| "go".into());
    let version = Command::new(&go).arg("version").output().unwrap();
    assert!(version.status.success());
    assert!(
        String::from_utf8(version.stdout)
            .unwrap()
            .contains("go1.27.1 linux/amd64")
    );
    let helper = directory.join("owned-native-peer");
    let build = Command::new(&go)
        .current_dir(&root)
        .args(["build", "-o"])
        .arg(&helper)
        .arg("crates/pixiv-sdk/tests/helpers/fanbox_native_peer.go")
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    for name in CASES {
        let owned = directory.join(name);
        fs::create_dir_all(owned.join("empty-cert-dir")).unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "fanbox_native_child", "--nocapture"])
            .env("PIXIV_RUST_NATIVE_CHILD", name)
            .env("PIXIV_RUST_NATIVE_PEER", &helper)
            .env("PIXIV_RUST_NATIVE_DIR", &owned)
            .env("SSL_CERT_FILE", owned.join("root.pem"))
            .env("SSL_CERT_DIR", owned.join("empty-cert-dir"))
            .spawn()
            .unwrap();
        let until = Instant::now() + Duration::from_secs(46);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(
                    status.success(),
                    "native contract failed: {name}; witness {}",
                    owned.display()
                );
                break;
            }
            if Instant::now() >= until {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("bounded native contract exceeded 46 seconds: {name}");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    eprintln!("native Rust lossless witnesses: {}", directory.display());
}

struct Peer {
    child: Child,
    events: mpsc::Receiver<Value>,
    pending: Vec<Value>,
}
impl Peer {
    fn start(name: &str, directory: &Path) -> Self {
        let mut child = Command::new(std::env::var_os("PIXIV_RUST_NATIVE_PEER").unwrap())
            .arg(name)
            .arg(directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (send, events) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let value = serde_json::from_str(&line.unwrap()).unwrap();
                if send.send(value).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            events,
            pending: vec![],
        }
    }
    fn wait(&mut self, name: &str, duration: Duration) -> Value {
        if let Some(index) = self.pending.iter().position(|value| value["event"] == name) {
            return self.pending.remove(index);
        }
        let until = Instant::now() + duration;
        loop {
            let event = self
                .events
                .recv_timeout(until.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|error| panic!("native peer event {name}: {error}"));
            if event["event"] == name {
                return event;
            }
            self.pending.push(event);
        }
    }
    fn remains_open(&mut self, duration: Duration) {
        assert!(!self.pending.iter().any(|value| value["event"] == "closed"));
        let until = Instant::now() + duration;
        loop {
            match self
                .events
                .recv_timeout(until.saturating_duration_since(Instant::now()))
            {
                Ok(event) => {
                    assert_ne!(
                        event["event"], "closed",
                        "socket closed before explicit idle cleanup"
                    );
                    self.pending.push(event);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => return,
                Err(error) => panic!("owned peer exited: {error}"),
            }
        }
    }
    fn finish(mut self) -> Value {
        writeln!(self.child.stdin.as_mut().unwrap(), "stop").unwrap();
        let report = self.wait("report", Duration::from_secs(3));
        assert!(self.child.wait().unwrap().success());
        report
    }
}
impl Drop for Peer {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn raw_get(endpoint: &str, context: Arc<dyn RequestContext>) -> RawRequest {
    RawRequest {
        method: "GET".into(),
        url: format!("https://{endpoint}/owned-native"),
        logical_host: None,
        headers: BTreeMap::new(),
        body: None,
        content_length: 0,
        context,
    }
}

fn cause<'a, T: std::error::Error + 'static>(
    mut error: &'a (dyn std::error::Error + 'static),
) -> Option<&'a T> {
    loop {
        if let Some(cause) = error.downcast_ref::<T>() {
            return Some(cause);
        }
        error = error.source()?;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanbox_native_child() {
    let Ok(name) = std::env::var("PIXIV_RUST_NATIVE_CHILD") else {
        eprintln!(
            "owned child-entry scaffold only; this no-mode return is excluded from meaningful native assertions"
        );
        return;
    };
    assert!(CASES.contains(&name.as_str()));
    let owned = PathBuf::from(std::env::var_os("PIXIV_RUST_NATIVE_DIR").unwrap());
    assert_eq!(
        PathBuf::from(std::env::var_os("SSL_CERT_FILE").unwrap()),
        owned.join("root.pem")
    );
    assert_eq!(
        PathBuf::from(std::env::var_os("SSL_CERT_DIR").unwrap()),
        owned.join("empty-cert-dir")
    );
    let mut peer = Peer::start(&name, &owned);
    let ready = peer.wait("ready", Duration::from_secs(3));
    let endpoint = ready["endpoint"].as_str().unwrap();
    assert!(endpoint.starts_with("localhost:"));
    let native = Arc::new(NativeTransport::new("").unwrap());
    let mut responses = vec![];
    let mut errors = vec![];
    let mut native_failure = Value::Null;
    let mut held_ns = 0_u128;
    let mut peer_observed_ns = 0_u128;
    let mut completed_liveness_checks = 0;
    let start = Instant::now();
    if name == "headers_sequential_idle" {
        let fixture: Value =
            serde_json::from_str(include_str!("fixtures/fanbox-native-profile.json")).unwrap();
        let witness = &fixture["cases"][0]["comparable"]["connections"];
        for (connection, frame) in [(0, 2), (0, 3), (1, 2)] {
            let fields = witness[connection]["frames"][frame]["fields"]
                .as_array()
                .unwrap();
            let field = |name: &str| {
                fields.iter().find(|field| field["Name"] == name).unwrap()["Value"]
                    .as_str()
                    .unwrap()
            };
            let mut request = raw_get(endpoint, Arc::new(Context::background()));
            request.url = format!("https://{endpoint}{}", field(":path"));
            request.method = field(":method").into();
            request.logical_host = Some(field(":authority").into());
            for header in fields
                .iter()
                .filter(|field| !field["Name"].as_str().unwrap().starts_with(':'))
            {
                request
                    .headers
                    .entry(header["Name"].as_str().unwrap().into())
                    .or_default()
                    .push(header["Value"].as_str().unwrap().into());
            }
            let mut response = native.send(request).await.unwrap().unwrap();
            assert_eq!(response.status, 200);
            let mut body = response.body.take().unwrap();
            let mut data = vec![];
            loop {
                let mut chunk = [0_u8; 128];
                let read = body.read(&mut chunk).await;
                assert!(read.error.is_none());
                data.extend_from_slice(&chunk[..read.count]);
                if read.eof {
                    break;
                }
                assert!(read.count > 0);
            }
            body.close().await.unwrap();
            responses.push(serde_json::from_slice::<Value>(&data).unwrap());
            peer.wait("request", Duration::from_secs(2));
            if frame == 3 || connection == 1 {
                peer.remains_open(Duration::from_millis(50));
                completed_liveness_checks += 1;
                native.close_idle_connections().await.unwrap();
                peer.wait("closed", Duration::from_secs(2));
            }
        }
    } else {
        let context = if name == "caller_deadline" {
            Context::with_deadline(Instant::now() + Duration::from_millis(300))
        } else {
            Context::background().child()
        };
        let future = native.send(raw_get(endpoint, Arc::new(context.clone())));
        tokio::pin!(future);
        if name == "response_header_stall" || name == "caller_deadline" {
            let wait_request = tokio::task::spawn_blocking(move || {
                peer.wait("request", Duration::from_secs(3));
                peer
            });
            peer = tokio::select! {
                value = wait_request => value.unwrap(),
                value = &mut future => panic!("stalled GET ended before peer observation: {}", value.err().map_or_else(|| "success".into(), |error| error.to_string())),
            };
            let observed = Instant::now();
            peer_observed_ns = start.elapsed().as_nanos();
            native.close_idle_connections().await.unwrap();
            peer.remains_open(Duration::from_millis(50));
            if name == "response_header_stall" {
                tokio::select! {
                    value = &mut future => panic!("GET ended before 31.2-second boundary: {}", value.err().map_or_else(|| "success".into(), |error| error.to_string())),
                    () = tokio::time::sleep(Duration::from_millis(31200).saturating_sub(observed.elapsed())) => {},
                }
                held_ns = observed.elapsed().as_nanos();
                assert!(held_ns >= 31_200_000_000);
                context.cancel();
            }
        }
        let result = tokio::time::timeout(Duration::from_secs(3), &mut future)
            .await
            .unwrap();
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("negative native case succeeded"),
        };
        let mut current: Option<&(dyn std::error::Error + 'static)> = Some(error.as_ref());
        while let Some(cause) = current {
            let known_type = if cause.is::<ContextError>() {
                Some("pixiv_sdk::context::ContextError")
            } else if cause.is::<CertificateVerificationError>() {
                Some("wreq::tls::CertificateVerificationError")
            } else if cause.is::<wreq::Error>() {
                Some("wreq::Error")
            } else if cause.is::<btls::ssl::Error>() {
                Some("btls::ssl::Error")
            } else if cause.is::<btls::error::ErrorStack>() {
                Some("btls::error::ErrorStack")
            } else {
                None
            };
            errors.push(json!({"recognized_rust_type": known_type, "message": cause.to_string()}));
            current = cause.source();
        }
        match name.as_str() {
            "response_header_stall" => assert_eq!(
                cause::<ContextError>(error.as_ref()),
                Some(&ContextError::Canceled)
            ),
            "caller_deadline" => assert_eq!(
                cause::<ContextError>(error.as_ref()),
                Some(&ContextError::DeadlineExceeded)
            ),
            "untrusted_certificate" => assert_eq!(
                cause::<CertificateVerificationError>(error.as_ref())
                    .unwrap()
                    .kind(),
                CertificateFailureKind::UnknownAuthority
            ),
            "invalid_hostname" => assert_eq!(
                cause::<CertificateVerificationError>(error.as_ref())
                    .unwrap()
                    .kind(),
                CertificateFailureKind::HostnameMismatch
            ),
            "expired_certificate" => assert_eq!(
                cause::<CertificateVerificationError>(error.as_ref())
                    .unwrap()
                    .kind(),
                CertificateFailureKind::Expired
            ),
            _ => unreachable!(),
        }
        if let Some(failure) = cause::<CertificateVerificationError>(error.as_ref()) {
            assert_eq!(
                failure.certificate_der().unwrap(),
                decode_hex(ready["certificate_der_hex"].as_str().unwrap())
            );
            assert_ne!(
                failure.verification_result(),
                btls::x509::X509VerifyError::CERT_REJECTED
            );
            assert!(failure.internal_verification_error().is_none());
            native_failure = json!({
                "kind": format!("{:?}", failure.kind()),
                "original_native_verify_code": failure.verification_result().as_raw(),
                "original_native_verify_message": failure.verification_result().to_string(),
                "certificate_der_hex": failure.certificate_der().map(|der| der.iter().map(|byte| format!("{byte:02x}")).collect::<String>()),
            });
        }
        if name == "response_header_stall" || name == "caller_deadline" {
            assert_eq!(peer.wait("reset", Duration::from_secs(2))["code"], 8);
        }
        native.close_idle_connections().await.unwrap();
        peer.wait("closed", Duration::from_secs(2));
    }
    let report = peer.finish();
    fs::write(owned.join("rust-observation.json"), serde_json::to_vec_pretty(&json!({
        "name": name, "boundary_mapping": "Exact Go-observed Session request inputs are sent through the genuine public Rust native raw boundary. This establishes native transport wire/lifecycle evidence, not private Session or public SDK policy equivalence.", "peer": report, "ready": ready, "responses": responses,
        "errors": errors, "native_certificate_failure": native_failure,
        "elapsed_ns": start.elapsed().as_nanos(), "peer_request_observed_elapsed_ns": peer_observed_ns,
        "peer_header_hold_ns": held_ns, "completed_idle_liveness_checks": completed_liveness_checks,
        "error_mapping": "Rust typed native verification errors preserve actual X509 class and native source. Go concrete type names and diagnostic wording are retained unchanged in the sealed fixture and are not claimed Rust-compatible."
    })).unwrap()).unwrap();
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/fanbox-native-profile.json")).unwrap();
    let expected = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == name)
        .unwrap();
    let connections = report["connections"].as_array().unwrap();
    let expected_connections = expected["comparable"]["connections"].as_array().unwrap();
    assert_eq!(connections.len(), expected_connections.len());
    for (actual, expected) in connections.iter().zip(expected_connections) {
        assert_eq!(
            project_connection(actual.clone(), endpoint),
            *expected,
            "{name} actual native wire/lifecycle differs from sealed Go witness"
        );
    }
    assert_eq!(json!(responses), expected["comparable"]["response_json"]);
}

fn decode_hex(value: &str) -> Vec<u8> {
    assert_eq!(value.len() % 2, 0);
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn project_connection(mut connection: Value, endpoint: &str) -> Value {
    let hello = connection["hello"].as_object_mut().unwrap();
    let raw = decode_hex(hello["record_hex"].as_str().unwrap());
    assert_eq!(
        format!("{:x}", Sha256::digest(&raw)),
        hello["record_sha256"]
    );
    assert_eq!(raw[0], 22);
    assert_eq!(raw[5], 1);
    assert_eq!(
        usize::from(u16::from_be_bytes([raw[3], raw[4]])),
        raw.len() - 5
    );
    hello.remove("record_hex");
    hello.remove("record_sha256");
    for name in ["random_hex", "session_id_hex"] {
        hello[name] = json!(format!(
            "random[{}]",
            decode_hex(hello[name].as_str().unwrap()).len()
        ));
    }
    for cipher in hello["cipher_suites"].as_array_mut().unwrap() {
        let value = cipher.as_u64().unwrap();
        if value & 0x0f0f == 0x0a0a && value >> 8 == value & 255 {
            *cipher = json!(2570);
        }
    }
    for extension in hello["extensions"].as_array_mut().unwrap() {
        extension["id"] = extension["comparable_id"].clone();
        extension["data_hex"] = extension["comparable_data"].clone();
        extension.as_object_mut().unwrap().remove("comparable_id");
        extension.as_object_mut().unwrap().remove("comparable_data");
        if extension["id"] == 65037 {
            extension["length"] = json!("variable:186|218|250|282");
        }
    }
    let mut ack = 0;
    // Go's comparable frame slice stays nil until a non-ACK frame is retained.
    let mut projected: Option<Vec<Value>> = None;
    for mut frame in connection["frames"].as_array().unwrap().clone() {
        let raw = decode_hex(frame["raw_hex"].as_str().unwrap());
        assert!(raw.len() >= 9);
        let length = (usize::from(raw[0]) << 16) | (usize::from(raw[1]) << 8) | usize::from(raw[2]);
        assert_eq!(raw.len(), length + 9);
        assert_eq!(json!(raw[4]), frame["flags"]);
        assert_eq!(
            json!(u32::from_be_bytes([raw[5], raw[6], raw[7], raw[8]]) & 0x7fff_ffff),
            frame["stream"]
        );
        frame.as_object_mut().unwrap().remove("raw_hex");
        if frame["type"] == "SETTINGS" && frame["flags"] == 1 {
            ack += 1;
            continue;
        }
        if let Some(fields) = frame.get_mut("fields").and_then(Value::as_array_mut) {
            for field in fields {
                field["Value"] = json!(
                    field["Value"]
                        .as_str()
                        .unwrap()
                        .replace(endpoint, "localhost:<owned-port>")
                );
            }
        }
        projected.get_or_insert_with(Vec::new).push(frame);
    }
    connection["frames"] = json!(projected);
    connection["settings_ack_count"] = json!(ack);
    connection
}
