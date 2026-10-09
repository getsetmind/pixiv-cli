use pixiv_app::handler_manifest::{
    HandlerFileSnapshot, HandlerManifest, HandlerManifestError, HandlerManifestStore, decode,
    encode,
};
use serde::Deserialize;
use std::fs;

#[derive(Deserialize)]
struct Case {
    name: String,
    input: String,
    output: Option<String>,
}

#[test]
fn shared_go_manifest_fixture() {
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("fixtures/handler_manifest.json")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let store = HandlerManifestStore::new(dir.path().join("manifest.json"));
    for case in cases {
        fs::write(store.path(), &case.input).unwrap();
        match case.output {
            Some(expected) => {
                let manifest = store
                    .load()
                    .unwrap_or_else(|error| panic!("{}: {error}", case.name))
                    .unwrap();
                assert_eq!(
                    encode(&manifest).unwrap(),
                    expected.as_bytes(),
                    "{}",
                    case.name
                );
            }
            None => {
                assert!(
                    matches!(store.load(), Err(HandlerManifestError::Invalid)),
                    "{}",
                    case.name
                );
                assert_eq!(
                    decode(case.input.as_bytes()).unwrap_err().to_string(),
                    "Pixiv URL handler manifest is invalid"
                );
            }
        }
    }
}

#[test]
fn go_depth_limit_unknown_values_and_non_utf8_strings() {
    for depth in [9999, 10000] {
        let input = format!(
            "{{\"version\":1,\"executable_path\":\"p\",\"future\":{}0{}}}",
            "[".repeat(depth),
            "]".repeat(depth)
        );
        assert_eq!(
            decode(input.as_bytes()).is_ok(),
            depth == 9999,
            "depth {depth}"
        );
    }
    let mut body = br#"{"version":1,"executable_path":""#.to_vec();
    body.extend_from_slice(&[0xff, 0xfe]);
    body.extend_from_slice(br#""}"#);
    assert_eq!(decode(&body).unwrap().executable_path, "��");
}

#[test]
fn nil_empty_slice_distinction_is_preserved_but_omitted_on_save() {
    let base = r#"{"version":1,"executable_path":"p""#;
    assert!(
        decode(format!("{base}}}").as_bytes())
            .unwrap()
            .linux_mime_snapshots
            .is_none()
    );
    let empty = decode(format!("{base},\"linux_mime_snapshots\":[]}}").as_bytes()).unwrap();
    assert_eq!(empty.linux_mime_snapshots, Some(vec![]));
    assert_eq!(
        encode(&empty).unwrap(),
        br#"{"version":1,"executable_path":"p"}"#
    );
    let snapshots = decode(
        format!("{base},\"linux_mime_snapshots\":[{{\"content\":[]}},{{\"content\":null}}]}}")
            .as_bytes(),
    )
    .unwrap()
    .linux_mime_snapshots
    .unwrap();
    assert_eq!(snapshots[0].content, Some(vec![]));
    assert_eq!(snapshots[1].content, None);
}

#[test]
fn save_promotes_only_version_zero_without_validation_or_locks() {
    let dir = tempfile::tempdir().unwrap();
    let store = HandlerManifestStore::new(dir.path().join("url-handler/handler-manifest.json"));
    assert_eq!(store.load().unwrap(), None);
    store.remove().unwrap();
    for version in [0, -2, 2] {
        let manifest = HandlerManifest {
            version,
            linux_mime_snapshots: Some(vec![]),
            ..HandlerManifest::default()
        };
        store.save(&manifest).unwrap();
        let expected = format!(
            "{{\"version\":{},\"executable_path\":\"\"}}",
            if version == 0 { 1 } else { version }
        );
        assert_eq!(fs::read(store.path()).unwrap(), expected.as_bytes());
        assert!(matches!(store.load(), Err(HandlerManifestError::Invalid)));
        assert_eq!(
            fs::read_dir(store.path().parent().unwrap())
                .unwrap()
                .count(),
            1
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(store.path()).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(store.path().parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
    }
    let manifest = HandlerManifest {
        executable_path: "p".into(),
        linux_mime_snapshots: Some(vec![HandlerFileSnapshot {
            path: "mime".into(),
            exists: true,
            mode: u32::MAX,
            content: Some(vec![0, 255]),
        }]),
        ..HandlerManifest::default()
    };
    store.save(&manifest).unwrap();
    assert_eq!(store.load().unwrap().unwrap().version, 1);
    store.remove().unwrap();
    assert_eq!(store.load().unwrap(), None);
}

#[test]
fn unreadable_save_and_remove_failures_use_storage_errors() {
    let dir = tempfile::tempdir().unwrap();
    let store = HandlerManifestStore::new(dir.path().join("manifest"));
    fs::create_dir(store.path()).unwrap();
    let error = store.load().unwrap_err();
    assert!(matches!(error, HandlerManifestError::Io(_)));
    assert!(std::error::Error::source(&error).is_some());
    assert!(std::error::Error::source(&HandlerManifestError::Invalid).is_none());
    fs::write(store.path().join("child"), b"x").unwrap();
    assert!(matches!(store.remove(), Err(HandlerManifestError::Io(_))));
    assert!(matches!(
        store.save(&HandlerManifest::default()),
        Err(HandlerManifestError::Storage(_))
    ));
    fs::remove_file(store.path().join("child")).unwrap();
    store.remove().unwrap();
    assert_eq!(store.load().unwrap(), None);
    fs::write(dir.path().join("blocker"), b"file").unwrap();
    assert!(
        HandlerManifestStore::new(dir.path().join("blocker/manifest"))
            .save(&HandlerManifest::default())
            .is_err()
    );
}
