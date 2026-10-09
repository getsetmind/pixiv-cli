#[path = "support/auth_login.rs"]
mod support;
use pixiv_app::{
    config::Store,
    database::{Database, PixivAccount},
    lifecycle::Context,
};
use pixiv_cli_rs::auth_login::LoginCommand;
use std::{sync::Arc, time::Duration};
use support::{Hooks, OAuth};

fn command(args: &[&str]) -> LoginCommand {
    LoginCommand::parse(&args.iter().map(|value| (*value).into()).collect::<Vec<_>>()).unwrap()
}
#[tokio::test]
async fn local_login_completes_oauth_before_final_page_and_cleans_temporary_handler_once() {
    for json in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let store = Store::new(home.path().join("config.toml"));
        let hooks = Hooks::new(true);
        let mut out = Vec::new();
        let mut args = vec!["login"];
        if json {
            args.push("--json");
        }
        command(&args)
            .execute_with_transport(
                &store,
                &Context::new(),
                &mut out,
                Arc::new(hooks.clone()),
                OAuth {
                    observed: hooks.observed.clone(),
                    status: 200,
                    delay: Duration::ZERO,
                },
            )
            .await
            .unwrap();
        hooks.join_clients();
        let observation = hooks.observed.lock().unwrap();
        assert_eq!(
            observation.events,
            ["ensure", "install", "open", "cleanup", "oauth"]
        );
        assert_eq!(observation.final_status, Some(200));
        assert!(String::from_utf8_lossy(&observation.http).contains("Login successful"));
        assert!(!observation.diagnostics.contains("synthetic-code"));
        assert!(!observation.diagnostics.contains("synthetic-refresh"));
        if json {
            assert_eq!(out, b"{\n  \"user_id\": 42,\n  \"username\": \"fixture-name\",\n  \"default\": true,\n  \"has_token\": true,\n  \"schedulable\": true,\n  \"eligible\": true\n}\n");
        } else {
            assert_eq!(out, "✓ uid:42 username:fixture-name\n".as_bytes());
        }
        assert_eq!(store.read_pixiv_default_user_id().unwrap(), Some(42));
        let account = Database::open(home.path()).unwrap().get_pixiv(42).unwrap();
        assert_eq!(account.refresh_token_copy(), b"synthetic-refresh");
        assert_eq!(account.credential_revision, 1);
    }
}
#[tokio::test]
async fn failed_oauth_sends_failure_page_and_never_saves_a_token_or_summary() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::new(home.path().join("config.toml"));
    let hooks = Hooks::new(true);
    let mut out = Vec::new();
    let error = command(&["login"])
        .execute_with_transport(
            &store,
            &Context::new(),
            &mut out,
            Arc::new(hooks.clone()),
            OAuth {
                observed: hooks.observed.clone(),
                status: 400,
                delay: Duration::ZERO,
            },
        )
        .await
        .err()
        .unwrap();
    assert!(error.sdk_error().is_some());
    hooks.join_clients();
    assert_eq!(hooks.observed.lock().unwrap().final_status, Some(400));
    assert!(out.is_empty());
    assert!(
        Database::open(home.path())
            .unwrap()
            .list_pixiv()
            .unwrap()
            .is_empty()
    );
}
#[tokio::test]
async fn no_open_and_use_flags_override_config_and_parent_cancellation_is_detached() {
    for (flags, expected_default, installed) in [
        (vec![], 42, false),
        (vec!["--no-open=false", "--use=false"], 11, true),
    ] {
        let home = tempfile::tempdir().unwrap();
        let store = Store::new(home.path().join("config.toml"));
        std::fs::write(
            store.path(),
            "[login]\nopen_browser=false\nuse_after_login=true\n",
        )
        .unwrap();
        store.set_pixiv_default_user_id(11).unwrap();
        Database::open(home.path())
            .unwrap()
            .save_pixiv_credential(&PixivAccount::new(11, "previous", b"synthetic-old"))
            .unwrap();
        let hooks = Hooks::new(true);
        let context = Context::new();
        context.cancel();
        let mut args = vec!["login"];
        args.extend(flags);
        command(&args)
            .execute_with_transport(
                &store,
                &context,
                &mut Vec::new(),
                Arc::new(hooks.clone()),
                OAuth {
                    observed: hooks.observed.clone(),
                    status: 200,
                    delay: Duration::ZERO,
                },
            )
            .await
            .unwrap();
        hooks.join_clients();
        assert_eq!(
            store.read_pixiv_default_user_id().unwrap(),
            Some(expected_default)
        );
        assert_eq!(
            hooks
                .observed
                .lock()
                .unwrap()
                .events
                .contains(&"install".into()),
            installed
        );
    }
}
#[tokio::test]
async fn login_timeout_bounds_wait_and_oauth_and_negative_timeout_adds_no_deadline() {
    for (args, submit, delay, succeeds) in [
        (
            vec!["login", "--no-open", "--timeout=5ms"],
            false,
            Duration::ZERO,
            false,
        ),
        (
            vec!["login", "--no-open", "--timeout=10ms"],
            true,
            Duration::from_millis(100),
            false,
        ),
        (
            vec!["login", "--no-open", "--timeout=-1s"],
            true,
            Duration::from_millis(20),
            true,
        ),
    ] {
        let home = tempfile::tempdir().unwrap();
        let store = Store::new(home.path().join("config.toml"));
        let hooks = Hooks::new(submit);
        let result = command(&args)
            .execute_with_transport(
                &store,
                &Context::new(),
                &mut Vec::new(),
                Arc::new(hooks.clone()),
                OAuth {
                    observed: hooks.observed.clone(),
                    status: 200,
                    delay,
                },
            )
            .await;
        assert_eq!(result.is_ok(), succeeds);
        hooks.join_clients();
        if !succeeds {
            let error = result.unwrap_err();
            if submit {
                assert!(pixiv_sdk::error::is_deadline_exceeded(&error));
                assert_eq!(hooks.observed.lock().unwrap().final_status, Some(400));
            } else {
                assert_eq!(error.to_string(), "context deadline exceeded");
            }
            assert!(
                Database::open(home.path())
                    .unwrap()
                    .list_pixiv()
                    .unwrap()
                    .is_empty()
            );
        }
    }
}
#[tokio::test]
async fn terminal_raw_code_emits_line_break_before_summary_and_hook_failures_remain_warnings() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::new(home.path().join("config.toml"));
    let mut hooks = Hooks::new(false);
    hooks.prompt = true;
    hooks.fail_ensure = true;
    hooks.fail_install = true;
    hooks.fail_open = true;
    hooks
        .inputs
        .lock()
        .unwrap()
        .extend(["".into(), "synthetic-code".into()]);
    command(&["login"])
        .execute_with_transport(
            &store,
            &Context::new(),
            &mut Vec::new(),
            Arc::new(hooks.clone()),
            OAuth {
                observed: hooks.observed.clone(),
                status: 200,
                delay: Duration::ZERO,
            },
        )
        .await
        .unwrap();
    let observation = hooks.observed.lock().unwrap();
    assert_eq!(observation.output, "\n\n");
    assert!(
        observation
            .diagnostics
            .contains("invalid login submission: sign-in result cannot be empty\n")
    );
    assert!(observation.diagnostics.contains("warning: persistent pixiv:// callback handler is unavailable: synthetic persistent failure\n"));
    assert!(observation.diagnostics.contains(
        "warning: pixiv:// callback handler is unavailable: synthetic temporary failure\n"
    ));
    assert!(
        observation
            .diagnostics
            .contains("warning: could not open browser: synthetic opener failure\n")
    );
}

struct FailingWriter;
impl std::io::Write for FailingWriter {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("synthetic summary failure"))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn summary_write_error_does_not_undo_login_or_replace_success_page() {
    for json in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let store = Store::new(home.path().join("config.toml"));
        let hooks = Hooks::new(true);
        let mut args = vec!["login", "--no-open"];
        if json {
            args.push("--json");
        }
        let result = command(&args)
            .execute_with_transport(
                &store,
                &Context::new(),
                &mut FailingWriter,
                Arc::new(hooks.clone()),
                OAuth {
                    observed: hooks.observed.clone(),
                    status: 200,
                    delay: Duration::ZERO,
                },
            )
            .await;
        if json {
            assert_eq!(result.unwrap_err().to_string(), "synthetic summary failure");
        } else {
            result.unwrap();
        }
        hooks.join_clients();
        assert_eq!(hooks.observed.lock().unwrap().final_status, Some(200));
        assert_eq!(
            Database::open(home.path())
                .unwrap()
                .get_pixiv(42)
                .unwrap()
                .refresh_token_copy(),
            b"synthetic-refresh"
        );
        assert_eq!(store.read_pixiv_default_user_id().unwrap(), Some(42));
    }
}
