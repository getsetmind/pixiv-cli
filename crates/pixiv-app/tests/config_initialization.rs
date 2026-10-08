use pixiv_app::config::Store;
use serde::Deserialize;

#[derive(Deserialize)]
struct Case {
    name: String,
    before: Option<String>,
    after: String,
}

#[test]
fn initial_configuration_matches_go_default_bytes_and_preserves_existing_documents() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/config-initialization.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 4);
    for case in cases {
        let home = tempfile::tempdir().unwrap();
        let directory = home.path().join(".pixiv-cli");
        let path = directory.join("config.toml");
        if let Some(before) = case.before {
            std::fs::create_dir(&directory).unwrap();
            std::fs::write(&path, before).unwrap();
        }
        let store = Store::new(&path);
        store.ensure_defaults().unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            case.after.as_bytes(),
            "{}",
            case.name
        );
        store.ensure_defaults().unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            case.after.as_bytes(),
            "{}",
            case.name
        );
        assert!(!directory.join("pixiv-cli.db").exists());
    }
}

#[cfg(unix)]
#[test]
fn initialization_tightens_the_directory_and_preserves_existing_file_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    let directory = home.path().join(".pixiv-cli");
    std::fs::create_dir(&directory).unwrap();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o777)).unwrap();
    let path = directory.join("config.toml");
    std::fs::write(&path, b"# existing\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    Store::new(&path).ensure_defaults().unwrap();
    assert_eq!(
        std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o644
    );
    let created = directory.join("new.toml");
    Store::new(&created).ensure_defaults().unwrap();
    assert_eq!(
        std::fs::metadata(&created).unwrap().permissions().mode() & 0o777,
        0o600
    );
}
