#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

#[path = "support/updater_assembly_process.rs"]
mod owned_process;

use owned_process::OwnedCommandHelper;
use pixiv_app::{
    reverse_search::http::{HttpRequest, HttpTransport},
    update::{
        BuildInfo, CallerContext, Command, CommandRunner, ExternalError, UpdateFuture,
        assembly::{
            ProcessCommandRunner, ProductionUpdateOptions, SharedUpdateWriter,
            new_automatic_checker_with_options, new_coordinator_with_options, shared_writer,
        },
        coordinator::{AutomaticRequest, UpdateRequest},
    },
};
use pixiv_sdk::{
    context::{Context, ContextError},
    fanbox::transport::{BodyFuture, Headers, RawBody, RawRead, RawResponse},
};
use std::{
    error::Error,
    fs,
    io::{self, Write},
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Clone, Default)]
struct Buffer(Arc<Mutex<Vec<u8>>>);
impl Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn writer() -> (SharedUpdateWriter, Buffer) {
    let buffer = Buffer::default();
    (shared_writer(buffer.clone()), buffer)
}
fn background() -> CallerContext {
    Arc::new(Context::background())
}
fn context_cause(error: &(dyn Error + 'static)) -> Option<ContextError> {
    error
        .downcast_ref::<ContextError>()
        .copied()
        .or_else(|| error.source().and_then(context_cause))
}
struct Body(Vec<u8>);
impl RawBody for Body {
    fn read<'a>(&'a mut self, buffer: &'a mut [u8]) -> BodyFuture<'a, RawRead> {
        Box::pin(async move {
            let count = buffer.len().min(self.0.len());
            buffer[..count].copy_from_slice(&self.0[..count]);
            self.0.drain(..count);
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
#[derive(Default)]
struct Transport {
    requests: Mutex<Vec<String>>,
}
impl HttpTransport for Transport {
    fn send(
        &self,
        request: HttpRequest,
    ) -> UpdateFuture<'_, Result<Option<RawResponse>, ExternalError>> {
        self.requests.lock().unwrap().push(request.url.clone());
        assert_eq!(request.method, "GET");
        assert_eq!(
            request.headers.get("User-Agent"),
            Some(&vec!["pixiv-cli".into()])
        );
        let status = if request.url.starts_with("https://api.github.com/") {
            200
        } else {
            503
        };
        Box::pin(async move {
            Ok(Some(RawResponse {
                status,
                headers: Headers::new(),
                content_length: 2,
                body: Some(Box::new(Body(b"[]".to_vec()))),
            }))
        })
    }
}
fn options(
    proxy: &str,
    directory: &std::path::Path,
    transport: Arc<Transport>,
) -> ProductionUpdateOptions {
    ProductionUpdateOptions {
        proxy: proxy.into(),
        cache_directory: Some(directory.join("cache")),
        http_transport: Some(transport),
    }
}

#[tokio::test]
async fn production_construction_and_development_checks_are_lazy_and_account_free() {
    let directory = tempfile::tempdir().unwrap();
    let transport = Arc::new(Transport::default());
    let (out, _) = writer();
    let (err, _) = writer();
    let coordinator =
        new_coordinator_with_options(options("", directory.path(), transport.clone()), out, err)
            .unwrap();
    assert!(!directory.path().join("cache").exists());
    let error = coordinator
        .execute(
            background(),
            UpdateRequest {
                build_info: BuildInfo {
                    version: "dev".into(),
                },
                check: true,
                include_prerelease: false,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "development builds cannot update themselves"
    );
    let automatic =
        new_automatic_checker_with_options(options("", directory.path(), transport.clone()))
            .unwrap();
    assert!(
        automatic
            .check(
                background(),
                AutomaticRequest {
                    build_info: BuildInfo {
                        version: "dev".into()
                    }
                }
            )
            .await
            .unwrap()
            .is_none()
    );
    assert!(transport.requests.lock().unwrap().is_empty());
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn invalid_proxy_errors_are_safe_and_precede_cache_construction() {
    for (proxy, expected) in [
        (
            "http://owned-secret:owned-password@owned-host.invalid/%zz",
            "parse update proxy URL: parse proxy URL: invalid proxy configuration",
        ),
        (
            "ftp://owned-host.invalid",
            "parse update proxy URL: proxy URL must use http, https, socks5, or socks5h: invalid proxy configuration",
        ),
        (
            "http:/owned-host.invalid",
            "parse update proxy URL: proxy URL must use http, https, socks5, or socks5h: invalid proxy configuration",
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let transport = Arc::new(Transport::default());
        let error = match new_automatic_checker_with_options(options(
            proxy,
            directory.path(),
            transport.clone(),
        )) {
            Ok(_) => panic!("invalid proxy was accepted"),
            Err(error) => error,
        };
        assert_eq!(error.to_string(), expected);
        assert!(transport.requests.lock().unwrap().is_empty());
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}

#[tokio::test]
async fn check_uses_the_file_cache_and_enables_embedded_sources_only_for_empty_proxy() {
    for proxy in [
        "",
        "http://proxy.invalid:8080",
        "https://proxy.invalid:8443",
        "socks5://proxy.invalid:1080",
        "socks5h://proxy.invalid:1080",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let transport = Arc::new(Transport::default());
        let (out, out_buffer) = writer();
        let (err, err_buffer) = writer();
        let coordinator = new_coordinator_with_options(
            options(proxy, directory.path(), transport.clone()),
            out,
            err,
        )
        .unwrap();
        let result = coordinator
            .execute(
                background(),
                UpdateRequest {
                    build_info: BuildInfo {
                        version: "v1.2.3".into(),
                    },
                    check: true,
                    include_prerelease: false,
                },
            )
            .await
            .unwrap();
        assert_eq!(result.source, "release");
        assert_eq!(result.latest_version, None);
        let requests = transport.requests.lock().unwrap();
        assert_eq!(
            requests
                .iter()
                .any(|url| url.starts_with("https://gh-proxy.com/")),
            proxy.is_empty()
        );
        if !proxy.is_empty() {
            assert_eq!(requests.len(), 1);
        }
        assert!(
            requests
                .iter()
                .any(|url| url == "https://api.github.com/repos/FlanChanXwO/pixiv-cli/releases")
        );
        let cached = fs::read(directory.path().join("cache/github-releases.json")).unwrap();
        assert!(cached.ends_with(b"\n"));
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&cached).unwrap()["schema_version"],
            2
        );
        assert!(out_buffer.0.lock().unwrap().is_empty());
        assert!(err_buffer.0.lock().unwrap().is_empty());
        assert!(!directory.path().join("accounts.db").exists());
    }
}

struct TrustBarrierTransport {
    requests: Mutex<Vec<String>>,
    release: Vec<u8>,
}
impl HttpTransport for TrustBarrierTransport {
    fn send(
        &self,
        request: HttpRequest,
    ) -> UpdateFuture<'_, Result<Option<RawResponse>, ExternalError>> {
        self.requests.lock().unwrap().push(request.url.clone());
        let bytes = if request.url.starts_with("https://api.github.com/") {
            self.release.clone()
        } else if request.url.ends_with("/checksums.txt") {
            b"owned checksums whose digest is never used\n".to_vec()
        } else if request.url.ends_with("/checksums.json") {
            br#"{"key_id":"owned-untrusted-key","checksums_sha256":"","signature":""}"#.to_vec()
        } else {
            panic!(
                "production trust barrier must reject before archive download: {}",
                request.url
            );
        };
        Box::pin(async move {
            Ok(Some(RawResponse {
                status: 200,
                headers: Headers::new(),
                content_length: bytes.len() as i64,
                body: Some(Box::new(Body(bytes))),
            }))
        })
    }
}

#[tokio::test]
async fn production_signed_installer_rejects_an_unknown_key_before_archive_or_target_access() {
    use pixiv_app::update::installer::{native_goarch, native_goos};
    let archive = format!(
        "pixiv-cli_1.2.4_{}_{}.tar.gz",
        native_goos(),
        native_goarch()
    );
    let names = [archive.as_str(), "checksums.txt", "checksums.json"];
    let assets: Vec<_> = names.iter().map(|name| serde_json::json!({
        "name": name,
        "browser_download_url": format!("https://github.com/FlanChanXwO/pixiv-cli/releases/download/v1.2.4/{name}"),
    })).collect();
    let transport = Arc::new(TrustBarrierTransport {
        requests: Mutex::new(Vec::new()),
        release: serde_json::to_vec(&serde_json::json!([{
            "tag_name": "v1.2.4", "prerelease": false, "draft": false, "assets": assets,
        }]))
        .unwrap(),
    });
    let directory = tempfile::tempdir().unwrap();
    let (out, _) = writer();
    let (err, _) = writer();
    let coordinator = new_coordinator_with_options(
        ProductionUpdateOptions {
            proxy: "http://proxy.invalid:8080".into(),
            cache_directory: Some(directory.path().join("cache")),
            http_transport: Some(transport.clone()),
        },
        out,
        err,
    )
    .unwrap();
    let error = coordinator
        .execute(
            background(),
            UpdateRequest {
                build_info: BuildInfo {
                    version: "v1.2.3".into(),
                },
                check: false,
                include_prerelease: false,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "install release update \"v1.2.4\": checksums manifest references unknown key ID \"owned-untrusted-key\""
    );
    assert_eq!(
        transport.requests.lock().unwrap().as_slice(),
        [
            "https://api.github.com/repos/FlanChanXwO/pixiv-cli/releases",
            "https://github.com/FlanChanXwO/pixiv-cli/releases/download/v1.2.4/checksums.txt",
            "https://github.com/FlanChanXwO/pixiv-cli/releases/download/v1.2.4/checksums.json",
        ]
    );
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[tokio::test]
async fn normal_commands_preserve_exact_argv_and_stream_both_output_pipes() {
    let helper = OwnedCommandHelper::build();
    let (out, out_buffer) = writer();
    let (err, err_buffer) = writer();
    let runner = ProcessCommandRunner::new(Some(out), Some(err));
    let args = [
        "args",
        "space value",
        "$(owned shell syntax)",
        "a;b",
        "",
        "--flag=value",
    ];
    let owned_args: Vec<String> = args.iter().map(|value| (*value).into()).collect();
    let go = helper.oracle(
        "normal",
        "background",
        &helper.executable.to_string_lossy(),
        &owned_args,
    );
    runner
        .run(
            background(),
            Command {
                name: helper.executable.to_string_lossy().into_owned(),
                args: owned_args,
            },
        )
        .await
        .unwrap();
    let value: Vec<String> = serde_json::from_slice(&out_buffer.0.lock().unwrap()).unwrap();
    assert_eq!(value, &args[1..]);
    assert_eq!(*err_buffer.0.lock().unwrap(), b"owned stderr\n");
    assert_eq!(
        String::from_utf8(out_buffer.0.lock().unwrap().clone()).unwrap(),
        go["stdout"]
    );
    assert_eq!(
        String::from_utf8(err_buffer.0.lock().unwrap().clone()).unwrap(),
        go["stderr"]
    );
    assert_eq!(go["error"], "");
    out_buffer.0.lock().unwrap().clear();
    err_buffer.0.lock().unwrap().clear();
    runner
        .run(
            background(),
            Command {
                name: helper.executable.to_string_lossy().into_owned(),
                args: vec!["large".into()],
            },
        )
        .await
        .unwrap();
    assert_eq!(*out_buffer.0.lock().unwrap(), vec![b'o'; 131072]);
    assert_eq!(*err_buffer.0.lock().unwrap(), vec![b'e'; 131072]);
    ProcessCommandRunner::new(None, None)
        .run(
            background(),
            Command {
                name: helper.executable.to_string_lossy().into_owned(),
                args: vec!["large".into()],
            },
        )
        .await
        .unwrap();
    let (combined, combined_buffer) = writer();
    ProcessCommandRunner::new(Some(combined.clone()), Some(combined))
        .run(
            background(),
            Command {
                name: helper.executable.to_string_lossy().into_owned(),
                args: vec!["exit".into(), "0".into()],
            },
        )
        .await
        .unwrap();
    assert_eq!(
        *combined_buffer.0.lock().unwrap(),
        b"owned output\nowned error\n"
    );
    let go = helper.oracle(
        "combined",
        "background",
        &helper.executable.to_string_lossy(),
        &["exit".into(), "0".into()],
    );
    assert_eq!(
        String::from_utf8(combined_buffer.0.lock().unwrap().clone()).unwrap(),
        go["stdout"]
    );
}

#[tokio::test]
async fn empty_and_lookup_errors_precede_precanceled_context_while_valid_paths_preserve_context_causes()
 {
    let helper = OwnedCommandHelper::build();
    let runner = ProcessCommandRunner::new(None, None);
    let context = Context::new();
    context.cancel();
    let error = runner
        .run(Arc::new(context.clone()), Command::default())
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "command name is empty");
    let go = helper.oracle("normal", "precanceled", "", &[]);
    assert_eq!(error.to_string(), go["error"]);
    let name = "pixiv-owned-definitely-missing-command-assembly";
    let error = runner
        .run(
            Arc::new(context.clone()),
            Command {
                name: name.into(),
                args: vec![],
            },
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("run command \"{name}\": exec: \"{name}\": executable file not found in $PATH")
    );
    assert_eq!(context_cause(error.as_ref()), None);
    let go = helper.oracle("normal", "precanceled", name, &[]);
    assert_eq!(error.to_string(), go["error"]);
    assert_eq!(go["canceled_cause"], false);
    let directory = tempfile::tempdir().unwrap();
    let name = directory
        .path()
        .join("missing")
        .to_string_lossy()
        .into_owned();
    let error = runner
        .run(
            Arc::new(context),
            Command {
                name: name.clone(),
                args: vec![],
            },
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("run command \"{name}\": context canceled")
    );
    assert_eq!(context_cause(error.as_ref()), Some(ContextError::Canceled));
    let go = helper.oracle("normal", "precanceled", &name, &[]);
    assert_eq!(error.to_string(), go["error"]);
    assert_eq!(go["canceled_cause"], true);
    let error = runner
        .run(
            background(),
            Command {
                name: name.clone(),
                args: vec![],
            },
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("run command \"{name}\": fork/exec {name}: no such file or directory")
    );
    let go = helper.oracle("normal", "background", &name, &[]);
    assert_eq!(error.to_string(), go["error"]);
}

#[tokio::test]
async fn after_start_cancellation_kills_and_waits_without_turning_exit_into_a_context_cause() {
    let helper = OwnedCommandHelper::build();
    let heartbeat = helper.directory.path().join("heartbeat");
    let (out, buffer) = writer();
    let runner = Arc::new(ProcessCommandRunner::new(Some(out), None));
    let context = Context::new();
    let task = tokio::spawn({
        let context = context.clone();
        let runner = runner.clone();
        let name = helper.executable.to_string_lossy().into_owned();
        let path = heartbeat.to_string_lossy().into_owned();
        async move {
            runner
                .run(
                    Arc::new(context),
                    Command {
                        name,
                        args: vec!["hold".into(), path],
                    },
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while buffer.0.lock().unwrap().as_slice() != b"started\n" {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    context.cancel();
    let error = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().ends_with(": signal: killed"));
    assert_eq!(context_cause(error.as_ref()), None);
    let stopped = fs::read(&heartbeat).unwrap();
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert_eq!(fs::read(&heartbeat).unwrap(), stopped);
    let go = helper.oracle(
        "normal",
        "cancel-after-start",
        &helper.executable.to_string_lossy(),
        &["hold".into(), heartbeat.to_string_lossy().into_owned()],
    );
    assert_eq!(error.to_string(), go["error"]);
    assert_eq!(go["canceled_cause"], false);
}

struct BrokenWriter;
impl Write for BrokenWriter {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("owned writer failure"))
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
struct ShortWriter;
impl Write for ShortWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        Ok(bytes.len().saturating_sub(1))
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
struct OversizeWriter;
impl Write for OversizeWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        Ok(bytes.len() + 1)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[tokio::test]
async fn copy_errors_are_reported_only_after_successful_child_exit_and_short_writes_fail() {
    let helper = OwnedCommandHelper::build();
    for (code, short, expected) in [
        (0, false, "owned writer failure"),
        (7, false, "exit status 7"),
        (0, true, "short write"),
    ] {
        let out = if short {
            shared_writer(ShortWriter)
        } else {
            shared_writer(BrokenWriter)
        };
        let runner = ProcessCommandRunner::new(Some(out), None);
        let error = runner
            .run(
                background(),
                Command {
                    name: helper.executable.to_string_lossy().into_owned(),
                    args: vec!["write".into(), code.to_string()],
                },
            )
            .await
            .unwrap_err();
        assert!(
            error.to_string().ends_with(&format!(": {expected}")),
            "{error}"
        );
        assert_eq!(context_cause(error.as_ref()), None);
        let go = helper.oracle(
            if short { "short" } else { "error" },
            "background",
            &helper.executable.to_string_lossy(),
            &["write".into(), code.to_string()],
        );
        assert_eq!(error.to_string(), go["error"]);
        assert_eq!(go["canceled_cause"], false);
    }
    let runner = ProcessCommandRunner::new(None, Some(shared_writer(BrokenWriter)));
    let error = runner
        .run(
            background(),
            Command {
                name: helper.executable.to_string_lossy().into_owned(),
                args: vec!["write".into(), "0".into()],
            },
        )
        .await
        .unwrap_err();
    assert!(error.to_string().ends_with(": owned writer failure"));
    let go = helper.oracle(
        "stderr-error",
        "background",
        &helper.executable.to_string_lossy(),
        &["write".into(), "0".into()],
    );
    assert_eq!(error.to_string(), go["error"]);
    let runner = ProcessCommandRunner::new(Some(shared_writer(OversizeWriter)), None);
    let error = runner
        .run(
            background(),
            Command {
                name: helper.executable.to_string_lossy().into_owned(),
                args: vec!["write".into(), "0".into()],
            },
        )
        .await
        .unwrap_err();
    assert!(error.to_string().ends_with(": invalid write result"));
    let go = helper.oracle(
        "oversize",
        "background",
        &helper.executable.to_string_lossy(),
        &["write".into(), "0".into()],
    );
    assert_eq!(error.to_string(), go["error"]);
}
