use pixiv_app::config::{RuntimeConfig, Scalar, Snapshot, Store};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn optional_string(value: &Option<String>) -> Value {
    let mut object = json!({"present": value.is_some()});
    if let Some(value) = value.as_ref().filter(|value| !value.is_empty()) {
        object["value"] = json!(value);
    }
    object
}

fn solver(value: &Option<pixiv_app::config::FlareSolverrConfig>) -> Value {
    match value {
        None => Value::Null,
        Some(value) => {
            let mut object = json!({"url": value.url});
            if !value.proxy_url.is_empty() {
                object["proxy_url"] = json!(value.proxy_url);
            }
            object
        }
    }
}

fn runtime(value: RuntimeConfig) -> Value {
    json!({
        "DownloadPath": value.download_path,
        "FilenameTemplate": value.filename_template,
        "DirectoryTemplate": value.directory_template,
        "HTTPSProxy": value.https_proxy,
        "RequestInterval": value.request_interval.as_nanos() as u64,
        "LogLevel": value.log_level,
        "LogFormat": value.log_format,
        "PixivNetwork": {"proxy_url": optional_string(&value.pixiv_network.proxy_url)},
        "FanboxNetwork": {"proxy_url": optional_string(&value.fanbox_network.proxy_url), "user_agent": optional_string(&value.fanbox_network.user_agent)},
        "ReverseSearchNetwork": {"proxy_url": optional_string(&value.reverse_search_network.proxy_url), "user_agent": optional_string(&value.reverse_search_network.user_agent)},
        "FanboxFlareSolverr": solver(&value.fanbox_flaresolverr),
        "ReverseSearchFlareSolverr": solver(&value.reverse_search_flaresolverr),
        "OutputJSON": value.output_json,
        "UpdateCheckEnabled": value.update_check_enabled,
        "LoginOpenBrowser": value.login_open_browser,
        "LoginUseAfterLogin": value.login_use_after_login,
        "ReverseSearchProvider": value.reverse_search_provider,
        "ReverseSearchPixivOnly": value.reverse_search_pixiv_only,
        "SauceNAOAPIKey": value.saucenao_api_key,
        "LoginRelayPublicURL": value.login_relay_public_url,
        "LoginRelayListenAddr": value.login_relay_listen_addr,
        "LoginRelayTLSCertFile": value.login_relay_tls_cert_file,
        "LoginRelayTLSKeyFile": value.login_relay_tls_key_file,
        "AccountPool": {"Enabled": value.account_pool.enabled, "Strategy": value.account_pool.strategy},
    })
}

fn default_id(value: Result<Option<i64>, pixiv_app::config::ConfigError>) -> Value {
    match value {
        Ok(value) => {
            json!({"id": value.unwrap_or_default(), "present": value.is_some(), "message": ""})
        }
        Err(error) => json!({"id": 0, "present": false, "message": error.to_string()}),
    }
}

#[test]
fn config_snapshot_matches_go_runtime_precedence_and_default_accounts() {
    let rows: Vec<Value> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/config-snapshot.json"
    ))
    .unwrap();
    for row in rows {
        let input = &row["input"];
        let name = input["name"].as_str().unwrap();
        let environment: BTreeMap<String, String> = input["env"]
            .as_object()
            .map(|env| {
                env.iter()
                    .map(|(key, value)| (key.clone(), value.as_str().unwrap().into()))
                    .collect()
            })
            .unwrap_or_default();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        let body = input["body"].as_str().unwrap();
        let missing = input["missing"].as_bool().unwrap();
        if !missing {
            std::fs::write(&path, body).unwrap();
        }
        let store = Store::new(&path);
        let snapshot = store.current_with_environment(environment.clone());
        let mut actual = json!({
            "input": input, "load_message": "", "runtime": null,
            "runtime_message": "", "runtime_removed": false, "values": null,
            "pixiv": default_id(store.read_pixiv_default_user_id()),
            "fanbox": default_id(store.read_fanbox_default_user_id()),
        });
        match snapshot {
            Err(error) => actual["load_message"] = json!(error.to_string()),
            Ok(snapshot) => {
                match snapshot.runtime() {
                    Ok(value) => actual["runtime"] = runtime(value),
                    Err(error) => {
                        actual["runtime_message"] = json!(error.to_string());
                        actual["runtime_removed"] = json!(error.is_removed());
                    }
                }
                if let Some(aliases) = input["aliases"].as_array() {
                    let values: Vec<Value> = aliases.iter().map(|alias| {
                        let alias = alias.as_str().unwrap();
                        match snapshot.effective(alias) {
                            Ok(value) => {
                                let raw = match &value.value {
                                    None => Value::Null,
                                    Some(Scalar::String(value)) => json!(value),
                                    Some(Scalar::Bool(value)) => json!(value),
                                    Some(Scalar::Duration(value)) => json!(value),
                                };
                                json!({"alias": alias, "value": {"Value": raw, "Text": value.text, "Source": value.source, "HasValue": value.value.is_some()}, "message": "", "removed": false})
                            }
                            Err(error) => json!({"alias": alias, "value": {"Value": null, "Text": "", "Source": "", "HasValue": false}, "message": error.to_string(), "removed": error.is_removed()}),
                        }
                    }).collect();
                    actual["values"] = json!(values);
                }
            }
        }
        assert_eq!(actual, row, "{name}");
        if missing {
            assert!(!path.exists(), "{name}: read created a config file");
        } else {
            assert_eq!(
                std::fs::read(&path).unwrap(),
                body.as_bytes(),
                "{name}: read changed the file"
            );
        }
    }
}

#[test]
fn captured_configuration_retains_file_and_environment_while_new_reads_observe_changes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    std::fs::write(
        &path,
        "[logging]\nlevel = 'debug'\n[pixiv.auth]\ndefault_user_id = 42\n",
    )
    .unwrap();
    let store = Store::new(&path);
    let first = store
        .current_with_environment(BTreeMap::from([(
            "DOWNLOAD_PATH".into(),
            "first-env".into(),
        )]))
        .unwrap();
    std::fs::write(
        &path,
        "[logging]\nlevel = 'info'\n[pixiv.auth]\ndefault_user_id = 43\n",
    )
    .unwrap();
    let second = store
        .current_with_environment(BTreeMap::from([(
            "DOWNLOAD_PATH".into(),
            "second-env".into(),
        )]))
        .unwrap();
    assert_eq!(first.runtime().unwrap().download_path, "first-env");
    assert_eq!(first.runtime().unwrap().log_level, "debug");
    assert_eq!(first.pixiv_default_user_id().unwrap(), Some(42));
    assert_eq!(second.runtime().unwrap().download_path, "second-env");
    assert_eq!(second.runtime().unwrap().log_level, "info");
    assert_eq!(store.read_pixiv_default_user_id().unwrap(), Some(43));
    let captured = Snapshot::parse("", BTreeMap::new()).unwrap();
    assert_eq!(
        captured.effective("download_path").unwrap().source,
        "default"
    );
}

#[tokio::test]
async fn configuration_failures_stop_client_acquisition_and_retain_the_typed_cause() {
    use pixiv_app::{
        account_service::AccountService,
        database::Database,
        facade::{Facade, UseOutcome},
        gate::Gate,
        lifecycle::Context,
        sessions::ClientSessions,
    };
    use std::{error::Error, sync::Arc};

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    std::fs::write(
        &path,
        "[pixiv.auth]\ndefault_user_id = 0\n[account_pool]\nenabled = 'true'\n",
    )
    .unwrap();
    let config = Store::new(&path);
    let default_config = config.clone();
    let service = AccountService {
        repository: Arc::new(std::sync::Mutex::new(
            Database::open(directory.path()).unwrap(),
        )),
        defaults: Some(Arc::new(move || {
            default_config
                .read_pixiv_default_user_id()
                .map_err(Into::into)
        })),
    };
    let error = service.selected_user_id(&Context::new()).unwrap_err();
    assert_eq!(
        error.to_string(),
        "config: pixiv.auth.default_user_id must be a positive integer"
    );
    assert!(
        error
            .source()
            .unwrap()
            .is::<pixiv_app::config::ConfigError>()
    );
    let facade = Facade {
        sessions: Arc::new(ClientSessions::<i64, ()> {
            accounts: Some(Arc::new(|_, _, _| {
                panic!("configuration failure still opened an account")
            })),
            gate: Some(Gate::new()),
            close_client: Arc::new(|_| panic!("configuration failure still closed a client")),
        }),
        load_pool_config: Some(Arc::new(move || {
            Ok(config.current()?.runtime()?.account_pool)
        })),
        pool_factory: Some(Arc::new(|_| {
            panic!("invalid configuration still created a pool")
        })),
    };
    let error = facade
        .use_client(
            Some(&Context::new()),
            0,
            (),
            Some(Arc::new(|_, _| {
                Box::pin(async {
                    UseOutcome {
                        committed: false,
                        error: None,
                    }
                })
            })),
        )
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "account_pool.enabled must be a boolean");
    assert!(
        error
            .source()
            .unwrap()
            .is::<pixiv_app::config::ConfigError>()
    );
    std::fs::remove_file(path).unwrap();
    let cause = Store::new(directory.path()).current().err().unwrap();
    assert!(cause.source().unwrap().is::<std::io::Error>());
}
