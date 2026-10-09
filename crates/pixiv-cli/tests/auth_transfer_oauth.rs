use pixiv_app::{
    config::Store,
    database::{Database, PixivAccount},
    lifecycle::Context,
};
use pixiv_cli_rs::{
    CommandError,
    auth_accounts::{AccountPrompts, AuthCommand},
    auth_transfer::TransferCommand,
};
use pixiv_sdk::transport::{Request, Response, Transport};
use std::sync::{Arc, Mutex};
struct Prompts;
impl AccountPrompts for Prompts {
    fn can_prompt(&self) -> bool {
        false
    }
    fn select(&mut self, _: &str, _: &[String]) -> Result<String, CommandError> {
        panic!("token import does not select accounts")
    }
    fn confirm(&mut self, _: &str, _: bool) -> Result<bool, CommandError> {
        panic!("token import does not confirm account selection")
    }
}
struct OAuth {
    requests: Arc<Mutex<usize>>,
}
impl Transport for OAuth {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        *self.requests.lock().unwrap() += 1;
        assert_eq!(request.method.as_str(), "POST");
        assert_eq!(request.url, "https://oauth.secure.pixiv.net/auth/token");
        assert_eq!(request.parameters.len(), 5);
        let field = |name: &str| {
            request
                .parameters
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
        };
        assert_eq!(field("refresh_token"), Some("synthetic-token"));
        assert_eq!(field("grant_type"), Some("refresh_token"));
        assert_eq!(field("include_policy"), Some("true"));
        assert!(
            request
                .headers
                .iter()
                .all(|(name, _)| !name.eq_ignore_ascii_case("authorization"))
        );
        Ok(Response {
            status: 200,
            retry_after: None,
            body: serde_json::json!({"access_token":"synthetic-access","refresh_token":"synthetic-rotated","expires_in":3600,"user":{"id":42,"name":"fixture-name"}}),
        })
    }
}
#[tokio::test]
async fn token_import_matches_go_oauth_argv_stdin_status_output_and_rotated_storage() {
    for existing in [false, true] {
        for stdin in [false, true] {
            for json in [false, true] {
                let home = tempfile::tempdir().unwrap();
                let store = Store::new(home.path().join("config.toml"));
                if existing {
                    let mut db = Database::open(home.path()).unwrap();
                    db.save_pixiv_credential(&PixivAccount::new(42, "old", b"synthetic-old"))
                        .unwrap();
                }
                let mut args = vec!["import".to_owned()];
                if !stdin {
                    args.push("  synthetic-token  ".into());
                }
                if json {
                    args.push("--json".into());
                }
                let AuthCommand::Transfer(command) =
                    AuthCommand::parse(&args, &mut &b"  synthetic-token  \r\n"[..], false).unwrap()
                else {
                    panic!("transfer dispatch")
                };
                let requests = Arc::new(Mutex::new(0));
                let mut output = Vec::new();
                command
                    .execute_with_transport(
                        &store,
                        &Context::new(),
                        &mut output,
                        &mut Prompts,
                        OAuth {
                            requests: requests.clone(),
                        },
                    )
                    .await
                    .unwrap();
                let status = if existing { "updated" } else { "added" };
                let want = if json {
                    format!(
                        "{{\n  \"user_id\": 42,\n  \"username\": \"fixture-name\",\n  \"status\": \"{status}\"\n}}\n"
                    )
                } else {
                    format!("{status} uid:42\nusername:fixture-name\n")
                };
                assert_eq!(output, want.as_bytes());
                assert_eq!(*requests.lock().unwrap(), 1);
                let account = Database::open(home.path()).unwrap().get_pixiv(42).unwrap();
                assert_eq!(account.refresh_token_copy(), b"synthetic-rotated");
                assert_eq!(account.credential_revision, if existing { 2 } else { 1 });
                assert_eq!(store.read_pixiv_default_user_id().unwrap(), Some(42));
            }
        }
    }
}
#[test]
fn valid_json_huge_numbers_reach_bundle_decoder_after_startup() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::new(home.path().join("config.toml"));
    let command =
        TransferCommand::parse(&["import".into()], &mut &b"{\"version\":1e999}"[..], false)
            .unwrap();
    assert!(command.execute_offline(&store, &mut Vec::new()).is_err());
    assert!(store.path().exists());
    assert!(home.path().join("pixiv-cli.db").exists());
}

#[test]
fn json_classifier_preserves_go_string_validity_and_proxy_precedence() {
    for mut body in [
        b"{\"name\":\"\xff\xff\"}".as_slice(),
        br#"{"name":"\ud800"}"#.as_slice(),
    ] {
        let error =
            TransferCommand::parse(&["import".into(), "--proxy=x".into()], &mut body, false)
                .err()
                .unwrap();
        assert_eq!(
            error.to_string(),
            "bundle import cannot be combined with --proxy or --no-proxy"
        );
    }
}

#[tokio::test]
async fn no_proxy_false_preserves_configured_proxy_validation() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::new(home.path().join("config.toml"));
    std::fs::write(store.path(), "[pixiv.network]\nproxy_url='invalid'\n").unwrap();
    let args = ["import", "synthetic-token", "--no-proxy=false"].map(str::to_owned);
    let AuthCommand::Transfer(command) = AuthCommand::parse(&args, &mut &b""[..], false).unwrap()
    else {
        panic!("transfer dispatch")
    };
    let requests = Arc::new(Mutex::new(0));
    let error = command
        .execute_with_transport(
            &store,
            &Context::new(),
            &mut Vec::new(),
            &mut Prompts,
            OAuth {
                requests: requests.clone(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "proxy URL must use http, https, socks5, or socks5h: invalid proxy configuration"
    );
    assert_eq!(*requests.lock().unwrap(), 0);
}
