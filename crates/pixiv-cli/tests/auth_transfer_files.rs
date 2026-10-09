use pixiv_app::{
    auth_bundle,
    config::Store,
    database::{Database, PixivAccount},
};
use pixiv_cli_rs::auth_transfer::TransferCommand;
#[test]
fn export_output_writes_bundle_and_force_requires_changed_output() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::new(home.path().join("config.toml"));
    let mut db = Database::open(home.path()).unwrap();
    db.save_pixiv_credential(&PixivAccount::new(42, "fixture", b"synthetic-refresh"))
        .unwrap();
    drop(db);
    let path = home.path().join("export.json");
    let args = vec![
        "export".into(),
        "42".into(),
        "--output".into(),
        path.to_string_lossy().into_owned(),
    ];
    let command = TransferCommand::parse(&args, &mut &b""[..], false).unwrap();
    let mut output = Vec::new();
    command.execute_offline(&store, &mut output).unwrap();
    assert_eq!(
        output,
        format!("output: {}\naccounts: 1\n", path.display()).as_bytes()
    );
    let bundle = auth_bundle::decode(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(bundle.accounts[0].user_id, 42);
    assert_eq!(bundle.accounts[0].refresh_token, "synthetic-refresh");
    assert!(!store.path().exists());
    let error = command
        .execute_offline(&store, &mut Vec::new())
        .unwrap_err()
        .to_string();
    assert!(!error.contains(&path.to_string_lossy().to_string()));
    assert!(!error.contains("synthetic-refresh"));
    let mut force = args.clone();
    force.push("--force".into());
    TransferCommand::parse(&force, &mut &b""[..], false)
        .unwrap()
        .execute_offline(&store, &mut Vec::new())
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
