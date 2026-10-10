use base64::{Engine, engine::general_purpose::STANDARD};
use flate2::{Compression, write::GzEncoder};
use pixiv_app::{
    reverse_search::{
        ReverseFuture,
        http::{HttpRequest, HttpTransport},
    },
    update::{
        ExternalError, Release, ReleaseAsset, ReleaseInstaller,
        http::ClientTransport,
        installer::{
            BinaryChecker, ExecutableLocator, ProcessExitError, ProcessReleaseBinaryChecker,
            ReleaseInstallerOptions, SignedReleaseInstaller,
        },
    },
};
use pixiv_sdk::{
    context::{Context, ContextError},
    fanbox::transport::{BodyFuture, Headers, RawBody, RawRead, RawResponse},
};
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    error::Error,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn hash(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}
fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../pixiv-cli/tests/fixtures/updater-owned-preflight.json"
    ))
    .unwrap()
}
fn assert_summary(bytes: &[u8], want: &Value, label: &str) {
    assert_eq!(
        bytes.len(),
        want["length"].as_u64().unwrap() as usize,
        "{label} length"
    );
    assert_eq!(
        hash(bytes),
        want["sha256"].as_str().unwrap(),
        "{label} SHA-256"
    );
    if let Some(text) = want["text"].as_str() {
        assert_eq!(String::from_utf8_lossy(bytes), text, "{label} text");
    }
    if let Some(prefix) = want["prefix"].as_str() {
        assert!(bytes.starts_with(prefix.as_bytes()), "{label} prefix");
    }
    if let Some(suffix) = want["suffix"].as_str() {
        assert!(bytes.ends_with(suffix.as_bytes()), "{label} suffix");
    }
}
fn write(path: &Path, bytes: &[u8], mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}
fn build_owned_helper(fixture: &Value) -> Vec<u8> {
    let toolchain = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .join("go-toolchain/go");
    let compiler = toolchain.join("bin/go");
    let helper = &fixture["helper"];
    assert_summary(
        &fs::read(&compiler).unwrap(),
        &json!({"length":fs::metadata(&compiler).unwrap().len(),"sha256":helper["compiler_sha256"]}),
        "official compiler",
    );
    assert_eq!(
        hash(&fs::read(toolchain.join("src/os/exec/exec.go")).unwrap()),
        helper["exec_stdlib_sha256"]
    );
    let cache = tempfile::tempdir().unwrap();
    let build = || {
        let directory = tempfile::tempdir().unwrap();
        write(
            &directory.path().join("main.go"),
            helper["source"].as_str().unwrap().as_bytes(),
            0o600,
        );
        write(
            &directory.path().join("go.mod"),
            helper["module"].as_str().unwrap().as_bytes(),
            0o600,
        );
        assert_eq!(
            hash(&fs::read(directory.path().join("main.go")).unwrap()),
            helper["source_sha256"]
        );
        assert_eq!(
            hash(&fs::read(directory.path().join("go.mod")).unwrap()),
            helper["module_sha256"]
        );
        let mut command = std::process::Command::new(&compiler);
        command
            .current_dir(directory.path())
            .args([
                "build",
                "-trimpath",
                "-buildvcs=false",
                "-ldflags=-buildid=",
                "-o",
            ])
            .arg(directory.path().join("helper"))
            .arg(".");
        for variable in helper["build_environment"].as_array().unwrap() {
            let (key, value) = variable.as_str().unwrap().split_once('=').unwrap();
            command.env(key, value);
        }
        command.env("GOCACHE", cache.path());
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "compile owned helper: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        fs::read(directory.path().join("helper")).unwrap()
    };
    let first = build();
    let second = build();
    assert_eq!(first, second, "independently compiled owned helper");
    assert_eq!(
        first.len(),
        helper["binary_length"].as_u64().unwrap() as usize
    );
    assert_eq!(hash(&first), helper["binary_sha256"]);
    first
}
struct Body {
    bytes: Vec<u8>,
    offset: usize,
}
impl RawBody for Body {
    fn read<'a>(&'a mut self, buffer: &'a mut [u8]) -> BodyFuture<'a, RawRead> {
        Box::pin(async move {
            let count = buffer.len().min(self.bytes.len() - self.offset);
            buffer[..count].copy_from_slice(&self.bytes[self.offset..self.offset + count]);
            self.offset += count;
            RawRead {
                count,
                eof: count == 0,
                error: None,
            }
        })
    }
    fn close(&mut self) -> BodyFuture<'_, Result<(), ExternalError>> {
        Box::pin(async { Ok(()) })
    }
}
struct Transport {
    assets: BTreeMap<String, Vec<u8>>,
    requests: Arc<Mutex<Vec<String>>>,
}
impl HttpTransport for Transport {
    fn send(
        &self,
        request: HttpRequest,
    ) -> ReverseFuture<'_, Result<Option<RawResponse>, ExternalError>> {
        Box::pin(async move {
            assert_eq!(request.method, "GET");
            assert!(request.headers.is_empty());
            let name = request.url.rsplit('/').next().unwrap();
            assert_eq!(
                request.url,
                format!("https://github.com/FlanChanXwO/pixiv-cli/releases/download/v1.2.3/{name}")
            );
            self.requests
                .lock()
                .unwrap()
                .push(format!("GET {} user-agent=\"\"", request.url));
            let bytes = self.assets.get(name).expect("owned asset only").clone();
            Ok(Some(RawResponse {
                status: 200,
                headers: Headers::new(),
                content_length: bytes.len() as i64,
                body: Some(Box::new(Body { bytes, offset: 0 })),
            }))
        })
    }
}
struct Locator {
    path: PathBuf,
    requests: Arc<Mutex<Vec<String>>>,
}
impl ExecutableLocator for Locator {
    fn executable(&self) -> Result<PathBuf, ExternalError> {
        self.requests
            .lock()
            .unwrap()
            .push("locate-owned-target".into());
        Ok(self.path.clone())
    }
}
fn archive(binary: &[u8], barrier: &str) -> Vec<u8> {
    if barrier == "corrupt_archive" {
        return b"owned non-gzip archive".to_vec();
    }
    let gzip = GzEncoder::new(Vec::new(), Compression::default());
    let mut archive = tar::Builder::new(gzip);
    let mut append = |name: &str, kind: tar::EntryType, body: &[u8]| {
        let mut header = tar::Header::new_ustar();
        header.as_mut_bytes()[..100].fill(0);
        header.as_mut_bytes()[..name.len()].copy_from_slice(name.as_bytes());
        header.set_mode(0o755);
        header.set_entry_type(kind);
        header.set_size(if kind.is_file() { body.len() as u64 } else { 0 });
        if kind.is_symlink() {
            header.set_link_name("owned-target").unwrap();
        }
        header.set_cksum();
        archive.append(&header, body).unwrap();
    };
    if barrier == "archive_traversal" {
        append(
            "../owned-escape",
            tar::EntryType::Regular,
            b"owned unsafe member",
        );
    }
    if barrier == "archive_symlink" {
        append("owned-link", tar::EntryType::Symlink, b"");
    }
    if barrier != "archive_missing_binary" {
        append("nested/pixiv", tar::EntryType::Regular, binary);
    }
    if barrier == "archive_duplicate_binary" {
        append("duplicate/pixiv", tar::EntryType::Regular, binary);
    }
    archive.into_inner().unwrap().finish().unwrap()
}
fn exit_error<'a>(error: &'a (dyn Error + 'static)) -> Option<&'a ProcessExitError> {
    if let Some(error) = error.downcast_ref::<ProcessExitError>() {
        return Some(error);
    }
    error.source().and_then(exit_error)
}
fn context_error(error: &(dyn Error + 'static)) -> Option<ContextError> {
    if let Some(error) = error.downcast_ref::<ContextError>() {
        return Some(*error);
    }
    error.source().and_then(context_error)
}
fn normalize(message: String, root: &Path) -> String {
    let mut text = message.replace(&format!("{}/", root.display()), "OWNED_ROOT/");
    let prefix = ".pixiv-update-";
    let mut at = 0;
    while let Some(start) = text[at..].find(prefix) {
        let from = at + start + prefix.len();
        let to = from
            + text[from..]
                .bytes()
                .take_while(|b| !matches!(b, b'/' | b'"' | b' '))
                .count();
        text.replace_range(from..to, "OWNED_TEMP");
        at = from + 10;
    }
    text
}
pub async fn replay() {
    assert_eq!(
        std::env::consts::OS,
        "linux",
        "owned fixture requires native Linux"
    );
    assert_eq!(
        std::env::consts::ARCH,
        "x86_64",
        "owned fixture requires native amd64"
    );
    let fixture = fixture();
    assert_eq!(fixture["rows"].as_array().unwrap().len(), 41);
    let binary = tokio::task::spawn_blocking({
        let fixture = fixture.clone();
        move || build_owned_helper(&fixture)
    })
    .await
    .unwrap();
    let seed = Sha256::digest(
        b"visible owned preflight synthetic signing fixture; never production trust",
    );
    let key = Ed25519KeyPair::from_seed_unchecked(&seed).unwrap();
    assert_eq!(
        hex(key.public_key().as_ref()),
        fixture["synthetic_public_key_hex"]
    );
    for row in fixture["rows"].as_array().unwrap() {
        let input = &row["input"];
        let label = format!(
            "{}/{}",
            input["boundary"].as_str().unwrap(),
            input["name"].as_str().unwrap()
        );
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        write(&target, b"owned previous target bytes\n", 0o644);
        write(
            &root.path().join("owned-control.json"),
            &serde_json::to_vec(&input["control"]).unwrap(),
            0o600,
        );
        let prepared = if input["boundary"] == "public_default_installer" {
            let barrier = input["barrier"].as_str().unwrap();
            let archive = archive(&binary, barrier);
            let checksum = match barrier {
                "archive_checksum_mismatch" => "0".repeat(64),
                "signed_invalid_checksum_entry" => "invalid-owned-checksum".into(),
                _ => hash(&archive),
            };
            let checksums =
                format!("{checksum}  pixiv-cli_1.2.3_linux_amd64.tar.gz\n").into_bytes();
            let mut signature = key.sign(&checksums).as_ref().to_vec();
            if barrier == "invalid_signature" {
                signature[0] ^= 1;
            }
            let manifest=serde_json::to_vec(&json!({"key_id":"owned-helper-only","checksums_sha256":hash(&checksums),"signature":STANDARD.encode(signature)})).unwrap();
            let names = [
                "pixiv-cli_1.2.3_linux_amd64.tar.gz",
                "checksums.txt",
                "checksums.json",
            ];
            let assets = BTreeMap::from([
                (names[0].into(), archive),
                (names[1].into(), checksums),
                (names[2].into(), manifest),
            ]);
            Some((names, assets))
        } else {
            None
        };
        let setting = input["context"].as_str().unwrap();
        let context = if setting == "one_second_deadline" {
            Context::with_deadline(Instant::now() + Duration::from_secs(1))
        } else {
            Context::new()
        };
        if setting == "pre_canceled" {
            context.cancel();
        }
        let watcher = if setting == "cancel_when_heartbeat_advances" {
            let context = context.clone();
            let path = root.path().join("owned-heartbeat");
            Some(tokio::spawn(async move {
                let start = Instant::now();
                let mut first = None;
                loop {
                    if let Ok(bytes) = fs::read(&path) {
                        if let Some(first) = &first {
                            if first != &bytes {
                                context.cancel();
                                return true;
                            }
                        } else {
                            first = Some(bytes);
                        }
                    }
                    assert!(
                        start.elapsed() < Duration::from_secs(5),
                        "owned helper never advanced heartbeat"
                    );
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            }))
        } else {
            None
        };
        let requests = Arc::new(Mutex::new(Vec::new()));
        let start = Instant::now();
        let result = if input["boundary"] == "concrete_checker" {
            let dir = root.path().join("direct");
            fs::create_dir(&dir).unwrap();
            write(&dir.join("pixiv"), &binary, 0o755);
            ProcessReleaseBinaryChecker
                .check(
                    Arc::new(context.clone()),
                    dir.join("pixiv"),
                    "v1.2.3".into(),
                )
                .await
        } else {
            let (names, assets) = prepared.expect("owned installer assets prepared");
            let mut install_target = target.clone();
            if input["symlink"].as_bool().unwrap() {
                install_target = root.path().join("owned-link");
                std::os::unix::fs::symlink("target", &install_target).unwrap();
            }
            let installer = SignedReleaseInstaller::new(ReleaseInstallerOptions {
                http_transport: Some(Arc::new(ClientTransport::new(Arc::new(Transport {
                    assets,
                    requests: requests.clone(),
                })))),
                trusted_keys: BTreeMap::from([(
                    "owned-helper-only".into(),
                    key.public_key().as_ref().to_vec(),
                )]),
                executable_path: Some(Arc::new(Locator {
                    path: install_target,
                    requests: requests.clone(),
                })),
                goos: "linux".into(),
                goarch: "amd64".into(),
                ..Default::default()
            });
            installer.install(Arc::new(context.clone()),Release{tag_name:"v1.2.3".into(),version:"1.2.3".into(),prerelease:false,assets:names.map(|name|ReleaseAsset{name:name.into(),download_url:format!("https://github.com/FlanChanXwO/pixiv-cli/releases/download/v1.2.3/{name}")}).to_vec()}).await
        };
        assert!(
            start.elapsed() < Duration::from_secs(6),
            "{label} bounded return"
        );
        if let Some(watcher) = watcher {
            assert!(watcher.await.unwrap(), "{label} heartbeat advanced");
        }
        let message = result
            .as_ref()
            .err()
            .map(|e| normalize(e.to_string(), root.path()))
            .unwrap_or_default();
        assert_summary(
            message.as_bytes(),
            &row["error"]["message"],
            &format!("{label} error"),
        );
        let cause = result
            .as_ref()
            .err()
            .and_then(|e| context_error(e.as_ref()));
        assert_eq!(
            cause == Some(ContextError::Canceled),
            row["error"]["is_canceled"].as_bool().unwrap(),
            "{label} canceled cause"
        );
        assert_eq!(
            cause == Some(ContextError::DeadlineExceeded),
            row["error"]["is_deadline"].as_bool().unwrap(),
            "{label} deadline cause"
        );
        let exit = result.as_ref().err().and_then(|e| exit_error(e.as_ref()));
        assert_eq!(
            exit.is_some(),
            row["error"]["exit_error"].as_bool().unwrap(),
            "{label} exit error"
        );
        assert_eq!(
            exit.map_or(0, ProcessExitError::exit_code),
            row["error"]["exit_code"].as_i64().unwrap() as i32,
            "{label} exit code"
        );
        assert_eq!(
            exit.map(ProcessExitError::process_state)
                .unwrap_or_default(),
            row["error"]["process_state"].as_str().unwrap(),
            "{label} process state"
        );
        assert_summary(
            exit.map_or(&[], |e| e.stderr.as_slice()),
            &row["error"]["stderr"],
            &format!("{label} stderr"),
        );
        assert_eq!(
            context.error().map(|e| e.to_string()).unwrap_or_default(),
            row["context_error"].as_str().unwrap(),
            "{label} context"
        );
        assert_eq!(
            *requests.lock().unwrap(),
            row["requests"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect::<Vec<_>>(),
            "{label} requests"
        );
        let effects = fs::read_to_string(root.path().join("owned-effects.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            effects,
            row["effects"].as_array().unwrap().clone(),
            "{label} effects"
        );
        if input["control"]["mode"] == "block" {
            let heartbeat = fs::read(root.path().join("owned-heartbeat")).unwrap();
            let log = fs::read(root.path().join("owned-effects.jsonl")).unwrap();
            tokio::time::sleep(Duration::from_millis(60)).await;
            assert_eq!(
                heartbeat,
                fs::read(root.path().join("owned-heartbeat")).unwrap(),
                "{label} child quiet"
            );
            assert_eq!(
                log,
                fs::read(root.path().join("owned-effects.jsonl")).unwrap(),
                "{label} effects quiet"
            );
        }
        assert_summary(
            &fs::read(&target).unwrap(),
            &row["target"],
            &format!("{label} target"),
        );
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            row["target_mode"].as_u64().unwrap() as u32,
            "{label} target mode"
        );
        assert_eq!(
            fs::read_link(root.path().join("owned-link"))
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
            row["symlink_target"].as_str().unwrap(),
            "{label} symlink"
        );
        assert!(
            !root.path().join("owned-escape").exists(),
            "{label} traversal"
        );
        assert!(
            fs::read_dir(root.path()).unwrap().all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".pixiv-update-")),
            "{label} transient cleanup"
        );
    }
}
