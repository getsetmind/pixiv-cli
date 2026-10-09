#[path = "support/auth_login_contract.rs"]
mod support;
use pixiv_app::{
    config::Store,
    database::{Database, PixivAccount},
    lifecycle::Context,
};
use pixiv_cli_rs::auth_login::LoginCommand;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
    time::Duration,
};
use support::{Hooks, OAuth};

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/cli_login_flow.json")).unwrap()
}
fn args(case: &Value, replace: &impl Fn(&str) -> String) -> Vec<String> {
    std::iter::once("login".to_owned())
        .chain(
            case["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|arg| replace(arg.as_str().unwrap())),
        )
        .collect()
}

#[tokio::test]
async fn login_validation_matches_go_error_priority_before_listener_or_oauth() {
    let fixture = fixture();
    let mut compared = 0;
    for case in fixture["validation"].as_array().unwrap() {
        // Go dependency callbacks have no injectable equivalent in the Rust composition root.
        if case["service_error"] != "" && case["name"] != "proxy_conflict_precedes_services" {
            continue;
        }
        let home = tempfile::tempdir().unwrap();
        let store = Store::new(home.path().join("config.toml"));
        std::fs::write(store.path(), support::runtime(case, &|value| value.into())).unwrap();
        let hooks = Hooks::new(case.clone(), fixture.clone());
        let error = match LoginCommand::parse(&args(case, &|value| value.into())) {
            Err(error) => error,
            Ok(command) => command
                .execute_with_transport(
                    &store,
                    &Context::new(),
                    &mut Vec::new(),
                    Arc::new(hooks.clone()),
                    OAuth {
                        hooks: hooks.clone(),
                    },
                )
                .await
                .err()
                .unwrap(),
        };
        assert_eq!(
            error.to_string(),
            case["expected"]["error"].as_str().unwrap(),
            "{}",
            case["name"]
        );
        assert!(
            hooks.observed.lock().unwrap().events.is_empty(),
            "{}",
            case["name"]
        );
        assert!(
            hooks.observed.lock().unwrap().diagnostics.is_empty(),
            "{}",
            case["name"]
        );
        compared += 1;
    }
    assert_eq!(
        compared,
        fixture["validation"].as_array().unwrap().len() - 4
    );
}

#[tokio::test]
async fn local_and_remote_login_match_go_commands_outputs_hooks_saved_state_and_final_pages() {
    let fixture = fixture();
    let pages: Value = serde_json::from_str(include_str!(
        "../../pixiv-app/tests/fixtures/login_page.json"
    ))
    .unwrap();
    for case in fixture["flows"].as_array().unwrap() {
        let home = tempfile::tempdir().unwrap();
        let store = Store::new(home.path().join("config.toml"));
        let reserved = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = reserved.local_addr().unwrap().to_string();
        let relay = format!("http://{address}/remote/login");
        let cert = home
            .path()
            .join("missing-certificate.pem")
            .to_string_lossy()
            .into_owned();
        let key = home
            .path()
            .join("missing-key.pem")
            .to_string_lossy()
            .into_owned();
        let replace = |text: &str| {
            text.replace("<ADDR>", &address)
                .replace("<RELAY>", &relay)
                .replace("<CERT>", &cert)
                .replace("<KEY>", &key)
        };
        std::fs::write(store.path(), support::runtime(case, &replace)).unwrap();
        store.ensure_defaults().unwrap();
        store.set_pixiv_default_user_id(7).unwrap();
        Database::open(home.path())
            .unwrap()
            .save_pixiv_credential(&PixivAccount::new(
                7,
                "previous-user",
                b"previous-synthetic-refresh",
            ))
            .unwrap();
        let context = Context::new();
        if case["command_canceled"] == true {
            context.cancel();
        }
        let hooks = Hooks::new(case.clone(), fixture.clone());
        let proxy_observed = Arc::new(Mutex::new(Vec::new()));
        let proxies = proxy_observed.clone();
        let oauth_hooks = hooks.clone();
        let mut output = Vec::new();
        let command = LoginCommand::parse(&args(case, &replace)).unwrap();
        drop(reserved);
        let result = tokio::time::timeout(
            Duration::from_secs(8),
            command.execute_with_factory(
                &store,
                &context,
                &mut output,
                Arc::new(hooks.clone()),
                move |proxy| {
                    proxies.lock().unwrap().push(proxy.map(str::to_owned));
                    Ok(OAuth {
                        hooks: oauth_hooks.clone(),
                    })
                },
            ),
        )
        .await;
        assert!(result.is_ok(), "flow hung: {}", case["name"]);
        let result = result.unwrap();
        assert_eq!(
            result
                .as_ref()
                .err()
                .map(ToString::to_string)
                .unwrap_or_default(),
            case["expected"]["error"].as_str().unwrap(),
            "{}",
            case["name"]
        );
        hooks.join();
        let observation = hooks.observed.lock().unwrap();
        let mut stdout = observation.output.clone().into_bytes();
        stdout.extend_from_slice(&output);
        assert_eq!(
            stdout,
            case["expected"]["stdout"].as_str().unwrap().as_bytes(),
            "{}",
            case["name"]
        );
        assert_eq!(
            support::normalize(&observation, &relay),
            case["expected"]["stderr"].as_str().unwrap(),
            "{}",
            case["name"]
        );
        let events = case["expected"]["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(observation.events, events, "{}", case["name"]);
        assert_eq!(
            store.read_pixiv_default_user_id().unwrap(),
            Some(case["expected"]["default_user_id"].as_i64().unwrap()),
            "{}",
            case["name"]
        );
        let database = Database::open(home.path()).unwrap();
        let stored = database.get_pixiv(42);
        assert_eq!(
            stored.is_ok(),
            case["expected"]["stored"].as_bool().unwrap(),
            "{}",
            case["name"]
        );
        if let Ok(stored) = stored {
            assert_eq!(stored.user_id, 42);
            assert_eq!(stored.username, "synthetic-user");
            assert_eq!(stored.refresh_token_copy(), b"synthetic-refresh-secret");
            assert_eq!(stored.credential_revision, 1);
        }
        let submission = case["submission"].as_str().unwrap();
        if matches!(submission, "callback" | "manual" | "relay") {
            assert_eq!(
                u64::from(observation.final_status),
                case["expected"]["final_status"].as_u64().unwrap(),
                "{}",
                case["name"]
            );
            let kind = if observation.final_status == 200 {
                "success"
            } else {
                "failure"
            };
            let expected_hash = pages
                .as_array()
                .unwrap()
                .iter()
                .find(|page| page["name"] == kind)
                .unwrap()["sha256"]
                .as_str()
                .unwrap();
            assert_eq!(
                format!("{:x}", Sha256::digest(&observation.final_page)),
                expected_hash,
                "{}",
                case["name"]
            );
        } else if submission == "none" {
            assert_eq!(observation.final_status, 0);
        }
        for secret in [
            "synthetic-login-code",
            "synthetic-access-secret",
            "synthetic-refresh-secret",
        ] {
            assert!(!observation.diagnostics.contains(secret));
            assert!(!String::from_utf8_lossy(&stdout).contains(secret));
            assert!(!String::from_utf8_lossy(&observation.final_page).contains(secret));
        }
        assert_eq!(proxy_observed.lock().unwrap().len(), 1);
        let explicit = case["args"]
            .as_array()
            .unwrap()
            .iter()
            .any(|arg| arg == "--no-proxy");
        assert_eq!(
            proxy_observed.lock().unwrap()[0],
            explicit.then(String::new),
            "{}",
            case["name"]
        );
        if !observation.address.is_empty() {
            assert!(
                TcpStream::connect(&observation.address).is_err(),
                "listener still alive: {}",
                case["name"]
            );
        }
    }
}
