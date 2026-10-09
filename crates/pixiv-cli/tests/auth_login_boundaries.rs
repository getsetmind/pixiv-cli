use pixiv_app::{config::Store, lifecycle::Context, login_bridge::LoginBridgeHooks};
use pixiv_cli_rs::{CommandError, auth_login::LoginCommand};
use pixiv_sdk::transport::{Request, Response, Transport};
use std::sync::{Arc, Mutex};

struct NoHooks;
impl LoginBridgeHooks for NoHooks {
    fn diagnostic(&self, _: &str) {
        panic!("validation must precede login listener")
    }
    fn open_browser(&self, _: &str) -> Result<(), pixiv_app::login_bridge::BridgeError> {
        panic!("validation must precede browser")
    }
}
struct NoOAuth;
impl Transport for NoOAuth {
    async fn send(&self, _: Request) -> pixiv_sdk::Result<Response> {
        panic!("validation must precede OAuth")
    }
}
fn command(args: &[&str]) -> Result<LoginCommand, CommandError> {
    LoginCommand::parse(&args.iter().map(|value| (*value).into()).collect::<Vec<_>>())
}

#[test]
fn login_parser_keeps_presence_of_explicit_false_and_empty_overrides() {
    let parsed = command(&[
        "login",
        "--no-open=false",
        "--use=false",
        "--proxy=",
        "--relay-public-url=",
        "--relay-listen-addr=",
        "--timeout=-1s",
    ])
    .unwrap();
    assert!(parsed.requires_config());
    assert!(!parsed.machine_output());
    assert!(command(&["login", "--json"]).unwrap().machine_output());
    let help = command(&["login", "--help"]).unwrap();
    assert!(!help.requires_config());
    assert!(!help.machine_output());
}

#[test]
fn login_parser_retains_go_flag_and_argument_errors() {
    for (args, expected) in [
        (
            vec!["login", "42"],
            "usage: pixiv auth login [--json] [--no-open] [--addr 127.0.0.1:0] [--use] [--timeout DURATION] [--relay-public-url URL] [--relay-listen-addr ADDR]",
        ),
        (
            vec!["login", "--timeout"],
            "flag needs an argument: --timeout",
        ),
        (
            vec!["login", "--timeout=1"],
            "invalid argument \"1\" for \"--timeout\" flag: time: missing unit in duration \"1\"",
        ),
        (
            vec!["login", "--timeout=1d"],
            "invalid argument \"1d\" for \"--timeout\" flag: time: unknown unit \"d\" in duration \"1d\"",
        ),
        (
            vec!["login", "--no-open=bad"],
            "invalid argument \"bad\" for \"--no-open\" flag: strconv.ParseBool: parsing \"bad\": invalid syntax",
        ),
        (
            vec!["login", "--unrecognized"],
            "unknown option '--unrecognized'",
        ),
        (vec!["login", "--no-input"], "unknown option '--no-input'"),
    ] {
        assert_eq!(
            command(&args).err().unwrap().to_string(),
            expected,
            "{args:?}"
        );
    }
}

#[tokio::test]
async fn login_checks_proxy_conflict_before_runtime_and_local_addr_before_proxy_transport() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::new(home.path().join("config.toml"));
    std::fs::write(store.path(), "[login]\nopen_browser='invalid'\n").unwrap();
    let error = command(&["login", "--proxy=bad", "--no-proxy=false"])
        .unwrap()
        .execute_with_transport(
            &store,
            &Context::new(),
            &mut Vec::new(),
            Arc::new(NoHooks),
            NoOAuth,
        )
        .await
        .err()
        .unwrap();
    assert_eq!(
        error.to_string(),
        "use either --proxy or --no-proxy, not both"
    );
    std::fs::write(store.path(), "").unwrap();
    let proxies = Arc::new(Mutex::new(Vec::new()));
    let record = proxies.clone();
    let error = command(&["login", "--addr=0.0.0.0:0", "--proxy=bad"])
        .unwrap()
        .execute_with_factory(
            &store,
            &Context::new(),
            &mut Vec::new(),
            Arc::new(NoHooks),
            move |proxy| {
                record.lock().unwrap().push(proxy.map(str::to_owned));
                Ok(NoOAuth)
            },
        )
        .await
        .err()
        .unwrap();
    assert_eq!(
        error.to_string(),
        "--addr must bind to a loopback address, got \"0.0.0.0:0\""
    );
    assert!(proxies.lock().unwrap().is_empty());
}

#[tokio::test]
async fn login_help_does_not_create_configuration_database_or_native_hooks() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::new(home.path().join("config.toml"));
    let mut output = Vec::new();
    command(&["login", "--help"])
        .unwrap()
        .execute_with_transport(
            &store,
            &Context::new(),
            &mut output,
            Arc::new(NoHooks),
            NoOAuth,
        )
        .await
        .unwrap();
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/cli_login_flow.json")).unwrap();
    assert_eq!(
        String::from_utf8(output).unwrap(),
        fixture["help"].as_str().unwrap()
    );
    assert!(!store.path().exists());
    assert!(!home.path().join("pixiv-cli.db").exists());
}

#[test]
fn login_error_output_tracks_json_flag_presence_until_first_parse_failure() {
    for (args, machine) in [
        (vec!["login", "--json=false", "--addr=0.0.0.0:1"], true),
        (vec!["login", "--json=false", "extra"], true),
        (vec!["login", "--json", "--timeout=invalid"], true),
        (vec!["login", "--timeout=invalid", "--json"], false),
        (vec!["login", "--json=invalid"], false),
        (vec!["login", "--no-open", "--addr=", "--json=false"], true),
    ] {
        let args = args.iter().map(|value| (*value).into()).collect::<Vec<_>>();
        assert_eq!(LoginCommand::machine_output_requested(&args), machine);
    }
}
