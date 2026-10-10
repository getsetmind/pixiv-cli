#[path = "support/browser_backend.rs"]
mod support;

use pixiv_app::{
    browser_chromium::{
        ChromiumCookieDecoder, ChromiumKeySource, ChromiumKind, PlatformChromiumKeyStore,
    },
    browser_chromium_provider::ChromiumCookieProvider,
    browser_cookies::{
        BrowserCookieBackend, BrowserCookieError, BrowserFiles, CookieQuery, SecretBytes,
        SystemBrowserFiles,
    },
    browser_profiles::FirefoxCookieProvider,
    browser_sqlite::SqliteCommand,
    host_process::HostPlatform,
    lifecycle::{Context, ContextError},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};
use support::{
    FixtureKeys, OwnedEnvironment, ReadProcess, StageFiles, context, decode_hex, provider_fixture,
    read_fields, text, unused_dpapi, write,
};

fn fixture_case<'a>(fixture: &'a Value, id: &str) -> &'a Value {
    fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == id)
        .unwrap()
}

fn provider(
    root: &Path,
    files: Arc<dyn BrowserFiles>,
    process: Arc<ReadProcess>,
    keys: Arc<dyn ChromiumKeySource>,
) -> ChromiumCookieProvider {
    ChromiumCookieProvider::new(
        root.into(),
        files,
        Arc::new(SqliteCommand::new(process)),
        ChromiumCookieDecoder::new(HostPlatform::Linux, keys),
    )
}

fn backend(
    browser: &str,
    home: &Path,
    config: Option<PathBuf>,
    platform: HostPlatform,
    files: Arc<dyn BrowserFiles>,
    process: Arc<ReadProcess>,
) -> BrowserCookieBackend {
    assert_eq!(
        pixiv_app::host_process::HostProcess::platform(process.as_ref()),
        platform
    );
    BrowserCookieBackend::new(
        browser,
        Arc::new(OwnedEnvironment {
            home: Some(home.into()),
            config,
        }),
        files,
        process,
        unused_dpapi(),
    )
    .unwrap()
}

#[test]
fn chromium_public_read_matches_all_twenty_two_actual_go_provider_rows() {
    let fixture = provider_fixture();
    let mut compared = 0;
    let mut constructor_rejections = 0;
    let mut normal_keysource_reads = 0;
    for case in fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["operation"] == "provider_read")
    {
        let root = tempfile::tempdir().unwrap();
        let input = &case["input"];
        let path = root.path().join("Profile 1/Cookies");
        write(
            &root.path().join("Local State"),
            &decode_hex(text(input, "state_body_hex")),
        );
        if input["database_mode"] != "missing" {
            write(&path, b"");
        }
        let context = context(text(input, "context"));
        let process = Arc::new(ReadProcess::from_input(input));
        let files = Arc::new(SystemBrowserFiles);
        let override_keys = FixtureKeys::from_input(input);
        let keys: Arc<dyn ChromiumKeySource> = if input["key_source"] == "linux_secret_tool" {
            normal_keysource_reads += 1;
            Arc::new(PlatformChromiumKeyStore::new(
                ChromiumKind::Chrome,
                root.path().into(),
                files.clone(),
                process.clone(),
                unused_dpapi(),
            ))
        } else {
            override_keys.clone()
        };
        let provider = provider(root.path(), files, process.clone(), keys);
        let query = CookieQuery::new(text(input, "query_host"), text(input, "query_name"));
        let result = match query {
            Ok(query) => {
                compared += 1;
                provider.read(&context, &query, text(input, "profile_id").as_bytes())
            }
            Err(error) => {
                constructor_rejections += 1;
                Err(error)
            }
        };
        let mut observed = read_fields(result);
        observed
            .as_object_mut()
            .unwrap()
            .extend(process.command_fields(&path).as_object().unwrap().clone());
        observed["key_calls"] = json!(override_keys.call_count());
        if input["key_source"] == "linux_secret_tool" {
            observed["secret_command_args"] = process.secret_args();
        }
        assert_eq!(
            observed,
            case["output"],
            "{} actual public Read",
            text(case, "id")
        );
        assert!(
            process
                .calls
                .lock()
                .unwrap()
                .iter()
                .all(|call| call.context == &context as *const Context as usize)
        );
        assert_eq!(provider.close(), Ok(()));
    }
    assert_eq!(compared, 21);
    assert_eq!(constructor_rejections, 1);
    assert_eq!(normal_keysource_reads, 2);
}

#[test]
fn backend_reads_the_same_go_linux_rows_through_normal_owned_state_and_secret_process() {
    let fixture = provider_fixture();
    for id in [
        "read-real-linux-key-and-read",
        "read-real-linux-secret-failure-discards-plaintext",
    ] {
        let case = fixture_case(&fixture, id);
        let input = &case["input"];
        for browser in ["chrome", "edge"] {
            let home = tempfile::tempdir().unwrap();
            let root = home.path().join(".config").join(if browser == "chrome" {
                "google-chrome"
            } else {
                "microsoft-edge"
            });
            let path = root.join("Profile 1/Cookies");
            write(&path, b"");
            write(
                &root.join("Local State"),
                &decode_hex(text(input, "state_body_hex")),
            );
            let process = Arc::new(ReadProcess::from_input(input));
            let backend = backend(
                browser,
                home.path(),
                None,
                HostPlatform::Linux,
                Arc::new(SystemBrowserFiles),
                process.clone(),
            );
            let query =
                CookieQuery::new(text(input, "query_host"), text(input, "query_name")).unwrap();
            let mut observed = read_fields(backend.read(
                &Context::new(),
                &query,
                text(input, "profile_id").as_bytes(),
            ));
            observed
                .as_object_mut()
                .unwrap()
                .extend(process.command_fields(&path).as_object().unwrap().clone());
            observed["key_calls"] = json!(0);
            let mut expected = case["output"].clone();
            if browser == "edge" {
                expected["secret_command_args"] = json!([
                    "lookup",
                    "xdg:schema",
                    "chrome_libsecret",
                    "application",
                    "microsoft-edge"
                ]);
            }
            observed["secret_command_args"] = process.secret_args();
            assert_eq!(observed, expected, "{browser} {id} normal backend");
            assert_eq!(backend.close(), Ok(()));
        }
    }
}

#[test]
fn captured_sources_and_public_read_fixture_remain_sealed() {
    let fixture = provider_fixture();
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for (path, expected) in fixture["sources"].as_object().unwrap() {
        assert_eq!(
            format!(
                "{:x}",
                Sha256::digest(fs::read(repository.join(path)).unwrap())
            ),
            expected.as_str().unwrap(),
            "{path} frozen Go source"
        );
    }
}

#[test]
fn backend_root_recipes_route_owned_profiles_and_fixed_sql_without_host_os_claims() {
    for (platform, browser, recipe) in [
        (HostPlatform::Linux, "chrome", ".config/google-chrome"),
        (HostPlatform::Linux, "edge", ".config/microsoft-edge"),
        (
            HostPlatform::Darwin,
            "chrome",
            "Library/Application Support/Google/Chrome",
        ),
        (
            HostPlatform::Darwin,
            "edge",
            "Library/Application Support/Microsoft Edge",
        ),
        (
            HostPlatform::Windows,
            "chrome",
            "AppData/Local/Google/Chrome/User Data",
        ),
        (
            HostPlatform::Windows,
            "edge",
            "AppData/Local/Microsoft/Edge/User Data",
        ),
        (HostPlatform::Linux, "firefox", ".config/mozilla/firefox"),
        (
            HostPlatform::Darwin,
            "firefox",
            "Library/Application Support/Firefox",
        ),
        (
            HostPlatform::Windows,
            "firefox",
            "AppData/Roaming/Mozilla/Firefox",
        ),
    ] {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join(recipe);
        let firefox = browser == "firefox";
        let path = root.join(if firefox {
            "Default/cookies.sqlite"
        } else {
            "Default/Cookies"
        });
        write(&path, b"");
        if firefox {
            write(&root.join("profiles.ini"), b"[Profile0]\nPath=Default\n");
        }
        let process = Arc::new(ReadProcess::new(
            platform,
            if firefox {
                b"owned-session\n".to_vec()
            } else {
                b".fanbox.cc,owned-session,\n".to_vec()
            },
        ));
        let backend = backend(
            browser,
            home.path(),
            None,
            platform,
            Arc::new(SystemBrowserFiles),
            process.clone(),
        );
        let profiles = backend.discover_profiles(&Context::new()).unwrap();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].id, b"Default");
        assert_eq!(profiles[0].path, root.join("Default"));
        let query = CookieQuery::new(".fanbox.cc", "FANBOXSESSID").unwrap();
        let result = backend.read(&Context::new(), &query, b"Default").unwrap();
        assert_eq!(result[0].as_bytes(), b"owned-session");
        let command = process.command_fields(&path);
        assert_eq!(
            command["sqlite_flags"],
            json!(["-readonly", "-noheader", "-csv", "-newline", "\n"])
        );
        assert_eq!(
            command["sqlite_parameters"],
            json!([
                ".parameter set @h1 .fanbox.cc",
                ".parameter set @h2 fanbox.cc",
                ".parameter set @n FANBOXSESSID"
            ])
        );
        assert_eq!(
            command["sqlite_sql"],
            if firefox {
                "SELECT value FROM moz_cookies WHERE (host = @h1 OR host = @h2) AND name = @n;"
            } else {
                "SELECT host_key, value, hex(encrypted_value) FROM cookies WHERE (host_key = @h1 OR host_key = @h2) AND name = @n;"
            }
        );
        assert_eq!(process.calls.lock().unwrap().len(), 1);
    }
}

#[test]
fn linux_config_home_overrides_only_linux_browser_root_recipes() {
    for (browser, recipe, firefox) in [
        ("chrome", "google-chrome", false),
        ("edge", "microsoft-edge", false),
        ("firefox", "mozilla/firefox", true),
    ] {
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join("owned-xdg");
        let root = config.join(recipe);
        write(
            &root.join(if firefox {
                "Default/cookies.sqlite"
            } else {
                "Default/Cookies"
            }),
            b"",
        );
        if firefox {
            write(&root.join("profiles.ini"), b"[Profile0]\nPath=Default\n");
        }
        let process = Arc::new(ReadProcess::new(HostPlatform::Linux, Vec::new()));
        let backend = backend(
            browser,
            home.path(),
            Some(config),
            HostPlatform::Linux,
            Arc::new(SystemBrowserFiles),
            process.clone(),
        );
        assert_eq!(
            backend.discover_profiles(&Context::new()).unwrap()[0].path,
            root.join("Default")
        );
        assert!(process.calls.lock().unwrap().is_empty());
    }
}

#[test]
fn safari_backend_uses_the_owned_container_then_legacy_candidate_on_each_platform_port() {
    for platform in [
        HostPlatform::Linux,
        HostPlatform::Darwin,
        HostPlatform::Windows,
    ] {
        for legacy in [false, true] {
            let home = tempfile::tempdir().unwrap();
            let first = home.path().join(
                "Library/Containers/com.apple.Safari/Data/Library/Cookies/Cookies.binarycookies",
            );
            let second = home.path().join("Library/Cookies/Cookies.binarycookies");
            let path = if legacy {
                fs::create_dir_all(&first).unwrap();
                &second
            } else {
                &first
            };
            write(path, b"cook\0\0\0\0");
            let process = Arc::new(ReadProcess::new(platform, Vec::new()));
            let backend = backend(
                "safari",
                home.path(),
                Some(home.path().join("unused-xdg")),
                platform,
                Arc::new(SystemBrowserFiles),
                process.clone(),
            );
            let profiles = backend.discover_profiles(&Context::new()).unwrap();
            assert_eq!(profiles.len(), 1);
            assert_eq!(profiles[0].id, b"Default");
            assert_eq!(profiles[0].name, b"Default");
            assert_eq!(&profiles[0].path, path);
            assert!(
                backend
                    .read(
                        &Context::new(),
                        &CookieQuery::new(".fanbox.cc", "FANBOXSESSID").unwrap(),
                        b"Default"
                    )
                    .unwrap()
                    .is_empty()
            );
            assert!(process.calls.lock().unwrap().is_empty());
        }
    }
}

#[test]
fn unavailable_home_and_unsupported_platform_keep_empty_go_roots_with_no_real_profile_reads() {
    for unavailable_home in [false, true] {
        for browser in ["chrome", "edge", "firefox"] {
            let home = tempfile::tempdir().unwrap();
            let files = Arc::new(StageFiles::default());
            files.error("read_dir", Path::new(""), io::ErrorKind::NotFound);
            files.error(
                "open_file",
                Path::new("profiles.ini"),
                io::ErrorKind::NotFound,
            );
            let platform = if unavailable_home {
                HostPlatform::Linux
            } else {
                HostPlatform::Unsupported("owned-unsupported")
            };
            let process = Arc::new(ReadProcess::new(platform, Vec::new()));
            let backend = BrowserCookieBackend::new(
                browser,
                Arc::new(OwnedEnvironment {
                    home: (!unavailable_home).then(|| home.path().into()),
                    config: Some(home.path().join("owned-xdg")),
                }),
                files.clone(),
                process.clone(),
                unused_dpapi(),
            )
            .unwrap();
            assert_eq!(
                backend.discover_profiles(&Context::new()).unwrap_err(),
                BrowserCookieError::NotInstalled
            );
            assert_eq!(
                files.trace.lock().unwrap().as_slice(),
                &[(
                    if browser == "firefox" {
                        "open_file".into()
                    } else {
                        "read_dir".into()
                    },
                    if browser == "firefox" {
                        PathBuf::from("profiles.ini")
                    } else {
                        PathBuf::new()
                    },
                )]
            );
            assert!(process.calls.lock().unwrap().is_empty());
        }
    }
    let home = tempfile::tempdir().unwrap();
    for browser in ["", "Chrome-beta", "unknown"] {
        let process = Arc::new(ReadProcess::new(HostPlatform::Linux, Vec::new()));
        assert_eq!(
            BrowserCookieBackend::new(
                browser,
                Arc::new(OwnedEnvironment {
                    home: Some(home.path().into()),
                    config: None
                }),
                Arc::new(SystemBrowserFiles),
                process.clone(),
                unused_dpapi()
            )
            .err()
            .unwrap(),
            BrowserCookieError::UnknownBrowser
        );
        assert!(process.calls.lock().unwrap().is_empty());
    }
}

#[test]
fn chromium_discovery_uses_owned_byte_ids_sorted_names_and_only_cookie_files() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join(".config/google-chrome");
    for id in ["Default", "Profile 1", ".hidden", "Cache"] {
        write(
            &root
                .join(id)
                .join(if id == "Cache" { "data" } else { "Cookies" }),
            b"",
        );
    }
    fs::create_dir_all(root.join("CookieDirectory/Cookies")).unwrap();
    write(&root.join("LooseFile"), b"");
    let process = Arc::new(ReadProcess::new(
        HostPlatform::Linux,
        b".fanbox.cc,owned-byte-session,\n".to_vec(),
    ));
    let backend = backend(
        "chrome",
        home.path(),
        None,
        HostPlatform::Linux,
        Arc::new(SystemBrowserFiles),
        process.clone(),
    );
    let profiles = backend.discover_profiles(&Context::new()).unwrap();
    assert_eq!(
        profiles
            .iter()
            .map(|profile| profile.id.as_slice())
            .collect::<Vec<_>>(),
        [b"Default".as_slice(), b"Profile 1".as_slice()]
    );
    for profile in profiles {
        assert_eq!(profile.name, profile.id);
        assert_eq!(
            profile.path,
            root.join(std::str::from_utf8(&profile.id).unwrap())
        );
        assert_eq!(format!("{profile:?}"), "BrowserProfile(<redacted>)");
    }
    assert!(process.calls.lock().unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn chromium_non_utf8_profile_ids_keep_native_bytes_through_discovery_and_query_path() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join(".config/google-chrome");
    let id = b"Profile\xff";
    let path = root.join(OsString::from_vec(id.to_vec())).join("Cookies");
    write(&path, b"");
    let process = Arc::new(ReadProcess::new(
        HostPlatform::Linux,
        b".fanbox.cc,owned-byte-session,\n".to_vec(),
    ));
    let backend = backend(
        "chrome",
        home.path(),
        None,
        HostPlatform::Linux,
        Arc::new(SystemBrowserFiles),
        process.clone(),
    );
    let profiles = backend.discover_profiles(&Context::new()).unwrap();
    assert_eq!(profiles[0].id, id);
    assert_eq!(profiles[0].name, id);
    assert_eq!(profiles[0].path, path.parent().unwrap());
    assert_eq!(
        backend
            .read(
                &Context::new(),
                &CookieQuery::new(".fanbox.cc", "FANBOXSESSID").unwrap(),
                id
            )
            .unwrap()[0]
            .as_bytes(),
        b"owned-byte-session"
    );
    assert_eq!(process.calls.lock().unwrap()[0].args[11], path.as_os_str());
}

#[test]
fn browser_files_stage_errors_keep_discovery_and_read_priority_before_cancellation() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join(".config/google-chrome");
    let path = root.join("Default/Cookies");
    write(&path, b"");
    let files = Arc::new(StageFiles::default());
    let process = Arc::new(ReadProcess::new(HostPlatform::Linux, Vec::new()));
    let backend = backend(
        "chrome",
        home.path(),
        None,
        HostPlatform::Linux,
        files.clone(),
        process.clone(),
    );
    let canceled = context("canceled");
    let query = CookieQuery::new(".fanbox.cc", "FANBOXSESSID").unwrap();
    files.error("read_dir", &root, io::ErrorKind::PermissionDenied);
    assert_eq!(
        backend.discover_profiles(&Context::new()).unwrap_err(),
        BrowserCookieError::PermissionDenied
    );
    let before = files.trace.lock().unwrap().len();
    assert_eq!(
        backend.discover_profiles(&canceled).unwrap_err(),
        BrowserCookieError::Context(ContextError::Canceled)
    );
    assert_eq!(files.trace.lock().unwrap().len(), before);
    files.errors.lock().unwrap().clear();
    files.error("metadata", &path, io::ErrorKind::PermissionDenied);
    assert_eq!(
        backend.discover_profiles(&Context::new()).unwrap_err(),
        BrowserCookieError::PermissionDenied
    );
    assert_eq!(
        backend.read(&canceled, &query, b"Default").unwrap_err(),
        BrowserCookieError::PermissionDenied
    );
    let before = files.trace.lock().unwrap().len();
    for id in [
        b"".as_slice(),
        b".",
        b"..",
        b".hidden",
        b"../private",
        b"other/private",
    ] {
        assert_eq!(
            backend.read(&canceled, &query, id).unwrap_err(),
            BrowserCookieError::InvalidProfileId
        );
    }
    assert_eq!(files.trace.lock().unwrap().len(), before);
    for kind in [io::ErrorKind::NotFound, io::ErrorKind::Other] {
        files.error("metadata", &path, kind);
        assert_eq!(
            backend.read(&canceled, &query, b"Default").unwrap_err(),
            BrowserCookieError::DatabaseNotFound
        );
        assert_eq!(
            backend.discover_profiles(&Context::new()).unwrap_err(),
            BrowserCookieError::NotInstalled
        );
    }
    assert!(process.calls.lock().unwrap().is_empty());
}

#[test]
fn firefox_open_and_late_read_errors_use_distinct_real_reader_stages() {
    let root = tempfile::tempdir().unwrap();
    let ini = root.path().join("profiles.ini");
    write(&ini, b"[Profile0]\nPath=Default\n");
    write(&root.path().join("Default/cookies.sqlite"), b"");
    for (stage, kind, expected) in [
        (
            "open_file",
            io::ErrorKind::NotFound,
            BrowserCookieError::NotInstalled,
        ),
        (
            "open_file",
            io::ErrorKind::Other,
            BrowserCookieError::NotInstalled,
        ),
        (
            "open_file",
            io::ErrorKind::PermissionDenied,
            BrowserCookieError::PermissionDenied,
        ),
        (
            "late_read",
            io::ErrorKind::Other,
            BrowserCookieError::InvalidFormat,
        ),
        (
            "late_read",
            io::ErrorKind::PermissionDenied,
            BrowserCookieError::PermissionDenied,
        ),
    ] {
        let files = Arc::new(StageFiles::default());
        files.error(stage, &ini, kind);
        let process = Arc::new(ReadProcess::new(HostPlatform::Linux, Vec::new()));
        let provider = FirefoxCookieProvider::new(
            root.path().into(),
            files.clone(),
            Arc::new(SqliteCommand::new(process.clone())),
        );
        assert_eq!(
            provider.discover_profiles(&Context::new()).unwrap_err(),
            expected,
            "{stage} {kind:?}"
        );
        assert_eq!(
            files.trace.lock().unwrap().as_slice(),
            &[("open_file".into(), ini.clone())]
        );
        assert!(process.calls.lock().unwrap().is_empty());
    }
}

#[test]
fn system_browser_files_report_owned_file_and_directory_reads_without_early_projection() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("owned-file");
    write(&file, b"owned bytes\0\xff");
    let files = SystemBrowserFiles;
    assert_eq!(files.read_file(&file).unwrap(), b"owned bytes\0\xff");
    let mut bytes = Vec::new();
    files
        .open_file(&file)
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes, b"owned bytes\0\xff");
    assert_eq!(
        files
            .open_file(&root.path().join("missing"))
            .err()
            .unwrap()
            .kind(),
        io::ErrorKind::NotFound
    );
    assert_eq!(
        files.read_file(root.path()).unwrap_err().kind(),
        io::ErrorKind::IsADirectory
    );
    #[cfg(unix)]
    {
        let mut reader = files.open_file(root.path()).unwrap();
        assert_eq!(
            reader.read_to_end(&mut bytes).unwrap_err().kind(),
            io::ErrorKind::IsADirectory
        );
    }
}

#[test]
fn query_constructor_and_secret_rendering_preserve_the_shared_go_constraints() {
    for value in [
        "",
        " ",
        "x/y",
        "x'y",
        "x;y",
        "x\ny",
        "x@y",
        "x:y",
        "日本語",
        &"x".repeat(257),
    ] {
        assert_eq!(
            CookieQuery::new(value, "FANBOXSESSID").unwrap_err(),
            BrowserCookieError::QueryInvalid
        );
        assert_eq!(
            CookieQuery::new(".fanbox.cc", value).unwrap_err(),
            BrowserCookieError::QueryInvalid
        );
    }
    for value in [".fanbox.cc", "FANBOXSESSID", "a_A-0.Z", &"x".repeat(256)] {
        let query = CookieQuery::new(value, value).unwrap();
        assert_eq!(query.host(), value);
        assert_eq!(query.name(), value);
    }
    let secret = SecretBytes::new(b"owned-private-cookie\0\xff".to_vec());
    assert_eq!(secret.to_string(), "<redacted>");
    assert_eq!(format!("{secret:?}"), "<redacted>");
    assert_eq!(format!("{secret:#?}"), "<redacted>");
    assert_eq!(serde_json::to_string(&secret).unwrap(), "\"<redacted>\"");
    assert_eq!(secret.as_bytes(), b"owned-private-cookie\0\xff");
    assert_eq!(secret.into_bytes(), b"owned-private-cookie\0\xff");
}
