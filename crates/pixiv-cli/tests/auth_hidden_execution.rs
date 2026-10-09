use pixiv_app::{
    callback_handler::{
        BrowserOpener, CallbackResult, FileCallbackEndpointStore, PreviousHandler, RemoteCallback,
        RemoteLoginHandoff,
    },
    handoff_client::HandoffFuture,
    handoff_protocol::RemoteLoginStart,
    handoff_state::HandoffStateError,
    lifecycle::Context,
    url_handler::{UrlHandler, UrlHandlerInstallation},
};
use pixiv_cli_rs::{
    auth_hidden::{HiddenAuthCommand, HiddenAuthDependencies},
    finish_command,
};
use std::{
    io::{self, Write},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

type Calls = Arc<Mutex<Vec<String>>>;
fn fixture() -> serde_json::Value {
    serde_json::from_str(include_str!("fixtures/cli_login_hidden_startup.json")).unwrap()
}
struct NoSession;
impl RemoteCallback for NoSession {
    fn result_url(&self) -> &str {
        panic!("inactive handoff has no result URL")
    }
    fn complete(&self) -> HandoffFuture<'_, CallbackResult<()>> {
        panic!("inactive handoff cannot complete")
    }
    fn abort(&self) {
        panic!("inactive handoff cannot abort")
    }
}
struct InactiveHandoff;
impl RemoteLoginHandoff for InactiveHandoff {
    type Session = NoSession;
    fn start<'a>(
        &'a self,
        _: &'a RemoteLoginStart,
        _: &'a CancellationToken,
    ) -> HandoffFuture<'a, CallbackResult<String>> {
        panic!("selected callback routes must not begin a remote handoff")
    }
    fn forward_callback<'a>(
        &'a self,
        _: &'a str,
        _: &'a CancellationToken,
    ) -> HandoffFuture<'a, CallbackResult<NoSession>> {
        Box::pin(async { Err(Box::new(HandoffStateError::NoActiveRemoteLogin) as _) })
    }
    fn clear(&self, _: &RemoteLoginStart) -> CallbackResult<()> {
        panic!("selected callback routes must not clear remote state")
    }
}
struct Previous {
    calls: Calls,
    context: Option<Context>,
}
impl PreviousHandler for Previous {
    fn delegate<'a>(
        &'a self,
        raw: &'a str,
        cancellation: &'a CancellationToken,
    ) -> HandoffFuture<'a, CallbackResult<()>> {
        Box::pin(async move {
            if let Some(context) = &self.context {
                context.cancel();
                tokio::time::timeout(Duration::from_secs(1), cancellation.cancelled())
                    .await
                    .expect("hidden callback detached its command context");
            }
            self.calls.lock().unwrap().push(format!("delegate:{raw}"));
            Ok(())
        })
    }
}
struct Browser {
    calls: Calls,
    fail: bool,
}
impl BrowserOpener for Browser {
    fn open(&self, url: &str) -> CallbackResult<()> {
        self.calls.lock().unwrap().push(format!("open:{url}"));
        if self.fail {
            Err(Box::new(io::Error::other(
                "synthetic private browser path and callback secret",
            )))
        } else {
            Ok(())
        }
    }
}
struct Handler {
    calls: Calls,
    ensure_error: String,
    context: Option<Context>,
}
impl PreviousHandler for Handler {
    fn delegate<'a>(
        &'a self,
        _: &'a str,
        _: &'a CancellationToken,
    ) -> HandoffFuture<'a, CallbackResult<()>> {
        panic!("hidden command must use its callback previous-handler dependency")
    }
}
impl UrlHandler for Handler {
    fn automatic_supported(&self) -> bool {
        panic!("hidden command must skip root startup hooks")
    }
    fn ensure_if_needed(&self, context: &Context) -> CallbackResult<()> {
        self.calls.lock().unwrap().push("ensure".into());
        if let Some(original) = &self.context {
            original.cancel();
            assert_eq!(context.error(), original.error());
        }
        if self.ensure_error.is_empty() {
            Ok(())
        } else {
            Err(Box::new(io::Error::other(self.ensure_error.clone())))
        }
    }
    fn ensure_persistent(&self, _: &Context) -> CallbackResult<()> {
        panic!("installer must use the conditional persistent policy")
    }
    fn disable_persistent(&self, _: &Context) -> CallbackResult<()> {
        panic!("hidden commands must not disable handler")
    }
    fn install(&self, _: &Context, _: &str) -> CallbackResult<Box<dyn UrlHandlerInstallation>> {
        panic!("hidden commands must not install a temporary handler")
    }
}
struct FailingWriter;
impl Write for FailingWriter {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("synthetic diagnostics failure"))
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn hidden_callback_command_connects_parser_to_existing_dispatch_without_root_state() {
    for case in fixture()["cli"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        if !matches!(
            name,
            "callback-invalid-url"
                | "callback-empty-url"
                | "callback-delegate"
                | "callback-inactive-delegate"
                | "callback-local"
                | "callback-local-open-fails"
                | "callback-local-bad-endpoint"
                | "callback-start-invalid"
                | "callback-bad-config-excluded-hooks"
        ) {
            continue;
        }
        let calls = Calls::default();
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join("config.toml");
        if let Some(before) = case["before"].as_str() {
            std::fs::write(&config, before).unwrap();
        }
        let endpoint = FileCallbackEndpointStore::new(home.path().join("url-handler-endpoint"));
        match case["callback_kind"].as_str().unwrap() {
            "local" => {
                endpoint.write("http://127.0.0.1:41871/callback").unwrap();
            }
            "invalid-endpoint" => {
                std::fs::write(endpoint.path(), "invalid").unwrap();
            }
            _ => {}
        }
        let args: Vec<String> = case["args"]
            .as_array()
            .unwrap()
            .iter()
            .skip(1)
            .map(|value| value.as_str().unwrap().into())
            .collect();
        let command = HiddenAuthCommand::parse(&args).unwrap().unwrap();
        let previous = Previous {
            calls: calls.clone(),
            context: None,
        };
        let browser = Browser {
            calls: calls.clone(),
            fail: case["open_error"].as_bool().unwrap(),
        };
        let handler = Handler {
            calls: calls.clone(),
            ensure_error: String::new(),
            context: None,
        };
        let mut output = Vec::new();
        let mut diagnostics = Vec::new();
        let result = command
            .execute_with_dependencies(
                &Context::new(),
                &mut output,
                &mut diagnostics,
                HiddenAuthDependencies {
                    endpoint: &endpoint,
                    handoff: &InactiveHandoff,
                    previous: &previous,
                    browser: &browser,
                    handler: &handler,
                },
            )
            .await;
        let exit = finish_command(result, false, false, &mut diagnostics);
        assert_eq!(exit, case["exit"].as_i64().unwrap() as i32, "{name}");
        assert_eq!(
            output,
            case["stdout"].as_str().unwrap().as_bytes(),
            "{name}"
        );
        assert_eq!(
            diagnostics,
            case["stderr"].as_str().unwrap().as_bytes(),
            "{name}"
        );
        let expected: Vec<String> = case["calls"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().into())
            .collect();
        assert_eq!(*calls.lock().unwrap(), expected, "{name}");
        assert_eq!(
            std::fs::read(&config).ok().as_deref(),
            case["before"].as_str().map(str::as_bytes),
            "{name}"
        );
        assert!(!home.path().join("pixiv-cli.db").exists(), "{name}");
    }
}

#[tokio::test]
async fn hidden_callback_shares_command_cancellation_with_previous_handler() {
    let home = tempfile::tempdir().unwrap();
    let endpoint = FileCallbackEndpointStore::new(home.path().join("url-handler-endpoint"));
    let context = Context::new();
    let calls = Calls::default();
    let previous = Previous {
        calls: calls.clone(),
        context: Some(context.clone()),
    };
    let browser = Browser {
        calls: calls.clone(),
        fail: false,
    };
    let handler = Handler {
        calls: calls.clone(),
        ensure_error: String::new(),
        context: None,
    };
    HiddenAuthCommand::Callback("pixiv://account/works/42".into())
        .execute_with_dependencies(
            &context,
            &mut Vec::new(),
            &mut Vec::new(),
            HiddenAuthDependencies {
                endpoint: &endpoint,
                handoff: &InactiveHandoff,
                previous: &previous,
                browser: &browser,
                handler: &handler,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        *calls.lock().unwrap(),
        ["delegate:pixiv://account/works/42"]
    );
}

#[tokio::test]
async fn hidden_installer_retains_go_warning_success_and_ignored_diagnostics_failures() {
    for case in fixture()["installer"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let mut args = vec!["_install-handler".to_owned()];
        args.extend(
            case["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap().to_owned()),
        );
        let calls = Calls::default();
        let home = tempfile::tempdir().unwrap();
        let endpoint = FileCallbackEndpointStore::new(home.path().join("url-handler-endpoint"));
        let context = Context::new();
        let previous = Previous {
            calls: calls.clone(),
            context: None,
        };
        let browser = Browser {
            calls: calls.clone(),
            fail: false,
        };
        let handler = Handler {
            calls: calls.clone(),
            ensure_error: case["ensure_error"].as_str().unwrap().into(),
            context: Some(context.clone()),
        };
        let mut output = Vec::new();
        let mut diagnostics = Vec::new();
        let result = match HiddenAuthCommand::parse(&args) {
            Err(error) => Err(error),
            Ok(Some(command)) => {
                let dependencies = HiddenAuthDependencies {
                    endpoint: &endpoint,
                    handoff: &InactiveHandoff,
                    previous: &previous,
                    browser: &browser,
                    handler: &handler,
                };
                if case["error_output_failure"].as_bool().unwrap() {
                    command
                        .execute_with_dependencies(
                            &context,
                            &mut output,
                            &mut FailingWriter,
                            dependencies,
                        )
                        .await
                } else {
                    command
                        .execute_with_dependencies(
                            &context,
                            &mut output,
                            &mut diagnostics,
                            dependencies,
                        )
                        .await
                }
            }
            Ok(None) => panic!("{name}: installer route was not discovered"),
        };
        assert_eq!(
            result
                .err()
                .map(|error| error.to_string())
                .unwrap_or_default(),
            case["error"].as_str().unwrap(),
            "{name}"
        );
        assert_eq!(
            output,
            case["stdout"].as_str().unwrap().as_bytes(),
            "{name}"
        );
        assert_eq!(
            diagnostics,
            case["stderr"].as_str().unwrap().as_bytes(),
            "{name}"
        );
        let expected: Vec<String> = case["calls"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().into())
            .collect();
        assert_eq!(*calls.lock().unwrap(), expected, "{name}");
        assert!(
            std::fs::read_dir(home.path()).unwrap().next().is_none(),
            "{name}"
        );
    }
}
