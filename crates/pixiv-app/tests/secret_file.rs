use pixiv_app::secret_file::{self, WriteCommitOutcome};
use std::fs;
#[test]
fn export_writer_full_output_force_workflow() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("new/deeper/export");
    secret_file::write(&path, b"synthetic-old", false).unwrap();
    let error = secret_file::write(&path, b"synthetic-new", false).unwrap_err();
    assert_eq!(error.commit_outcome(), WriteCommitOutcome::NotCommitted);
    assert_eq!(
        error.to_string(),
        "authentication export write failed: destination already exists (not_committed)"
    );
    assert_eq!(fs::read(&path).unwrap(), b"synthetic-old");
    secret_file::write(&path, b"synthetic-new", true).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"synthetic-new");
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
}
#[cfg(unix)]
#[test]
fn export_preserves_parent_modes_and_replaces_symlink_itself() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("exports");
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o751)).unwrap();
    let target = dir.join("target");
    fs::write(&target, b"untouched").unwrap();
    let path = dir.join("synthetic-secret-path");
    symlink(&target, &path).unwrap();
    assert!(secret_file::write(&path, b"new", false).is_err());
    secret_file::write(&path, b"new", true).unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"untouched");
    assert!(fs::symlink_metadata(&path).unwrap().is_file());
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o751
    );
    let error = secret_file::write(&dir, b"invalid", true).unwrap_err();
    assert_eq!(error.commit_outcome(), WriteCommitOutcome::NotCommitted);
    assert!(!error.to_string().contains(dir.to_str().unwrap()));
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 2);
}

#[test]
fn parent_file_is_not_reported_as_an_existing_output() {
    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path().join("synthetic-parent");
    fs::write(&parent, b"untouched").unwrap();
    for force in [false, true] {
        let error = secret_file::write(&parent.join("output"), b"secret", force).unwrap_err();
        assert_eq!(
            error.to_string(),
            "authentication export write failed: filesystem operation failed (not_committed)"
        );
    }
    assert_eq!(fs::read(&parent).unwrap(), b"untouched");
}
