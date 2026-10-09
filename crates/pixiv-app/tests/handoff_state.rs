use pixiv_app::handoff_protocol::RemoteLoginStart;
use pixiv_app::handoff_state::{
    ActiveRemoteLogin, HandoffState, HandoffStateError, load_active_remote_login_at,
    save_active_remote_login_at,
};
use pixiv_app::private_lock::with_private_lock;
use std::{fs, path::Path, process::Command, time::Duration};
use tokio_util::sync::CancellationToken;

fn active(session: &str) -> ActiveRemoteLogin {
    ActiveRemoteLogin {
        version: 1,
        origin: "https://relay.example".into(),
        session_id: session.into(),
        proof: "proof".into(),
    }
}

#[test]
fn ordered_go_json_fields_nulls_casefold_and_unknown_fields() {
    let cases = [
        (
            r#"{"version":1,"origin":" https://relay.example/ ","session_id":" session ","proof":"proof"}"#,
            " session ",
        ),
        (
            r#"{"VERSION":1,"ORIGIN":"https://relay.example","SESSION_ID":"session","PROOF":"proof"}"#,
            "session",
        ),
        (
            r#"{"verſion":1,"origin":"https://relay.example","ſeſſion_id":"session","proof":"proof"}"#,
            "session",
        ),
        (
            r#"{"version":0,"VERSION":1,"origin":"https://relay.example","session_id":"old","SESSION_ID":"new","proof":"proof"}"#,
            "new",
        ),
        (
            r#"{"version":1,"version":null,"origin":"https://relay.example","origin":null,"session_id":"session","session_id":null,"proof":"proof","proof":null}"#,
            "session",
        ),
        (
            r#"{"version":1,"origin":"https://relay.example","session_id":"session","proof":"proof","ttl":-1,"unknown":{"deep":[null]}}"#,
            "session",
        ),
        (
            r#"{"version":1,"origin":"https://relay.example","session_id":"\ud800","proof":"proof"}"#,
            "�",
        ),
    ];
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    for (body, session) in cases {
        fs::write(&path, body).unwrap();
        assert_eq!(load_active_remote_login_at(&path).unwrap(), active(session));
    }
    let mut invalid_utf8 =
        br#"{"version":1,"origin":"https://relay.example","session_id":""#.to_vec();
    invalid_utf8.extend_from_slice(&[0xff, 0xfe]);
    invalid_utf8.extend_from_slice(br#"","proof":"proof"}"#);
    fs::write(&path, invalid_utf8).unwrap();
    assert_eq!(load_active_remote_login_at(&path).unwrap(), active("��"));
}

#[test]
fn invalid_states_and_read_errors_have_exact_redacted_messages() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    assert!(matches!(
        load_active_remote_login_at(&path),
        Err(HandoffStateError::NoActiveRemoteLogin)
    ));
    for body in [
        "null",
        "",
        "[]",
        r#"{"version":1.0,"origin":"https://relay.example","session_id":"session","proof":"proof"}"#,
        r#"{"version":"bad","version":1,"origin":"https://relay.example","session_id":"session","proof":"proof"}"#,
        r#"{"version":1,"origin":"https://relay.example","session_id":"\u0085\u00a0","proof":"proof"}"#,
        r#"{"version":1,"origin":"https://relay.example","SessionID":"session","proof":"proof"}"#,
        r#"{"version":1,"origin":"https://relay.example","session_id":"session","proof":"proof"} {}"#,
        r#"{"version":9223372036854775808,"origin":"https://relay.example","session_id":"session","proof":"proof"}"#,
    ] {
        fs::write(&path, body).unwrap();
        assert_eq!(
            load_active_remote_login_at(&path).unwrap_err().to_string(),
            "active remote login handoff is invalid"
        );
    }
    assert_eq!(
        load_active_remote_login_at(dir.path())
            .unwrap_err()
            .to_string(),
        "could not read active remote login handoff"
    );
}

#[test]
fn save_is_unvalidated_compact_atomic_and_private() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("private").join("state.json");
    let session = ActiveRemoteLogin {
        version: 7,
        origin: "bad".into(),
        session_id: String::new(),
        proof: "<&>\u{2028}\u{2029}".into(),
    };
    save_active_remote_login_at(&path, &session).unwrap();
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        r#"{"version":7,"origin":"bad","session_id":"","proof":"\u003c\u0026\u003e\u2028\u2029"}"#
    );
    save_active_remote_login_at(&path, &active("new")).unwrap();
    assert_eq!(load_active_remote_login_at(&path).unwrap(), active("new"));
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
}

#[test]
fn conditional_clear_preserves_newer_session_and_opaque_capabilities() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let store = HandoffState::new(&path);
    let start = RemoteLoginStart {
        origin: " https://relay.example/ ".into(),
        session_id: "old".into(),
        proof: "proof".into(),
    };
    store.clear_remote_login_handoff(&start).unwrap();
    store.save(&active("new")).unwrap();
    store.clear_remote_login_handoff(&start).unwrap();
    assert_eq!(store.load().unwrap(), active("new"));
    let mut expected = active("new");
    expected.proof.push(' ');
    store.clear_if_matches(&expected).unwrap();
    assert!(path.exists());
    expected = active("new");
    expected.version = 2;
    store.clear_if_matches(&expected).unwrap();
    assert!(path.exists());
    expected = active("new");
    expected.origin.push('/');
    store.clear_if_matches(&expected).unwrap();
    assert!(path.exists());
    store.clear_if_matches(&active("new")).unwrap();
    assert!(!path.exists());
    assert!(Path::new(&format!("{}.lock", path.display())).exists());
    fs::write(&path, "synthetic-invalid").unwrap();
    assert_eq!(
        store
            .clear_remote_login_handoff(&start)
            .unwrap_err()
            .to_string(),
        "could not clear active remote login handoff"
    );
    let invalid = RemoteLoginStart {
        origin: start.origin,
        session_id: " ".into(),
        proof: start.proof,
    };
    assert_eq!(
        store
            .clear_remote_login_handoff(&invalid)
            .unwrap_err()
            .to_string(),
        "invalid remote login handoff"
    );
}

#[test]
fn private_lock_coordinates_processes_and_survives_atomic_replacement() {
    if let Some(path) = std::env::var_os("PIXIV_MIGRATION_RUST_LOCK_PATH") {
        let store = HandoffState::new(path);
        let cancel = CancellationToken::new();
        fs::write(store.path().with_extension("ready"), "ready").unwrap();
        with_private_lock(store.path(), Some(&cancel), || {
            fs::write(store.path().with_extension("acquired"), "yes").unwrap();
            Ok(())
        })
        .unwrap();
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let store = HandoffState::new(&path);
    let cancel = CancellationToken::new();
    let mut child = None;
    with_private_lock(store.path(), Some(&cancel), || {
        child = Some(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "private_lock_coordinates_processes_and_survives_atomic_replacement",
                ])
                .env("PIXIV_MIGRATION_RUST_LOCK_PATH", &path)
                .spawn()
                .unwrap(),
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !path.with_extension("ready").exists() {
            assert!(
                child.as_mut().unwrap().try_wait().unwrap().is_none(),
                "lock child exited before readiness"
            );
            assert!(
                std::time::Instant::now() < deadline,
                "lock child did not become ready"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(Duration::from_millis(50));
        assert!(!path.with_extension("acquired").exists());
        save_active_remote_login_at(&path, &active("replacement")).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        assert!(!path.with_extension("acquired").exists());
        Ok(())
    })
    .unwrap();
    assert!(child.unwrap().wait().unwrap().success());
    assert!(path.with_extension("acquired").exists());
    assert_eq!(store.load().unwrap(), active("replacement"));
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let absent = HandoffState::new(dir.path().join("absent/state"));
    assert!(matches!(
        with_private_lock(
            absent.path(),
            Some(&cancelled),
            || -> Result<(), HandoffStateError> { panic!("cancelled action ran") }
        ),
        Err(HandoffStateError::Cancelled)
    ));
    assert!(!dir.path().join("absent").exists());
}

#[test]
fn unknown_fields_follow_go_json_depth_limit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    for depth in [129, 9999, 10000] {
        let body = format!(
            r#"{{"version":1,"origin":"https://relay.example","session_id":"session","proof":"proof","unknown":{}null{}}}"#,
            "[".repeat(depth),
            "]".repeat(depth)
        );
        fs::write(&path, body).unwrap();
        let result = load_active_remote_login_at(&path);
        if depth < 10000 {
            assert_eq!(result.unwrap(), active("session"));
        } else {
            assert_eq!(
                result.unwrap_err().to_string(),
                "active remote login handoff is invalid"
            );
        }
    }
}

#[test]
fn cancellation_while_waiting_skips_action_after_native_acquire() {
    let dir = tempfile::tempdir().unwrap();
    let store = HandoffState::new(dir.path().join("state.json"));
    let owner_cancel = CancellationToken::new();
    let waiting_cancel = CancellationToken::new();
    let (ready_send, ready_recv) = std::sync::mpsc::channel();
    let (result_send, result_recv) = std::sync::mpsc::channel();
    let mut waiter = None;
    with_private_lock(store.path(), Some(&owner_cancel), || {
        let store = store.clone();
        let cancel = waiting_cancel.clone();
        waiter = Some(std::thread::spawn(move || {
            ready_send.send(()).unwrap();
            let result: Result<(), HandoffStateError> =
                with_private_lock(store.path(), Some(&cancel), || {
                    panic!("cancelled action ran")
                });
            result_send.send(result).unwrap();
        }));
        ready_recv.recv_timeout(Duration::from_secs(10)).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        waiting_cancel.cancel();
        std::thread::sleep(Duration::from_millis(50));
        assert!(matches!(
            result_recv.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        Ok(())
    })
    .unwrap();
    assert!(matches!(
        result_recv.recv_timeout(Duration::from_secs(10)).unwrap(),
        Err(HandoffStateError::Cancelled)
    ));
    waiter.unwrap().join().unwrap();
    store.save(&active("after-cancellation")).unwrap();
    assert_eq!(store.load().unwrap(), active("after-cancellation"));
}
