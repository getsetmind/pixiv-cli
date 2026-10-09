use pixiv_app::config::{Store, cli_setting_aliases, public_setting_text, valid_setting_aliases};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[test]
fn config_store_mutations_match_frozen_go_sparse_document_and_diagnostics() {
    let rows: Vec<Value> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/config-mutations.json"
    ))
    .unwrap();
    for row in rows {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        if let Some(before) = row["before"].as_str() {
            std::fs::write(&path, before).unwrap();
        }
        let store = Store::new(&path);
        let environment: BTreeMap<String, String> = row["environment"]
            .as_object()
            .map(|values| {
                values
                    .iter()
                    .map(|(key, value)| (key.clone(), value.as_str().unwrap().into()))
                    .collect()
            })
            .unwrap_or_default();
        let alias = row["alias"].as_str().unwrap();
        let mut actual = json!({"error":"", "removed":false,"text":"", "source":"", "has_value":false,"env_override":"", "has_override":false});
        let result = match row["operation"].as_str().unwrap() {
            "get" => store.get_with_environment(alias, environment).map(|value| {
                actual["text"] = json!(public_setting_text(alias, &value.text));
                actual["source"] = json!(value.source);
                actual["has_value"] = json!(value.value.is_some());
            }),
            operation => {
                let result = if operation == "set" {
                    store.set_with_environment(alias, row["raw"].as_str().unwrap(), environment)
                } else {
                    store.unset_with_environment(alias, environment)
                };
                result.map(|value| {
                    assert_eq!(value.alias, alias);
                    actual["env_override"] = json!(value.env_override);
                    actual["has_override"] = json!(value.has_override);
                })
            }
        };
        if let Err(error) = result {
            actual["error"] = json!(error.to_string());
            actual["removed"] = json!(error.is_removed());
        }
        actual["after"] = match std::fs::read_to_string(&path) {
            Ok(body) => json!(body),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Value::Null,
            Err(error) => panic!("{error}"),
        };
        for key in [
            "after",
            "error",
            "removed",
            "text",
            "source",
            "has_value",
            "env_override",
            "has_override",
        ] {
            assert_eq!(actual[key], row[key], "{}: {key}", row["name"]);
        }
    }
}

#[test]
fn managed_aliases_are_twelve_sorted_settings_and_schema_contains_twenty_live_aliases() {
    assert_eq!(
        cli_setting_aliases(),
        vec![
            "account_pool_enabled",
            "account_pool_strategy",
            "directory_template",
            "download_path",
            "filename_template",
            "https_proxy",
            "log_format",
            "log_level",
            "request_interval",
            "reverse_search_pixiv_only",
            "reverse_search_provider",
            "saucenao_api_key"
        ]
    );
    assert_eq!(valid_setting_aliases().len(), 20);
    assert!(!valid_setting_aliases().contains(&"web_fallback_enabled"));
    assert!(!valid_setting_aliases().contains(&"account_pool_accounts"));
}

#[test]
fn store_mutations_read_fresh_files_and_failed_validation_preserves_existing_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    let store = Store::new(&path);
    std::fs::write(&path, "[download]\npath='first'\n").unwrap();
    assert_eq!(
        store
            .get_with_environment("download_path", [])
            .unwrap()
            .text,
        "first"
    );
    let before = "[download]\npath='second'\n";
    std::fs::write(&path, before).unwrap();
    assert_eq!(
        store
            .get_with_environment("download_path", [])
            .unwrap()
            .text,
        "second"
    );
    assert!(
        store
            .set_with_environment("output_json", "invalid", [])
            .is_err()
    );
    assert_eq!(std::fs::read_to_string(path).unwrap(), before);
}

#[test]
fn filesystem_read_failures_leave_existing_targets_and_no_staging_files() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    std::fs::create_dir(&path).unwrap();
    let store = Store::new(&path);
    assert!(
        store
            .set_with_environment("download_path", "./saved", [])
            .is_err()
    );
    assert!(path.is_dir());
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    let blocking = directory.path().join("blocking");
    std::fs::write(&blocking, "retained").unwrap();
    let store = Store::new(blocking.join("config.toml"));
    assert!(store.unset_with_environment("download_path", []).is_err());
    assert_eq!(std::fs::read_to_string(blocking).unwrap(), "retained");
}

#[cfg(unix)]
#[test]
fn sparse_writes_create_and_tighten_private_directory_and_file_modes() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("nested").join("settings");
    let path = private.join("config.toml");
    let store = Store::new(&path);
    store
        .set_with_environment("download_path", "./saved", [])
        .unwrap();
    assert_eq!(
        std::fs::metadata(&private).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    store.unset_with_environment("download_path", []).unwrap();
    assert_eq!(
        std::fs::metadata(&private).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(std::fs::read_dir(private).unwrap().count(), 1);
}

#[test]
fn malformed_document_diagnostics_never_include_synthetic_credentials() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    let synthetic = "synthetic-never-publish-key";
    std::fs::write(
        &path,
        format!("[reverse_search]\nsaucenao_api_key = \"{synthetic}\" trailing\n"),
    )
    .unwrap();
    let store = Store::new(&path);
    let error = match store.set_with_environment("download_path", "./saved", []) {
        Err(error) => error,
        Ok(_) => panic!("malformed document unexpectedly accepted"),
    };
    assert!(!error.to_string().contains(synthetic));
    assert!(!format!("{error:?}").contains(synthetic));
}
