use pixiv_app::{
    callback_handler::{CallbackResult, PreviousHandler},
    handler_manifest::{HandlerManifest, HandlerManifestStore},
    handoff_client::HandoffFuture,
    host_process::HostPlatform,
    lifecycle::{Context, ContextError},
    url_handler::{
        PlatformUrlHandler, SystemUrlHandler, UrlHandler, UrlHandlerEnvironment,
        UrlHandlerInstallation,
    },
};
use std::{
    ffi::OsString,
    io,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

type Calls = Arc<Mutex<Vec<String>>>;
fn fixture() -> serde_json::Value {
    serde_json::from_str(include_str!(
        "../../pixiv-cli/tests/fixtures/cli_login_hidden_startup.json"
    ))
    .unwrap()
}
fn platform(value: &str) -> HostPlatform {
    match value {
        "linux" => HostPlatform::Linux,
        "darwin" => HostPlatform::Darwin,
        "windows" => HostPlatform::Windows,
        "freebsd" => HostPlatform::FreeBsd,
        _ => panic!("unsupported fixture platform {value}"),
    }
}
struct Environment {
    platform: HostPlatform,
    program: OsString,
    executable: PathBuf,
    executable_error: String,
    calls: Calls,
}
impl UrlHandlerEnvironment for Environment {
    fn platform(&self) -> HostPlatform {
        self.platform
    }
    fn program_name(&self) -> OsString {
        self.program.clone()
    }
    fn executable_path(&self) -> io::Result<PathBuf> {
        self.calls.lock().unwrap().push("executable".into());
        if self.executable_error.is_empty() {
            Ok(self.executable.clone())
        } else {
            Err(io::Error::other(self.executable_error.clone()))
        }
    }
}
struct Backend {
    calls: Calls,
    manifest: HandlerManifestStore,
    executable: String,
    ensure_error: String,
    cancellation: Option<Context>,
}
impl Backend {
    fn check_context(&self, context: &Context) {
        if let Some(cancellation) = &self.cancellation {
            cancellation.cancel();
            assert_eq!(context.error(), Some(ContextError::Canceled));
        }
    }
}
impl PlatformUrlHandler for Backend {
    fn ensure_persistent(&self, context: &Context) -> CallbackResult<()> {
        self.check_context(context);
        self.calls.lock().unwrap().push("ensure".into());
        if !self.ensure_error.is_empty() {
            return Err(Box::new(io::Error::other(self.ensure_error.clone())));
        }
        let mut manifest = self.manifest.load()?.unwrap_or_else(|| HandlerManifest {
            version: 1,
            previous_handler: "synthetic.previous".into(),
            ..HandlerManifest::default()
        });
        manifest.executable_path = self.executable.clone();
        self.manifest.save(&manifest)?;
        Ok(())
    }
    fn disable_persistent(&self, context: &Context) -> CallbackResult<()> {
        self.check_context(context);
        self.calls.lock().unwrap().push("disable".into());
        Ok(())
    }
    fn install(
        &self,
        context: &Context,
        relay: &str,
    ) -> CallbackResult<Box<dyn UrlHandlerInstallation>> {
        self.check_context(context);
        self.calls.lock().unwrap().push(format!("install:{relay}"));
        Ok(Box::new(Installation(self.calls.clone())))
    }
}
impl PreviousHandler for Backend {
    fn delegate<'a>(
        &'a self,
        raw: &'a str,
        cancellation: &'a CancellationToken,
    ) -> HandoffFuture<'a, CallbackResult<()>> {
        Box::pin(async move {
            assert!(cancellation.is_cancelled());
            self.calls.lock().unwrap().push(format!("delegate:{raw}"));
            Ok(())
        })
    }
}
struct Installation(Calls);
impl UrlHandlerInstallation for Installation {
    fn cleanup(&mut self) {
        self.0.lock().unwrap().push("cleanup".into());
    }
}

#[test]
fn automatic_registration_preserves_frozen_platform_and_program_policy() {
    for case in fixture()["startup_policy"].as_array().unwrap() {
        let calls = Calls::default();
        let home = tempfile::tempdir().unwrap();
        let manifest = HandlerManifestStore::new(home.path().join("handler-manifest.json"));
        let handler = SystemUrlHandler::with_environment(
            Arc::new(Environment {
                platform: platform(case["os"].as_str().unwrap()),
                program: case["argv0"].as_str().unwrap().into(),
                executable: "/opt/current/pixiv".into(),
                executable_error: "automatic support must not inspect the executable".into(),
                calls: calls.clone(),
            }),
            Arc::new(Backend {
                calls: calls.clone(),
                manifest: manifest.clone(),
                executable: "/opt/current/pixiv".into(),
                ensure_error: String::new(),
                cancellation: None,
            }),
            manifest,
        );
        assert_eq!(
            handler.automatic_supported(),
            case["supported"].as_bool().unwrap(),
            "{}",
            case["name"]
        );
        assert!(calls.lock().unwrap().is_empty(), "{}", case["name"]);
        assert!(
            std::fs::read_dir(home.path()).unwrap().next().is_none(),
            "{}",
            case["name"]
        );
    }
}

#[test]
fn persistent_registration_preserves_go_executable_manifest_order_and_retry_policy() {
    for case in fixture()["persistent_policy"].as_array().unwrap() {
        let calls = Calls::default();
        let home = tempfile::tempdir().unwrap();
        let manifest =
            HandlerManifestStore::new(home.path().join("url-handler/handler-manifest.json"));
        let before = case["manifest"].as_str();
        if let Some(before) = before {
            std::fs::create_dir_all(manifest.path().parent().unwrap()).unwrap();
            std::fs::write(manifest.path(), before).unwrap();
        }
        let context = Context::new();
        let handler = SystemUrlHandler::with_environment(
            Arc::new(Environment {
                platform: platform(case["os"].as_str().unwrap()),
                program: case["argv0"].as_str().unwrap().into(),
                executable: case["executable"].as_str().unwrap().into(),
                executable_error: case["executable_error"].as_str().unwrap().into(),
                calls: calls.clone(),
            }),
            Arc::new(Backend {
                calls: calls.clone(),
                manifest: manifest.clone(),
                executable: case["executable"].as_str().unwrap().into(),
                ensure_error: case["ensure_error"].as_str().unwrap().into(),
                cancellation: Some(context.clone()),
            }),
            manifest.clone(),
        );
        let name = case["name"].as_str().unwrap();
        for _ in 0..case["repeats"].as_u64().unwrap() {
            let result = handler.ensure_if_needed(&context);
            assert_eq!(
                result
                    .err()
                    .map(|error| error.to_string())
                    .unwrap_or_default(),
                case["error"].as_str().unwrap(),
                "{name}"
            );
        }
        let expected: Vec<String> = case["calls"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().into())
            .collect();
        assert_eq!(*calls.lock().unwrap(), expected, "{name}");
        if expected.iter().any(|call| call == "ensure")
            && case["ensure_error"].as_str().unwrap().is_empty()
        {
            let saved = manifest.load().unwrap().unwrap();
            assert_eq!(
                saved.executable_path,
                case["executable"].as_str().unwrap(),
                "{name}"
            );
            assert_eq!(saved.previous_handler, "synthetic.previous", "{name}");
        } else {
            assert_eq!(
                std::fs::read(manifest.path()).ok().as_deref(),
                before.map(str::as_bytes),
                "{name}"
            );
        }
    }
}

#[tokio::test]
async fn explicit_handler_operations_forward_context_url_and_installation_ownership() {
    let calls = Calls::default();
    let home = tempfile::tempdir().unwrap();
    let manifest = HandlerManifestStore::new(home.path().join("handler-manifest.json"));
    let context = Context::new();
    let handler = SystemUrlHandler::with_environment(
        Arc::new(Environment {
            platform: HostPlatform::Linux,
            program: "/opt/bin/pixiv.test".into(),
            executable: "/opt/current/pixiv".into(),
            executable_error: "explicit operations must use the platform backend".into(),
            calls: calls.clone(),
        }),
        Arc::new(Backend {
            calls: calls.clone(),
            manifest: manifest.clone(),
            executable: "/opt/current/pixiv".into(),
            ensure_error: String::new(),
            cancellation: Some(context.clone()),
        }),
        manifest,
    );
    assert!(!handler.automatic_supported());
    handler.ensure_persistent(&context).unwrap();
    handler.disable_persistent(&context).unwrap();
    let mut installation = handler
        .install(&context, "http://127.0.0.1:41871/callback")
        .unwrap();
    installation.cleanup();
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    handler
        .delegate("pixiv://account/works/42", &cancellation)
        .await
        .unwrap();
    assert_eq!(
        *calls.lock().unwrap(),
        [
            "ensure",
            "disable",
            "install:http://127.0.0.1:41871/callback",
            "cleanup",
            "delegate:pixiv://account/works/42",
        ]
    );
}
