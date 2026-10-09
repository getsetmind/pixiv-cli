use pixiv_app::config::Store;

#[test]
fn default_account_mutations_match_frozen_go_document_bytes_and_validation_order() {
    for (name, before, clear, id, after, error) in [
        (
            "set-missing",
            None,
            false,
            42,
            "[pixiv.auth]\ndefault_user_id = 42\n",
            "",
        ),
        ("clear-missing", None, true, 0, "", ""),
        (
            "replace-comments",
            Some(
                "# root\n[pixiv.auth] # table\n# owned\ndefault_user_id=7 # owned\nkeep='value' # retained\n\n[unknown]\nkeep=true\n",
            ),
            false,
            42,
            "# root\n[pixiv.auth]  # table\ndefault_user_id = 42\nkeep = 'value'  # retained\n\n[unknown]\nkeep = true\n",
            "",
        ),
        (
            "clear-empty-table",
            Some("[pixiv.auth]\ndefault_user_id=7\n"),
            true,
            0,
            "",
            "",
        ),
        (
            "clear-keeps-unknown",
            Some("[pixiv.auth]\ndefault_user_id=7\nkeep=true\n"),
            true,
            0,
            "[pixiv.auth]\nkeep = true\n",
            "",
        ),
        (
            "invalid-before-read",
            Some("[broken\n"),
            false,
            0,
            "[broken\n",
            "config: default_user_id must be positive",
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        if let Some(before) = before {
            std::fs::write(&path, before).unwrap();
        }
        let store = Store::new(&path);
        let result = if clear {
            store.clear_pixiv_default_user_id()
        } else {
            store.set_pixiv_default_user_id(id)
        };
        assert_eq!(
            result
                .err()
                .map(|error| error.to_string())
                .unwrap_or_default(),
            error,
            "{name}"
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), after, "{name}");
    }
}

#[test]
fn default_writes_read_current_documents_and_leave_targets_intact_on_failure() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    let store = Store::new(&path);
    store.set_pixiv_default_user_id(42).unwrap();
    std::fs::write(&path, "# fresh\n[unknown]\nkeep=true\n").unwrap();
    store.set_pixiv_default_user_id(7).unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "# fresh\n[unknown]\nkeep = true\n\n[pixiv.auth]\ndefault_user_id = 7\n"
    );
    std::fs::write(&path, "[broken\n").unwrap();
    assert!(store.clear_pixiv_default_user_id().is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "[broken\n");
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(store.set_pixiv_default_user_id(42).is_err());
    assert!(store.clear_pixiv_default_user_id().is_err());
    assert!(path.is_dir());
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[cfg(unix)]
#[test]
fn default_writes_create_and_tighten_private_modes_without_staging_files() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("nested/settings");
    let path = private.join("config.toml");
    let store = Store::new(&path);
    store.set_pixiv_default_user_id(42).unwrap();
    for (path, mode) in [(&private, 0o700), (&path, 0o600)] {
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            mode
        );
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o777)).unwrap();
    }
    store.clear_pixiv_default_user_id().unwrap();
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
