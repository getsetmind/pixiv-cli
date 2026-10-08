use pixiv_app::{config::Snapshot, connection::CommandConnection};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[test]
fn command_connection_matches_go_proxy_presence_errors_and_pacing() {
    let rows: Vec<Value> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/connection-options.json"
    ))
    .unwrap();
    for row in rows {
        let input = &row["input"];
        let mut runtime = Snapshot::parse("", BTreeMap::new())
            .unwrap()
            .runtime()
            .unwrap();
        runtime.https_proxy = input["global"].as_str().unwrap().into();
        runtime.pixiv_network.proxy_url = input["service"].as_str().map(str::to_owned);
        runtime.request_interval =
            std::time::Duration::from_nanos(input["interval"].as_u64().unwrap());
        let selected = CommandConnection::resolve(&runtime, input["override"].as_str());
        let actual = match selected {
            Ok(connection) => {
                connection.open_transport().unwrap();
                json!({"message": "", "invalid_proxy": false, "proxy": connection.proxy(), "pacing": connection.pacing().as_nanos() as u64, "http_client": true})
            }
            Err(error) => {
                json!({"message": error.to_string(), "invalid_proxy": true, "proxy": "", "pacing": 0, "http_client": false})
            }
        };
        let expected = json!({"message": row["message"], "invalid_proxy": row["invalid_proxy"], "proxy": row["proxy"], "pacing": row["pacing"], "http_client": row["http_client"]});
        assert_eq!(actual, expected, "{}", input["name"]);
    }
}

#[test]
fn rejected_proxy_never_exposes_credentials_or_other_url_components() {
    let mut runtime = Snapshot::parse("", BTreeMap::new())
        .unwrap()
        .runtime()
        .unwrap();
    runtime.https_proxy = "http://synthetic-user:synthetic-password@synthetic-host.invalid/synthetic-path-%zz?synthetic-key=value".into();
    let error = CommandConnection::resolve(&runtime, None).err().unwrap();
    assert_eq!(
        error.to_string(),
        "parse proxy URL: invalid proxy configuration"
    );
    assert_eq!(format!("{error:?}"), error.to_string());
    assert!(std::error::Error::source(&error).is_none());
}
