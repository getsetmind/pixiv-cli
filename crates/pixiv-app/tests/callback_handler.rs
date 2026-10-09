use pixiv_app::{
    callback_handler::*,
    handoff_client::{
        HandoffBody, HandoffFuture, HandoffRead, HandoffRequest, HandoffResponse, HandoffTransport,
        HandoffTransportError,
    },
    handoff_protocol::RemoteLoginStart,
    handoff_state::{ActiveRemoteLogin, HandoffState, HandoffStateError},
};
use serde::Deserialize;
use std::{
    error::Error,
    fmt, io,
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

#[derive(Deserialize)]
struct EndpointCase {
    name: String,
    input: String,
    output: String,
    error: String,
}
#[derive(Deserialize)]
struct DispatchCase {
    name: String,
    input: String,
    local: String,
    remote: String,
    delegate_error: String,
    calls: Vec<String>,
    result: String,
    error: String,
}
#[derive(Deserialize)]
struct BrowserCase {
    name: String,
    kind: String,
    open_error: bool,
    body: String,
    calls: Vec<String>,
    error: String,
}
#[derive(Deserialize)]
struct PathCase {
    name: String,
    home: String,
    #[serde(default)]
    unset: bool,
    output: String,
    error: String,
}
#[derive(Deserialize)]
struct RelayCase {
    name: String,
    endpoint: String,
    callback: String,
    output: String,
}
#[derive(Deserialize)]
struct Fixture {
    path_inputs: Vec<PathCase>,
    relay: Vec<RelayCase>,
    paths: serde_json::Value,
    endpoints: Vec<EndpointCase>,
    dispatch: Vec<DispatchCase>,
    browser: Vec<BrowserCase>,
}
fn fixture() -> Fixture {
    serde_json::from_str(include_str!("fixtures/callback_dispatch.json")).unwrap()
}
type Calls = Arc<Mutex<Vec<String>>>;
fn calls() -> Calls {
    Arc::default()
}
fn record(calls: &Calls, value: &str) {
    calls.lock().unwrap().push(value.into());
}
fn error(message: &str) -> CallbackError {
    Box::new(io::Error::other(message.to_owned()))
}
fn error_text<T>(result: &CallbackResult<T>) -> String {
    result
        .as_ref()
        .err()
        .map(ToString::to_string)
        .unwrap_or_default()
}
#[derive(Debug)]
struct Wrapped(CallbackError);
impl fmt::Display for Wrapped {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "wrapped: {}", self.0)
    }
}
impl Error for Wrapped {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}
struct Endpoint {
    mode: String,
    calls: Calls,
    raw: String,
    log: bool,
}
impl CallbackEndpointStore for Endpoint {
    fn local_relay_url(&self, raw: &str) -> CallbackResult<String> {
        assert_eq!(raw, self.raw);
        if self.log {
            record(&self.calls, "local");
        }
        match self.mode.as_str() {
            "success" => Ok("http://localhost:9/callback#synthetic".into()),
            "inactive" => Err(Box::new(CallbackEndpointError::NoActiveLocalCallback)),
            "wrapped_inactive" => Err(Box::new(Wrapped(Box::new(
                CallbackEndpointError::NoActiveLocalCallback,
            )))),
            _ => Err(error("synthetic local failure")),
        }
    }
}
struct Previous {
    calls: Calls,
    raw: String,
    failure: String,
    cancellation: CancellationToken,
}
impl PreviousHandler for Previous {
    fn delegate<'a>(
        &'a self,
        raw: &'a str,
        cancellation: &'a CancellationToken,
    ) -> HandoffFuture<'a, CallbackResult<()>> {
        Box::pin(async move {
            assert_eq!(raw, self.raw);
            self.cancellation.cancel();
            assert!(cancellation.is_cancelled());
            record(&self.calls, "delegate");
            if self.failure.is_empty() {
                Ok(())
            } else {
                Err(error(&self.failure))
            }
        })
    }
}
struct Session {
    calls: Calls,
    result: String,
}
impl RemoteCallback for Session {
    fn result_url(&self) -> &str {
        &self.result
    }
    fn complete(&self) -> HandoffFuture<'_, CallbackResult<()>> {
        Box::pin(async { Ok(()) })
    }
    fn abort(&self) {
        record(&self.calls, "abort");
    }
}
struct Handoff {
    mode: String,
    calls: Calls,
    raw: String,
    clear_error: bool,
    cancellation: CancellationToken,
}
impl RemoteLoginHandoff for Handoff {
    type Session = Session;
    fn start<'a>(
        &'a self,
        start: &'a RemoteLoginStart,
        cancellation: &'a CancellationToken,
    ) -> HandoffFuture<'a, CallbackResult<String>> {
        Box::pin(async move {
            assert_eq!(start.origin, "https://relay.example");
            assert_eq!(start.session_id, "session");
            assert_eq!(start.proof, "synthetic");
            self.cancellation.cancel();
            assert!(cancellation.is_cancelled());
            record(&self.calls, "start");
            Ok("https://app-api.pixiv.net/web/v1/login?synthetic".into())
        })
    }
    fn forward_callback<'a>(
        &'a self,
        raw: &'a str,
        cancellation: &'a CancellationToken,
    ) -> HandoffFuture<'a, CallbackResult<Session>> {
        Box::pin(async move {
            assert_eq!(raw, self.raw);
            self.cancellation.cancel();
            assert!(cancellation.is_cancelled());
            record(&self.calls, "remote");
            match self.mode.as_str() {
                "success" => Ok(Session {
                    calls: self.calls.clone(),
                    result: "https://relay.example/result/AA".into(),
                }),
                "inactive" => {
                    Err(Box::new(HandoffStateError::NoActiveRemoteLogin) as CallbackError)
                }
                "wrapped_inactive" => Err(Box::new(Wrapped(Box::new(
                    HandoffStateError::NoActiveRemoteLogin,
                ))) as CallbackError),
                _ => Err(error("synthetic remote failure")),
            }
        })
    }
    fn clear(&self, _: &RemoteLoginStart) -> CallbackResult<()> {
        record(&self.calls, "clear");
        if self.clear_error {
            Err(error("secret clear failure"))
        } else {
            Ok(())
        }
    }
}
struct Browser {
    calls: Calls,
    failure: bool,
    expected_url: String,
}
impl BrowserOpener for Browser {
    fn open(&self, url: &str) -> CallbackResult<()> {
        assert_eq!(url, self.expected_url);
        record(&self.calls, "open");
        if self.failure {
            Err(error("secret callback URL and local path"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn endpoints_match_go_validation_and_canonical_serialization() {
    for case in fixture().endpoints {
        let result = validated_callback_endpoint(&case.input);
        assert_eq!(
            result
                .as_ref()
                .err()
                .map(ToString::to_string)
                .unwrap_or_default(),
            case.error,
            "{}",
            case.name
        );
        assert_eq!(result.unwrap_or_default(), case.output, "{}", case.name);
    }
}
#[test]
fn endpoint_storage_is_private_and_preserves_original_callback_fragment() {
    let directory = tempfile::tempdir().unwrap();
    let store = FileCallbackEndpointStore::new(directory.path().join("private/endpoint"));
    assert_eq!(
        store
            .local_relay_url("pixiv://account/login?code=synthetic")
            .unwrap_err()
            .to_string(),
        "Pixiv login callback is no longer active"
    );
    store
        .write("http://synthetic@localhost:/%63allback?")
        .unwrap();
    assert_eq!(
        std::fs::read(store.path()).unwrap(),
        b"http://synthetic@localhost:/%63allback?\n"
    );
    for case in fixture().relay {
        store.write(&case.endpoint).unwrap();
        assert_eq!(
            store.local_relay_url(&case.callback).unwrap(),
            case.output,
            "{}",
            case.name
        );
    }
    store
        .write("http://synthetic@localhost:/%63allback?")
        .unwrap();
    let raw = " pixiv://account/login?code=synthetic&state=a+b ";
    assert_eq!(
        store.local_relay_url(raw).unwrap(),
        "http://synthetic@localhost:/%63allback?#%20pixiv://account/login?code=synthetic&state=a+b%20"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(store.path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(store.path().parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
    std::fs::write(store.path(), b"https://external.invalid/callback").unwrap();
    assert_eq!(
        store.local_relay_url(raw).unwrap_err().to_string(),
        "Pixiv login callback endpoint is invalid"
    );
    assert_eq!(
        store
            .local_relay_url("pixiv://account/works/1")
            .unwrap_err()
            .to_string(),
        "invalid Pixiv callback URL"
    );
}
#[tokio::test]
async fn dispatch_matches_go_precedence_and_preserves_raw_input() {
    for case in fixture().dispatch {
        let calls = calls();
        let cancellation = CancellationToken::new();
        let endpoint = Endpoint {
            mode: case.local,
            calls: calls.clone(),
            raw: case.input.clone(),
            log: true,
        };
        let handoff = Handoff {
            mode: case.remote,
            calls: calls.clone(),
            raw: case.input.clone(),
            clear_error: false,
            cancellation: cancellation.clone(),
        };
        let previous = Previous {
            calls: calls.clone(),
            raw: case.input.clone(),
            failure: case.delegate_error,
            cancellation: cancellation.clone(),
        };
        let result =
            handle_callback(&case.input, &cancellation, &endpoint, &handoff, &previous).await;
        assert_eq!(error_text(&result), case.error, "{}", case.name);
        let kind = match result {
            Ok(CallbackHandlingResult::LocalRelay(_)) => "local",
            Ok(CallbackHandlingResult::RemoteCallback(_)) => "remote",
            Ok(CallbackHandlingResult::RemoteLoginStart(_)) => "start",
            _ => "",
        };
        assert_eq!(kind, case.result, "{}", case.name);
        assert_eq!(*calls.lock().unwrap(), case.calls, "{}", case.name);
    }
}

struct Body {
    calls: Calls,
    bytes: Mutex<(Vec<u8>, usize)>,
}
impl HandoffBody for Body {
    fn read<'a>(&'a self, buffer: &'a mut [u8]) -> HandoffFuture<'a, HandoffRead> {
        Box::pin(async move {
            record(&self.calls, "read");
            let mut bytes = self.bytes.lock().unwrap();
            let count = buffer.len().min(bytes.0.len() - bytes.1);
            buffer[..count].copy_from_slice(&bytes.0[bytes.1..bytes.1 + count]);
            bytes.1 += count;
            HandoffRead { count, error: None }
        })
    }
    fn close(&self) {
        record(&self.calls, "close");
    }
}
struct Transport {
    calls: Calls,
    body: String,
}
impl HandoffTransport for Transport {
    fn send<'a>(
        &'a self,
        request: HandoffRequest,
        _: CancellationToken,
    ) -> HandoffFuture<'a, Result<HandoffResponse, HandoffTransportError>> {
        Box::pin(async move {
            assert_eq!(request.method, "POST");
            assert!(request.endpoint.ends_with("/callback/session"));
            record(&self.calls, "forward");
            Ok(HandoffResponse {
                status: 200,
                headers: vec![(
                    "X-Pixiv-Relay-Result-URL".into(),
                    "https://relay.example/result/AA".into(),
                )],
                body: Box::new(Body {
                    calls: self.calls.clone(),
                    bytes: Mutex::new((self.body.as_bytes().to_vec(), 0)),
                }),
            })
        })
    }
}
#[tokio::test]
async fn browser_actions_match_go_and_complete_only_after_open() {
    for case in fixture().browser {
        let calls = calls();
        let cancellation = CancellationToken::new();
        let raw = match case.kind.as_str() {
            "delegate" => "pixiv://account/works/1",
            "start" => {
                "pixiv://account/remote-login?origin=https%3A%2F%2Frelay.example&session=session&access=synthetic"
            }
            _ => "pixiv://account/login?code=synthetic",
        };
        let endpoint = Endpoint {
            mode: if case.kind == "local" {
                "success"
            } else {
                "inactive"
            }
            .into(),
            calls: calls.clone(),
            raw: raw.into(),
            log: false,
        };
        let previous = Previous {
            calls: calls.clone(),
            raw: raw.into(),
            failure: String::new(),
            cancellation: cancellation.clone(),
        };
        let browser = Browser {
            calls: calls.clone(),
            failure: case.open_error,
            expected_url: match case.kind.as_str() {
                "local" => "http://localhost:9/callback#synthetic",
                "start" => "https://app-api.pixiv.net/web/v1/login?synthetic",
                _ => "https://relay.example/result/AA",
            }
            .into(),
        };
        let result = if case.kind == "remote" {
            let directory = tempfile::tempdir().unwrap();
            let state = HandoffState::new(directory.path().join("active"));
            state
                .save(&ActiveRemoteLogin {
                    version: 1,
                    origin: "https://relay.example".into(),
                    session_id: "session".into(),
                    proof: "synthetic".into(),
                })
                .unwrap();
            let handoff = ClientHandoff::new(
                Transport {
                    calls: calls.clone(),
                    body: case.body,
                },
                state.clone(),
            );
            let result =
                run_callback(raw, &cancellation, &endpoint, &handoff, &previous, &browser).await;
            assert!(!state.path().exists());
            result
        } else {
            let handoff = Handoff {
                mode: "inactive".into(),
                calls: calls.clone(),
                raw: raw.into(),
                clear_error: false,
                cancellation: cancellation.clone(),
            };
            run_callback(raw, &cancellation, &endpoint, &handoff, &previous, &browser).await
        };
        assert_eq!(error_text(&result), case.error, "{}", case.name);
        assert_eq!(*calls.lock().unwrap(), case.calls, "{}", case.name);
    }
}
#[tokio::test]
async fn failed_start_cleanup_error_takes_precedence_over_browser_error() {
    let calls = calls();
    let cancellation = CancellationToken::new();
    let raw = "pixiv://account/remote-login?origin=https%3A%2F%2Frelay.example&session=session&access=synthetic";
    let endpoint = Endpoint {
        mode: "error".into(),
        calls: calls.clone(),
        raw: raw.into(),
        log: true,
    };
    let handoff = Handoff {
        mode: "error".into(),
        calls: calls.clone(),
        raw: raw.into(),
        clear_error: true,
        cancellation: cancellation.clone(),
    };
    let previous = Previous {
        calls: calls.clone(),
        raw: raw.into(),
        failure: String::new(),
        cancellation: cancellation.clone(),
    };
    let browser = Browser {
        calls: calls.clone(),
        failure: true,
        expected_url: "https://app-api.pixiv.net/web/v1/login?synthetic".into(),
    };
    let result = run_callback(raw, &cancellation, &endpoint, &handoff, &previous, &browser).await;
    assert_eq!(
        error_text(&result),
        "could not clear remote Pixiv login handoff after browser launch failed"
    );
    assert_eq!(*calls.lock().unwrap(), ["start", "open", "clear"]);
}
#[test]
fn default_paths_match_go_in_isolated_process() {
    let directory = tempfile::tempdir().unwrap();
    let variable = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "default_paths_child", "--ignored"])
        .env(variable, directory.path())
        .env("PIXIV_CALLBACK_PATH_HOME", directory.path())
        .env("XDG_DATA_HOME", directory.path().join("ignored"))
        .env("APPDATA", directory.path().join("ignored"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    #[cfg(not(windows))]
    for case in fixture().path_inputs {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "path_input_child", "--ignored"])
            .env("PIXIV_CALLBACK_PATH_CASE", &case.name);
        if case.unset {
            command.env_remove("HOME");
        } else {
            command.env("HOME", &case.home);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}: {} {}",
            case.name,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
#[test]
#[ignore]
fn default_paths_child() {
    let home = std::path::PathBuf::from(std::env::var_os("PIXIV_CALLBACK_PATH_HOME").unwrap());
    let fixture = fixture();
    assert_eq!(
        callback_endpoint_path().unwrap(),
        home.join(fixture.paths["endpoint"].as_str().unwrap())
    );
    assert_eq!(
        active_remote_login_path().unwrap(),
        home.join(fixture.paths["active"].as_str().unwrap())
    );
    assert_eq!(
        handler_manifest_path().unwrap(),
        home.join(fixture.paths["manifest"].as_str().unwrap())
    );
}

#[test]
#[ignore]
fn path_input_child() {
    let name = std::env::var("PIXIV_CALLBACK_PATH_CASE").unwrap();
    let case = fixture()
        .path_inputs
        .into_iter()
        .find(|case| case.name == name)
        .unwrap();
    let result = callback_endpoint_path();
    assert_eq!(
        result
            .as_ref()
            .err()
            .map(ToString::to_string)
            .unwrap_or_default(),
        case.error
    );
    assert_eq!(
        result
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default(),
        case.output
    );
}
