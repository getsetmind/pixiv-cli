use pixiv_app::{config::Store, lifecycle::Context};
use pixiv_cli_rs::{
    auth_accounts::{AccountPrompts, AuthCommand},
    update::{PostSuccessFuture, PostSuccessPolicy},
};
use std::{cell::Cell, io::Cursor};

struct Noninteractive;
impl AccountPrompts for Noninteractive {
    fn can_prompt(&self) -> bool {
        false
    }
    fn select(&mut self, _: &str, _: &[String]) -> Result<String, pixiv_cli_rs::CommandError> {
        unreachable!("no synthetic selection")
    }
    fn confirm(&mut self, _: &str, _: bool) -> Result<bool, pixiv_cli_rs::CommandError> {
        unreachable!("no synthetic confirmation")
    }
}
#[cfg(target_os = "linux")]
fn open_owned_database(directory: &std::path::Path) -> bool {
    std::fs::read_dir("/proc/self/fd")
        .unwrap()
        .filter_map(Result::ok)
        .any(|entry| {
            std::fs::read_link(entry.path()).is_ok_and(|path| {
                path.starts_with(directory)
                    && path.extension().is_some_and(|extension| extension == "db")
            })
        })
}

#[tokio::test]
async fn account_success_calls_once_before_owned_database_close_and_errors_do_not_call() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::new(home.path().join("config.toml"));
    let calls = Cell::new(0);
    let callback = |policy: PostSuccessPolicy| -> PostSuccessFuture<'_> {
        assert!(!policy.skip_automatic_update);
        calls.set(calls.get() + 1);
        #[cfg(target_os = "linux")]
        assert!(
            open_owned_database(home.path()),
            "Go root after-success precedes database close"
        );
        Box::pin(async {})
    };
    let command =
        AuthCommand::parse(&["list".into()], &mut Cursor::new(Vec::new()), false).unwrap();
    command
        .execute_with_prompts_and_post_success(
            &store,
            &Context::new(),
            &mut Vec::new(),
            &mut Noninteractive,
            Some(&callback),
        )
        .await
        .unwrap();
    assert_eq!(calls.get(), 1);
    #[cfg(target_os = "linux")]
    assert!(!open_owned_database(home.path()));
    let command = AuthCommand::parse(
        &["use".into(), "999".into()],
        &mut Cursor::new(Vec::new()),
        false,
    )
    .unwrap();
    assert!(
        command
            .execute_with_prompts_and_post_success(
                &store,
                &Context::new(),
                &mut Vec::new(),
                &mut Noninteractive,
                Some(&callback)
            )
            .await
            .is_err()
    );
    assert_eq!(calls.get(), 1);
}

#[tokio::test]
async fn resolved_bundle_import_and_export_carry_skip_policy_while_database_is_owned() {
    use pixiv_cli_rs::auth_transfer::TransferCommand;
    let home = tempfile::tempdir().unwrap();
    let store = Store::new(home.path().join("config.toml"));
    let calls = Cell::new(0);
    let callback = |policy: PostSuccessPolicy| -> PostSuccessFuture<'_> {
        assert!(policy.skip_automatic_update);
        calls.set(calls.get() + 1);
        #[cfg(target_os = "linux")]
        assert!(open_owned_database(home.path()));
        Box::pin(async {})
    };
    let bundle = br#"{"schema":"pixiv-cli.auth-export","version":1,"default_user_id":42,"accounts":[{"user_id":42,"username":"owned","refresh_token":"synthetic-refresh"}]}"#;
    let import =
        TransferCommand::parse(&["import".into(), "--json".into()], &mut &bundle[..], false)
            .unwrap();
    import
        .execute_offline_with_post_success(&store, &mut Vec::new(), Some(&callback))
        .await
        .unwrap();
    assert_eq!(calls.get(), 1);
    #[cfg(target_os = "linux")]
    assert!(!open_owned_database(home.path()));
    let export =
        TransferCommand::parse(&["export".into(), "42".into()], &mut &b""[..], false).unwrap();
    export
        .execute_offline_with_post_success(&store, &mut Vec::new(), Some(&callback))
        .await
        .unwrap();
    assert_eq!(calls.get(), 2);
    let invalid =
        TransferCommand::parse(&["export".into(), "999".into()], &mut &b""[..], false).unwrap();
    assert!(
        invalid
            .execute_offline_with_post_success(&store, &mut Vec::new(), Some(&callback))
            .await
            .is_err()
    );
    assert_eq!(calls.get(), 2);
    #[cfg(target_os = "linux")]
    assert!(!open_owned_database(home.path()));
}

#[path = "support/auth_login.rs"]
mod login_support;

#[tokio::test]
async fn login_success_runs_after_owned_callback_cleanup_and_before_database_close() {
    use pixiv_cli_rs::auth_login::LoginCommand;
    use std::{sync::Arc, time::Duration};
    for status in [200, 400] {
        let home = tempfile::tempdir().unwrap();
        let store = Store::new(home.path().join("config.toml"));
        let hooks = login_support::Hooks::new(true);
        let calls = Cell::new(0);
        let drops = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let callback = |policy: PostSuccessPolicy| -> PostSuccessFuture<'_> {
            assert!(!policy.skip_automatic_update);
            assert_eq!(drops.load(std::sync::atomic::Ordering::SeqCst), 1);
            calls.set(calls.get() + 1);
            #[cfg(target_os = "linux")]
            assert!(open_owned_database(home.path()));
            assert_eq!(hooks.observed.lock().unwrap().final_status, Some(200));
            Box::pin(async {})
        };
        let command = LoginCommand::parse(&["login".into()]).unwrap();
        let mut transport = Some(DroppedTransport {
            inner: login_support::OAuth {
                observed: hooks.observed.clone(),
                status,
                delay: Duration::ZERO,
            },
            drops: drops.clone(),
        });
        let result = tokio::time::timeout(
            Duration::from_secs(15),
            command.execute_with_factory_and_post_success(
                &store,
                &Context::new(),
                &mut Vec::new(),
                Arc::new(hooks.clone()),
                |_| Ok(transport.take().unwrap()),
                Some(&callback),
            ),
        )
        .await
        .unwrap();
        hooks.join_clients();
        assert_eq!(result.is_ok(), status == 200);
        assert_eq!(calls.get(), usize::from(status == 200));
        assert_eq!(drops.load(std::sync::atomic::Ordering::SeqCst), 1);
        #[cfg(target_os = "linux")]
        assert!(!open_owned_database(home.path()));
    }
}

#[derive(Clone)]
struct OwnedOAuth;
impl pixiv_sdk::transport::Transport for OwnedOAuth {
    async fn send(
        &self,
        request: pixiv_sdk::transport::Request,
    ) -> pixiv_sdk::Result<pixiv_sdk::transport::Response> {
        let body = match request.url.as_str() {
            "https://oauth.secure.pixiv.net/auth/token" => {
                serde_json::json!({"access_token":"synthetic-access","refresh_token":"synthetic-rotated","expires_in":3600,"user":{"id":42,"name":"owned"}})
            }
            "https://app-api.pixiv.net/v1/user/detail" => {
                serde_json::json!({"user":{"id":42,"name":"owned"},"profile":{"is_premium":true},"profile_publicity":{},"workspace":{}})
            }
            _ => panic!("no external transport route"),
        };
        Ok(pixiv_sdk::transport::Response {
            status: 200,
            retry_after: None,
            body,
        })
    }
}

#[tokio::test]
async fn token_import_and_validation_keep_actual_database_until_success_callback_returns() {
    use pixiv_app::database::{Database, PixivAccount};
    use pixiv_cli_rs::{auth_transfer::TransferCommand, auth_validation::ValidationCommand};
    let home = tempfile::tempdir().unwrap();
    let store = Store::new(home.path().join("config.toml"));
    store.ensure_defaults().unwrap();
    let calls = Cell::new(0);
    let callback = |policy: PostSuccessPolicy| -> PostSuccessFuture<'_> {
        assert!(!policy.skip_automatic_update);
        calls.set(calls.get() + 1);
        #[cfg(target_os = "linux")]
        assert!(open_owned_database(home.path()));
        Box::pin(async {})
    };
    let import = TransferCommand::parse(
        &["import".into(), "synthetic-token".into()],
        &mut &b""[..],
        false,
    )
    .unwrap();
    import
        .execute_with_transport_and_post_success(
            &store,
            &Context::new(),
            &mut Vec::new(),
            &mut Noninteractive,
            OwnedOAuth,
            Some(&callback),
        )
        .await
        .unwrap();
    assert_eq!(calls.get(), 1);
    Database::open(home.path())
        .unwrap()
        .save_pixiv_credential(&PixivAccount::new(42, "owned", b"synthetic-refresh"))
        .unwrap();
    let validation =
        ValidationCommand::parse(&["check".into(), "42".into()], &mut &b""[..], false).unwrap();
    validation
        .execute_with_factory_and_post_success(
            &store,
            &Context::new(),
            &mut Vec::new(),
            |_| Ok(OwnedOAuth),
            Some(&callback),
        )
        .await
        .unwrap();
    assert_eq!(calls.get(), 2);
    #[cfg(target_os = "linux")]
    assert!(!open_owned_database(home.path()));
}

struct DroppedTransport<T> {
    inner: T,
    drops: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}
impl<T> Drop for DroppedTransport<T> {
    fn drop(&mut self) {
        self.drops.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}
impl<T: pixiv_sdk::transport::Transport> pixiv_sdk::transport::Transport for DroppedTransport<T> {
    async fn send(
        &self,
        request: pixiv_sdk::transport::Request,
    ) -> pixiv_sdk::Result<pixiv_sdk::transport::Response> {
        self.inner.send(request).await
    }
}

#[tokio::test]
async fn token_and_validation_per_call_transport_closes_before_the_hook_and_database_after() {
    use pixiv_cli_rs::{auth_transfer::TransferCommand, auth_validation::ValidationCommand};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    for validate in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let store = Store::new(home.path().join("config.toml"));
        store.ensure_defaults().unwrap();
        let drops = Arc::new(AtomicUsize::new(0));
        let callback = |_: PostSuccessPolicy| -> PostSuccessFuture<'_> {
            Box::pin(async {
                tokio::task::yield_now().await;
                assert_eq!(
                    drops.load(Ordering::SeqCst),
                    1,
                    "frozen Go account methods defer per-call CloseIdleConnections before returning to PersistentPostRun"
                );
            })
        };
        if validate {
            pixiv_app::database::Database::open(home.path())
                .unwrap()
                .save_pixiv_credential(&pixiv_app::database::PixivAccount::new(
                    42,
                    "owned",
                    b"synthetic-refresh",
                ))
                .unwrap();
            let command =
                ValidationCommand::parse(&["check".into(), "42".into()], &mut &b""[..], false)
                    .unwrap();
            command
                .execute_with_factory_and_post_success(
                    &store,
                    &Context::new(),
                    &mut Vec::new(),
                    |_| {
                        Ok(DroppedTransport {
                            inner: OwnedOAuth,
                            drops: drops.clone(),
                        })
                    },
                    Some(&callback),
                )
                .await
                .unwrap();
        } else {
            let command = TransferCommand::parse(
                &["import".into(), "synthetic-token".into()],
                &mut &b""[..],
                false,
            )
            .unwrap();
            command
                .execute_with_transport_and_post_success(
                    &store,
                    &Context::new(),
                    &mut Vec::new(),
                    &mut Noninteractive,
                    DroppedTransport {
                        inner: OwnedOAuth,
                        drops: drops.clone(),
                    },
                    Some(&callback),
                )
                .await
                .unwrap();
        }
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }
}
