use pixiv_app::{
    callback_handler::CallbackResult,
    lifecycle::{Context, ContextError},
};
use pixiv_cli_rs::startup::{StartupHooks, run_startup};
use std::{
    io::{self, Write},
    sync::{Arc, Mutex},
};

fn fixture() -> serde_json::Value {
    serde_json::from_str(include_str!("fixtures/cli_login_hidden_startup.json")).unwrap()
}

type Calls = Arc<Mutex<Vec<String>>>;
struct Hooks {
    calls: Calls,
    cleanup_error: String,
    supported: bool,
    ensure_error: String,
    cancellation: Option<Context>,
}
impl StartupHooks for Hooks {
    fn cleanup_pending_update(&self) -> CallbackResult<()> {
        self.calls.lock().unwrap().push("cleanup".into());
        if self.cleanup_error.is_empty() {
            Ok(())
        } else {
            Err(Box::new(io::Error::other(self.cleanup_error.clone())))
        }
    }
    fn automatic_supported(&self) -> bool {
        self.calls.lock().unwrap().push("supported".into());
        self.supported
    }
    fn ensure_if_needed(&self, context: &Context) -> CallbackResult<()> {
        self.calls.lock().unwrap().push("ensure".into());
        if let Some(cancellation) = &self.cancellation {
            cancellation.cancel();
            assert_eq!(context.error(), Some(ContextError::Canceled));
        }
        if self.ensure_error.is_empty() {
            Ok(())
        } else {
            Err(Box::new(io::Error::other(self.ensure_error.clone())))
        }
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

#[test]
fn normal_startup_preserves_go_hook_order_and_hides_registration_details() {
    for case in fixture()["cli"].as_array().unwrap() {
        let expected: Vec<String> = case["calls"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_owned())
            .collect();
        if expected.first().map(String::as_str) != Some("cleanup") {
            continue;
        }
        let calls = Calls::default();
        let context = Context::new();
        let hooks = Hooks {
            calls: calls.clone(),
            cleanup_error: case["cleanup_error"].as_str().unwrap().into(),
            supported: case["supported"].as_bool().unwrap(),
            ensure_error: case["ensure_error"].as_str().unwrap().into(),
            cancellation: Some(context.clone()),
        };
        let mut diagnostics = Vec::new();
        let result = run_startup(&context, &hooks, &mut diagnostics);
        let name = case["name"].as_str().unwrap();
        assert_eq!(*calls.lock().unwrap(), expected, "{name}");
        if hooks.cleanup_error.is_empty() {
            result.unwrap_or_else(|error| panic!("{name}: {error}"));
            let expected = if hooks.supported && !hooks.ensure_error.is_empty() {
                "warning: persistent pixiv:// callback handler was not initialized\n"
            } else {
                ""
            };
            assert_eq!(diagnostics, expected.as_bytes(), "{name}");
        } else {
            assert_eq!(
                result.err().unwrap().to_string(),
                format!("clean pending update: {}", hooks.cleanup_error),
                "{name}"
            );
            assert!(diagnostics.is_empty(), "{name}");
        }
    }
}

#[test]
fn registration_warning_write_failure_does_not_stop_normal_startup() {
    let calls = Calls::default();
    let hooks = Hooks {
        calls: calls.clone(),
        cleanup_error: String::new(),
        supported: true,
        ensure_error: "synthetic private registration failure".into(),
        cancellation: None,
    };
    run_startup(&Context::new(), &hooks, &mut FailingWriter).unwrap();
    assert_eq!(*calls.lock().unwrap(), ["cleanup", "supported", "ensure"]);
}

#[test]
fn cleanup_failure_preserves_raw_startup_error_even_for_machine_output() {
    let fixture = fixture();
    let case = fixture["cli"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == "normal-cleanup-error")
        .unwrap();
    for (ndjson, machine) in [(false, false), (false, true), (true, true)] {
        let hooks = Hooks {
            calls: Calls::default(),
            cleanup_error: case["cleanup_error"].as_str().unwrap().into(),
            supported: true,
            ensure_error: String::new(),
            cancellation: None,
        };
        let mut diagnostics = Vec::new();
        let result = run_startup(&Context::new(), &hooks, &mut diagnostics);
        let exit = pixiv_cli_rs::finish_command(result, ndjson, machine, &mut diagnostics);
        assert_eq!(exit, case["exit"].as_i64().unwrap() as i32);
        assert_eq!(diagnostics, case["stderr"].as_str().unwrap().as_bytes());
        assert_eq!(*hooks.calls.lock().unwrap(), ["cleanup"]);
    }
}

#[test]
fn auth_startup_policy_is_independent_of_configuration_path_requirements() {
    struct NoInput;
    impl io::Read for NoInput {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            panic!("explicit arguments and help must not read stdin");
        }
    }
    for (args, startup, config) in [
        (vec!["login"], true, true),
        (vec!["import", "synthetic"], true, true),
        (vec!["list"], true, true),
        (vec!["use", "42"], true, true),
        (vec!["remove", "42", "--yes"], true, true),
        (vec!["pool", "status"], true, true),
        (vec!["pool", "enable", "42"], true, true),
        (vec!["check", "42"], true, true),
        (vec!["refresh", "42"], true, true),
        (vec!["export", "42"], false, true),
        (vec!["export", "--all"], false, true),
        (
            vec!["export", "42", "--output", "synthetic.json"],
            false,
            true,
        ),
        (vec!["_callback", "pixiv://account/works/42"], false, false),
        (vec!["_install-handler"], false, false),
        (vec!["_callback", "--help"], false, false),
        (vec!["_install-handler", "--help"], false, false),
        (vec!["login", "--help"], false, false),
        (vec!["import", "--help"], false, false),
        (vec!["export", "--help"], false, false),
        (vec!["list", "--help"], false, false),
        (vec!["check", "--help"], false, false),
        (vec!["pool", "status", "--help"], false, false),
        (vec!["--help"], false, false),
        (vec!["--help", "_callback"], false, false),
    ] {
        let args = args.into_iter().map(str::to_owned).collect::<Vec<_>>();
        let command = pixiv_cli_rs::auth_accounts::AuthCommand::parse(&args, &mut NoInput, false)
            .unwrap_or_else(|error| panic!("{args:?}: {error}"));
        assert_eq!(command.requires_startup(), startup, "{args:?}");
        assert_eq!(command.requires_config(), config, "{args:?}");
    }
}
