#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

#[path = "support/browser_profiles.rs"]
mod support;

use pixiv_app::{
    browser_cookies::{BrowserCookieError, CookieQuery},
    browser_profiles::{FirefoxCookieProvider, SafariCookieProvider},
    browser_sqlite::SqliteCommand,
    lifecycle::Context,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};
use support::{CapturedProcess, OwnedFiles, context, nodes, unhex, values, write};

const FIREFOX_BYTES: &[u8] = include_bytes!("../../pixiv-cli/tests/fixtures/browser-firefox.json");
const SAFARI_BYTES: &[u8] = include_bytes!("../../pixiv-cli/tests/fixtures/browser-safari.json");
const FIRST_SAFARI: &str =
    "Library/Containers/com.apple.Safari/Data/Library/Cookies/Cookies.binarycookies";
const SECOND_SAFARI: &str = "Library/Cookies/Cookies.binarycookies";

fn fixture(bytes: &[u8], seal: &str, count: usize) -> Value {
    assert_eq!(format!("{:x}", Sha256::digest(bytes)), seal);
    let fixture: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        fixture["reference"],
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    assert_eq!(fixture["environment"], "linux/amd64");
    assert_eq!(fixture["cases"].as_array().unwrap().len(), count);
    fixture
}
fn firefox_fixture() -> Value {
    fixture(
        FIREFOX_BYTES,
        "72e2ad4c9ca25642647e537589de4b061572aa2a55d2f9d0260e19f32e67c568",
        60,
    )
}
fn safari_fixture() -> Value {
    fixture(
        SAFARI_BYTES,
        "9fc9b0d8a311d35cbcf705bc44e160d0696bfc74fe1f99a6128a9c0765e291de",
        57,
    )
}
fn error_or_values(
    result: Result<Vec<pixiv_app::browser_cookies::SecretBytes>, BrowserCookieError>,
) -> Value {
    match result {
        Ok(secrets) => json!({"values_hex": values(secrets), "error": ""}),
        Err(error) => json!({"values_hex": null, "error": error.to_string()}),
    }
}
fn safari_paths(root: &Path) -> Vec<PathBuf> {
    vec![root.join(FIRST_SAFARI), root.join(SECOND_SAFARI)]
}
fn duplicate_profiles(root: &Path) {
    write(&root.join("profiles.ini"), b"[Profile0]\nName=first\nPath=one/duplicate\n[Profile1]\nName=second\nPath=two/duplicate\n");
    write(&root.join("one/duplicate/cookies.sqlite"), b"");
    write(&root.join("two/duplicate/cookies.sqlite"), b"");
}
fn no_process() -> Arc<CapturedProcess> {
    Arc::new(CapturedProcess::from_input(
        &json!({"stdout_hex":"", "stderr_hex":"", "exit":0, "missing_command":false}),
    ))
}
fn firefox(
    root: &Path,
    files: Arc<OwnedFiles>,
    process: Arc<CapturedProcess>,
) -> FirefoxCookieProvider {
    FirefoxCookieProvider::new(root.into(), files, Arc::new(SqliteCommand::new(process)))
}

#[test]
fn firefox_provider_discovery_matches_twenty_sealed_go_rows_using_owned_files() {
    let fixture = firefox_fixture();
    let mut compared = 0;
    for case in fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["operation"] == "discover")
    {
        let root = tempfile::tempdir().unwrap();
        let input = &case["input"];
        nodes(root.path(), input, true);
        let ini = root.path().join("profiles.ini");
        if input["ini_directory"] == true {
            fs::create_dir(&ini).unwrap();
        } else if input["ini_missing"] != true {
            write(
                &ini,
                &support::replace(
                    &unhex(input["ini_hex"].as_str().unwrap()),
                    b"$root",
                    root.path().as_os_str().as_encoded_bytes(),
                ),
            );
        }
        let result = firefox(root.path(), Arc::new(OwnedFiles::default()), no_process())
            .discover_profiles(&context(input));
        let observed = match result {
            Ok(profiles) => {
                json!({"profiles":support::firefox_profiles(&profiles, root.path()), "error":""})
            }
            Err(error) => json!({"profiles":null, "error":error.to_string()}),
        };
        assert_eq!(
            observed["profiles"], case["output"]["profiles"],
            "{}",
            case["name"]
        );
        assert_eq!(
            observed["error"], case["output"]["error"],
            "{}",
            case["name"]
        );
        compared += 1;
    }
    assert_eq!(compared, 20);
}

#[test]
fn firefox_provider_reads_match_seventeen_go_process_boundary_rows_and_fixed_command_semantics() {
    let fixture = firefox_fixture();
    let mut compared = 0;
    let mut constructor_rejections = 0;
    for case in fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["operation"] == "read")
    {
        let root = tempfile::tempdir().unwrap();
        duplicate_profiles(root.path());
        let input = &case["input"];
        let process = Arc::new(CapturedProcess::from_input(input));
        let provider = firefox(
            root.path(),
            Arc::new(OwnedFiles::default()),
            process.clone(),
        );
        let context = context(input);
        let query = CookieQuery::new(
            input["query_host"].as_str().unwrap(),
            input["query_name"].as_str().unwrap(),
        );
        let observed = match query {
            Ok(query) => {
                compared += 1;
                error_or_values(provider.read(
                    &context,
                    &query,
                    &unhex(input["profile_id_hex"].as_str().unwrap()),
                ))
            }
            Err(error) => {
                constructor_rejections += 1;
                json!({"values_hex":null, "error":error.to_string()})
            }
        };
        assert_eq!(
            observed["values_hex"], case["output"]["values_hex"],
            "{}",
            case["name"]
        );
        assert_eq!(
            observed["error"], case["output"]["error"],
            "{}",
            case["name"]
        );
        assert_eq!(
            process.command(root.path()),
            case["output"]["command"],
            "{}",
            case["name"]
        );
        assert!(
            process
                .observed
                .lock()
                .unwrap()
                .iter()
                .all(|(_, _, _, address)| *address == &context as *const Context as usize)
        );
    }
    assert_eq!(compared, 17);
    assert_eq!(constructor_rejections, 1);
}

#[test]
fn firefox_provider_reads_match_seven_go_rows_with_the_pinned_official_sqlite_shell() {
    use pixiv_app::host_process::{HostProcess, ProcessStdio, SystemHostProcess};
    use std::ffi::OsStr;
    let fixture = firefox_fixture();
    let process = SystemHostProcess;
    let executable = process
        .look_path(OsStr::new("sqlite3"))
        .expect("the pinned official SQLite shell must be on PATH");
    assert_eq!(
        format!("{:x}", Sha256::digest(fs::read(&executable).unwrap())),
        fixture["sqlite_shell"]["sha256"].as_str().unwrap()
    );
    let version = process
        .run(
            OsStr::new("sqlite3"),
            &["-version".into()],
            ProcessStdio::CaptureStdout,
        )
        .unwrap();
    assert_eq!(
        std::str::from_utf8(&version.stdout).unwrap().trim(),
        fixture["sqlite_shell"]["version"].as_str().unwrap()
    );
    let mut compared = 0;
    for case in fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["operation"] == "read_sqlite")
    {
        let root = tempfile::tempdir().unwrap();
        let input = &case["input"];
        assert_eq!(input["boundary"], "official_sqlite_shell");
        write(
            &root.path().join("profiles.ini"),
            b"[Profile0]\nPath=release\n",
        );
        let path = root.path().join("release/cookies.sqlite");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let database = if input["database_state"] == "invalid" {
            write(&path, b"owned invalid database");
            None
        } else {
            let database = rusqlite::Connection::open(&path).unwrap();
            database
                .execute_batch(input["schema"].as_str().unwrap())
                .unwrap();
            if let Some(statements) = input["statements"].as_array() {
                for statement in statements {
                    database.execute_batch(statement.as_str().unwrap()).unwrap();
                }
            }
            if input["exclusive_lock"] == true {
                database.execute_batch("BEGIN EXCLUSIVE;").unwrap();
                Some(database)
            } else {
                database.close().unwrap();
                None
            }
        };
        let provider = FirefoxCookieProvider::new(
            root.path().into(),
            Arc::new(OwnedFiles::default()),
            Arc::new(SqliteCommand::new(Arc::new(SystemHostProcess))),
        );
        let query = CookieQuery::new(
            input["query_host"].as_str().unwrap(),
            input["query_name"].as_str().unwrap(),
        )
        .unwrap();
        assert_eq!(
            error_or_values(provider.read(
                &Context::new(),
                &query,
                input["profile_id"].as_str().unwrap().as_bytes()
            )),
            case["output"],
            "{}",
            case["name"]
        );
        if let Some(database) = database {
            database.execute_batch("ROLLBACK;").unwrap();
        }
        compared += 1;
    }
    assert_eq!(compared, 7);
}

#[test]
fn safari_provider_discovery_matches_seven_sealed_go_rows_using_owned_files() {
    let fixture = safari_fixture();
    let mut compared = 0;
    for case in fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["operation"] == "discover")
    {
        let root = tempfile::tempdir().unwrap();
        nodes(root.path(), &case["input"], false);
        let paths = safari_paths(root.path());
        let normalized = paths
            .iter()
            .map(|path| {
                String::from_utf8(support::replace(
                    path.as_os_str().as_encoded_bytes(),
                    root.path().as_os_str().as_encoded_bytes(),
                    b"$home",
                ))
                .unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(json!(normalized), case["output"]["candidate_paths"]);
        let provider = SafariCookieProvider::new(paths, Arc::new(OwnedFiles::default()));
        let observed = match provider.discover_profiles(&context(&case["input"])) {
            Ok(profiles) => {
                json!({"profiles":support::safari_profiles(&profiles, root.path()), "error":""})
            }
            Err(error) => json!({"profiles":null, "error":error.to_string()}),
        };
        assert_eq!(
            observed["profiles"], case["output"]["profiles"],
            "{}",
            case["name"]
        );
        assert_eq!(
            observed["error"], case["output"]["error"],
            "{}",
            case["name"]
        );
        compared += 1;
    }
    assert_eq!(compared, 7);
}

#[test]
fn safari_provider_reads_match_nineteen_sealed_go_rows_without_strengthened_cancellation() {
    let fixture = safari_fixture();
    let mut compared = 0;
    let mut constructor_rejections = 0;
    for case in fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["operation"] == "read")
    {
        let root = tempfile::tempdir().unwrap();
        let input = &case["input"];
        let paths = if input["empty_candidates"] == true {
            vec![]
        } else {
            safari_paths(root.path())
        };
        if input["directory_file"] == true {
            fs::create_dir_all(root.path().join(FIRST_SAFARI)).unwrap();
        } else if input["missing_file"] != true && input["empty_candidates"] != true {
            write(&paths[0], &unhex(input["data_hex"].as_str().unwrap()));
        }
        let provider = SafariCookieProvider::new(paths, Arc::new(OwnedFiles::default()));
        let query = CookieQuery::new(
            input["query_host"].as_str().unwrap(),
            input["query_name"].as_str().unwrap(),
        );
        let observed = match query {
            Ok(query) => {
                compared += 1;
                error_or_values(provider.read(
                    &context(input),
                    &query,
                    input["profile_id"].as_str().unwrap().as_bytes(),
                ))
            }
            Err(error) => {
                constructor_rejections += 1;
                json!({"values_hex":null, "error":error.to_string()})
            }
        };
        assert_eq!(observed, case["output"], "{}", case["name"]);
    }
    assert_eq!(compared, 19);
    assert_eq!(constructor_rejections, 1);
}

#[test]
fn safari_read_reuses_go_parser_invalid_input_specimens_and_preserves_the_genuine_slice_panic() {
    let fixture = safari_fixture();
    let mut malformed = 0;
    let mut panic_specimens = 0;
    for case in fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["operation"] == "parse_binary_cookies")
    {
        let expected = &case["output"];
        if expected["error"] == "" && expected["panic"] == "" {
            continue;
        }
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("Cookies.binarycookies");
        write(&path, &unhex(case["input"]["data_hex"].as_str().unwrap()));
        let provider = SafariCookieProvider::new(vec![path], Arc::new(OwnedFiles::default()));
        let query = CookieQuery::new(".fanbox.cc", "FANBOXSESSID").unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            provider.read(&Context::new(), &query, b"Default")
        }));
        if expected["panic"] != "" {
            assert!(result.is_err(), "{}", case["name"]);
            panic_specimens += 1;
        } else {
            let error = result.unwrap().unwrap_err();
            assert_eq!(
                error.to_string(),
                expected["error"].as_str().unwrap(),
                "{}",
                case["name"]
            );
            malformed += 1;
        }
    }
    assert_eq!(malformed, 16);
    assert_eq!(panic_specimens, 1);
}

#[test]
fn safari_read_reuses_successful_parser_specimens_without_exporting_or_mirroring_the_parser() {
    let fixture = safari_fixture();
    for name in [
        "valid-record",
        "zero-pages",
        "zero-pages-ignore-trailing-bytes",
        "valid-file-ignore-trailing-bytes",
        "record-version-is-ignored",
        "timestamps-are-ignored",
        "unknown-flags-ignored",
        "unknown-final-byte-optional",
        "multiple-interleaved-pages-preserve-order",
        "1025-pages-no-arbitrary-cap",
    ] {
        let case = fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["name"] == name)
            .unwrap();
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("Cookies.binarycookies");
        write(&path, &unhex(case["input"]["data_hex"].as_str().unwrap()));
        let provider = SafariCookieProvider::new(vec![path], Arc::new(OwnedFiles::default()));
        let result = provider
            .read(
                &Context::new(),
                &CookieQuery::new(".fanbox.cc", "FANBOXSESSID").unwrap(),
                b"Default",
            )
            .unwrap();
        let observed = values(result);
        if case["output"]["cookies"].is_null() {
            assert_eq!(observed, json!([]));
        } else {
            let captured = case["output"]["cookies"]
                .as_array()
                .unwrap()
                .iter()
                .map(|cookie| cookie["value_hex"].clone())
                .collect::<Vec<_>>();
            assert_eq!(observed, json!(captured));
        }
    }
}

#[test]
fn permission_and_context_order_follow_the_frozen_provider_sources() {
    let root = tempfile::tempdir().unwrap();
    let ini = root.path().join("profiles.ini");
    let files = Arc::new(OwnedFiles::default());
    files
        .errors
        .lock()
        .unwrap()
        .insert(ini.clone(), io::ErrorKind::PermissionDenied);
    let provider = firefox(root.path(), files.clone(), no_process());
    assert!(matches!(
        provider.discover_profiles(&Context::new()),
        Err(BrowserCookieError::PermissionDenied)
    ));
    let canceled = Context::new();
    canceled.cancel();
    let before = files.trace.lock().unwrap().len();
    assert!(matches!(
        provider.discover_profiles(&canceled),
        Err(BrowserCookieError::Context(_))
    ));
    assert_eq!(files.trace.lock().unwrap().len(), before);
    files.errors.lock().unwrap().remove(&ini);
    duplicate_profiles(root.path());
    files.errors.lock().unwrap().insert(
        root.path().join("one/duplicate/cookies.sqlite"),
        io::ErrorKind::PermissionDenied,
    );
    assert!(matches!(
        provider.discover_profiles(&Context::new()),
        Err(BrowserCookieError::PermissionDenied)
    ));
    let paths = safari_paths(root.path());
    write(&paths[1], b"cook\0\0\0\0");
    files
        .errors
        .lock()
        .unwrap()
        .insert(paths[0].clone(), io::ErrorKind::PermissionDenied);
    let safari = SafariCookieProvider::new(paths.clone(), files.clone());
    assert!(matches!(
        safari.discover_profiles(&Context::new()),
        Err(BrowserCookieError::PermissionDenied)
    ));
    let query = CookieQuery::new(".fanbox.cc", "FANBOXSESSID").unwrap();
    assert!(matches!(
        safari.read(&canceled, &query, b"Default"),
        Err(BrowserCookieError::PermissionDenied)
    ));
    assert!(matches!(
        safari.read(&canceled, &query, b"default"),
        Err(BrowserCookieError::ProfileNotFound)
    ));
    let before = files.trace.lock().unwrap().len();
    assert!(matches!(
        safari.discover_profiles(&canceled),
        Err(BrowserCookieError::Context(_))
    ));
    assert_eq!(files.trace.lock().unwrap().len(), before);
}

#[test]
fn cancellation_during_file_read_is_not_a_new_provider_discovery_or_safari_read_check() {
    let root = tempfile::tempdir().unwrap();
    duplicate_profiles(root.path());
    let context = Context::new();
    let files = Arc::new(OwnedFiles::default());
    *files.cancel_on_read.lock().unwrap() = Some(context.clone());
    let provider = firefox(root.path(), files.clone(), no_process());
    assert_eq!(provider.discover_profiles(&context).unwrap().len(), 2);
    assert!(context.error().is_some());
    let context = Context::new();
    *files.cancel_on_read.lock().unwrap() = Some(context.clone());
    let path = root.path().join("Cookies.binarycookies");
    write(&path, b"cook\0\0\0\0");
    let provider = SafariCookieProvider::new(vec![path], files);
    assert!(
        provider
            .read(
                &context,
                &CookieQuery::new(".fanbox.cc", "FANBOXSESSID").unwrap(),
                b"Default"
            )
            .unwrap()
            .is_empty()
    );
    assert!(context.error().is_some());
}

#[test]
fn direct_helper_and_go_snapshot_hook_shapes_remain_explicitly_uncompared() {
    let firefox = firefox_fixture();
    for (operation, count) in [
        ("safe_profile_id", 11),
        ("default_root", 2),
        ("read_hook_snapshot", 2),
    ] {
        assert_eq!(
            firefox["cases"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|case| case["operation"] == operation)
                .count(),
            count
        );
    }
    let safari = safari_fixture();
    assert_eq!(
        safari["cases"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|case| case["operation"] == "parse_binary_cookies")
            .count(),
        30
    );
}
