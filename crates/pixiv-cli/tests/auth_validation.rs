use pixiv_app::{
    config::Store,
    database::{Database, PixivAccount},
    lifecycle::Context,
};
use pixiv_cli_rs::{auth_accounts::AuthCommand, auth_validation::ValidationCommand};
use pixiv_sdk::transport::{Request, Response, Transport};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
#[derive(Clone)]
struct OAuth {
    directory: PathBuf,
    routes: Arc<Mutex<Vec<String>>>,
    fail_second: bool,
}
impl Transport for OAuth {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        let field = |name: &str| {
            request
                .parameters
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
        };
        let (id, body) = if request.method.as_str() == "POST" {
            assert_eq!(request.url, "https://oauth.secure.pixiv.net/auth/token");
            assert_eq!(request.parameters.len(), 5);
            assert_eq!(field("grant_type"), Some("refresh_token"));
            let id = field("refresh_token")
                .unwrap()
                .strip_prefix("synthetic-")
                .unwrap()
                .parse::<i64>()
                .unwrap();
            self.routes.lock().unwrap().push(format!("oauth:{id}"));
            (
                id,
                serde_json::json!({"access_token":"synthetic-access","refresh_token":format!("synthetic-rotated-{id}"),"user":{"id":id,"name":"oauth-name"}}),
            )
        } else {
            assert_eq!(request.url, "https://app-api.pixiv.net/v1/user/detail");
            let id = field("user_id").unwrap().parse::<i64>().unwrap();
            self.routes.lock().unwrap().push(format!("profile:{id}"));
            let db = Database::open(&self.directory).unwrap();
            let stored = db.get_pixiv(id).unwrap();
            assert_eq!(stored.credential_revision, 2);
            assert_eq!(
                String::from_utf8(stored.refresh_token_copy()).unwrap(),
                format!("synthetic-rotated-{id}")
            );
            if self.fail_second && id == 1 {
                return Err(pixiv_sdk::Error::new(
                    pixiv_sdk::Reason::UpstreamUnavailable,
                    "CurrentUser",
                ));
            }
            (
                id,
                serde_json::json!({"user":{"id":id,"name":"profile-name"},"profile":{"is_premium":true},"profile_publicity":{},"workspace":{}}),
            )
        };
        assert!(id > 0);
        Ok(Response {
            status: 200,
            retry_after: None,
            body,
        })
    }
}
fn command(args: &[&str], input: &[u8]) -> ValidationCommand {
    let args = args.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let AuthCommand::Validation(command) =
        AuthCommand::parse(&args, &mut &input[..], false).unwrap()
    else {
        panic!("validation dispatch")
    };
    command
}
fn seed(store: &Store, ids: &[i64]) {
    store.ensure_defaults().unwrap();
    store.set_pixiv_default_user_id(ids[0]).unwrap();
    let mut db = Database::open(store.path().parent().unwrap()).unwrap();
    for id in ids {
        db.save_pixiv_credential(&PixivAccount::new(
            *id,
            "local-name",
            format!("synthetic-{id}").as_bytes(),
        ))
        .unwrap();
    }
}
#[tokio::test]
async fn check_and_refresh_match_go_current_explicit_stdin_json_and_storage() {
    for refresh in [false, true] {
        for selection in [0, 1, 2] {
            for json in [false, true] {
                let dir = tempfile::tempdir().unwrap();
                let store = Store::new(dir.path().join("config.toml"));
                seed(&store, &[42]);
                let mut args = vec![if refresh { "refresh" } else { "check" }];
                if selection == 1 {
                    args.push("42");
                }
                if json {
                    args.push("--json");
                }
                let routes = Arc::new(Mutex::new(Vec::new()));
                let mut out = Vec::new();
                command(&args, if selection == 2 { b"42\r\n" } else { b"" })
                    .execute_with_transport(
                        &store,
                        &Context::new(),
                        &mut out,
                        OAuth {
                            directory: dir.path().into(),
                            routes: routes.clone(),
                            fail_second: false,
                        },
                    )
                    .await
                    .unwrap();
                let want = if refresh {
                    if json {
                        "{\n  \"accounts\": [\n    {\n      \"user_id\": 42,\n      \"username\": \"local-name\",\n      \"default\": true,\n      \"has_token\": true,\n      \"premium_status\": true,\n      \"schedulable\": true,\n      \"eligible\": true\n    }\n  ]\n}\n"
                    } else {
                        "✓ refreshed uid:42 premium:yes\n"
                    }
                } else if json {
                    "{\n  \"user_id\": 42,\n  \"username\": \"oauth-name\",\n  \"default\": false,\n  \"has_token\": true\n}\n"
                } else if selection == 0 {
                    "token ok, uid:42\nusername:oauth-name\n"
                } else {
                    "account uid:42 ok\nusername:oauth-name\n"
                };
                assert_eq!(String::from_utf8(out).unwrap(), want);
                assert_eq!(
                    *routes.lock().unwrap(),
                    if refresh {
                        vec!["oauth:42", "profile:42"]
                    } else {
                        vec!["oauth:42"]
                    }
                );
                let db = Database::open(dir.path()).unwrap();
                let a = db.get_pixiv(42).unwrap();
                assert_eq!(a.credential_revision, 2);
                assert_eq!(a.username, "local-name");
                assert_eq!(
                    String::from_utf8(a.refresh_token_copy()).unwrap(),
                    "synthetic-rotated-42"
                );
            }
        }
    }
}
#[tokio::test]
async fn refresh_all_keeps_partial_commits_and_suppresses_output_on_failure() {
    for fail in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("config.toml"));
        seed(&store, &[2, 1]);
        let routes = Arc::new(Mutex::new(Vec::new()));
        let mut out = Vec::new();
        let result = command(&["refresh", "--all"], b"ignored stdin")
            .execute_with_transport(
                &store,
                &Context::new(),
                &mut out,
                OAuth {
                    directory: dir.path().into(),
                    routes: routes.clone(),
                    fail_second: fail,
                },
            )
            .await;
        assert_eq!(result.is_err(), fail);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            if fail {
                ""
            } else {
                "✓ refreshed uid:2 premium:yes\n✓ refreshed uid:1 premium:yes\n"
            }
        );
        assert_eq!(
            *routes.lock().unwrap(),
            vec!["oauth:2", "profile:2", "oauth:1", "profile:1"]
        );
        let db = Database::open(dir.path()).unwrap();
        for id in [1, 2] {
            let a = db.get_pixiv(id).unwrap();
            assert_eq!(a.credential_revision, 2);
            assert_eq!(a.premium_status.is_some(), !fail || id == 2);
        }
    }
}
#[tokio::test]
async fn all_uid_precedence_empty_pool_and_proxy_errors_match_go() {
    for (args, want) in [
        (
            vec!["refresh", "bad", "--all"],
            "--all cannot be combined with a UID",
        ),
        (vec!["refresh", "--all"], "no accounts"),
        (
            vec!["check", "42", "--proxy=x", "--no-proxy=false"],
            "use either --proxy or --no-proxy, not both",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("config.toml"));
        let error = command(&args, b"")
            .execute_with_factory(
                &store,
                &Context::new(),
                &mut Vec::new(),
                |_| -> Result<OAuth, pixiv_cli_rs::CommandError> {
                    panic!("invalid selection does not send OAuth")
                },
            )
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), want);
    }
}

#[tokio::test]
async fn proxy_override_presence_and_false_preserve_go_connection_selection() {
    for (flag, expected) in [
        ("--no-proxy=false", "http://synthetic-proxy:8080"),
        ("--no-proxy", ""),
        (
            "--proxy=http://command-proxy:8080",
            "http://command-proxy:8080",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("config.toml"));
        std::fs::write(
            store.path(),
            "[pixiv.network]\nproxy_url='http://synthetic-proxy:8080'\n",
        )
        .unwrap();
        seed(&store, &[42]);
        let transport = OAuth {
            directory: dir.path().into(),
            routes: Arc::new(Mutex::new(Vec::new())),
            fail_second: false,
        };
        command(&["check", "42", flag], b"")
            .execute_with_factory(&store, &Context::new(), &mut Vec::new(), |proxy| {
                assert_eq!(proxy, expected);
                Ok(transport.clone())
            })
            .await
            .unwrap();
    }
}
#[test]
fn machine_output_flag_scanning_preserves_proxy_values_and_double_dash() {
    for (args, expected) in [
        (vec!["refresh", "--proxy", "--all", "--json"], true),
        (vec!["refresh", "--all", "--json"], true),
        (vec!["refresh", "--all=bad", "--json"], false),
        (vec!["refresh", "--", "--all", "--json"], false),
        (vec!["check", "--all", "--json"], false),
    ] {
        assert_eq!(
            pixiv_cli_rs::auth_accounts::machine_output_requested(
                &args.into_iter().map(str::to_owned).collect::<Vec<_>>()
            ),
            expected
        );
    }
}

#[tokio::test]
async fn json_time_serialization_failure_happens_after_all_refresh_commits() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("config.toml"));
    store.ensure_defaults().unwrap();
    store.set_pixiv_default_user_id(2).unwrap();
    {
        let mut db = Database::open(dir.path()).unwrap();
        for id in [2, 1] {
            let mut a = PixivAccount::new(id, "local", format!("synthetic-{id}").as_bytes());
            if id == 2 {
                a.pool_frozen_until = Some(253402300800);
            }
            db.save_pixiv_credential(&a).unwrap();
        }
    }
    let routes = Arc::new(Mutex::new(Vec::new()));
    let mut out = Vec::new();
    let error = command(&["refresh", "--all", "--json"], b"")
        .execute_with_transport(
            &store,
            &Context::new(),
            &mut out,
            OAuth {
                directory: dir.path().into(),
                routes: routes.clone(),
                fail_second: false,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "json: error calling MarshalJSON for type *time.Time: year outside of range [0,9999]"
    );
    assert!(out.is_empty());
    assert_eq!(
        *routes.lock().unwrap(),
        vec!["oauth:2", "profile:2", "oauth:1", "profile:1"]
    );
    let db = Database::open(dir.path()).unwrap();
    for id in [1, 2] {
        let a = db.get_pixiv(id).unwrap();
        assert_eq!(a.credential_revision, 2);
        assert_eq!(a.premium_status, Some(true));
    }
}

#[tokio::test]
async fn explicit_proxy_skips_per_account_runtime_reload_after_startup_validation() {
    for explicit in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("config.toml"));
        seed(&store, &[2, 1]);
        let transport = OAuth {
            directory: dir.path().into(),
            routes: Arc::new(Mutex::new(Vec::new())),
            fail_second: false,
        };
        let mut args = vec!["refresh", "--all"];
        if explicit {
            args.push("--no-proxy");
        }
        let mut calls = 0;
        let result = command(&args, b"")
            .execute_with_factory(&store, &Context::new(), &mut Vec::new(), |_| {
                calls += 1;
                if calls == 1 {
                    std::fs::write(
                        store.path(),
                        "[network]\nrequest_interval='invalid'\n[pixiv.auth]\ndefault_user_id=2\n",
                    )
                    .unwrap();
                }
                Ok(transport.clone())
            })
            .await;
        assert_eq!(result.is_ok(), explicit);
        assert_eq!(calls, if explicit { 2 } else { 1 });
        let db = Database::open(dir.path()).unwrap();
        assert_eq!(db.get_pixiv(2).unwrap().credential_revision, 2);
        assert_eq!(
            db.get_pixiv(1).unwrap().credential_revision,
            if explicit { 2 } else { 1 }
        );
    }
}
