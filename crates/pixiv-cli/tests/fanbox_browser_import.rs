#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

#[allow(dead_code)]
#[path = "fanbox_download_support/isolation.rs"]
mod binary_network_isolation;
#[allow(dead_code)]
mod fanbox_auth_support;
mod fanbox_browser_import_support;

use fanbox_browser_import_support::{isolation, observe, owned, schema::Case};
use pixiv_app::host_process::{HostProcess, SystemHostProcess};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    ffi::OsStr,
    path::{Path, PathBuf},
    process::Command,
};

const FIXTURE: &[u8] = include_bytes!("fixtures/fanbox-browser-import.json");
const SEAL: &str = "846fda57f94a9ba52d42e1e9e6fc28cd49f9ac437fe9527a37238f3cb630c793";
const PRODUCER_SHA: &str = "e7a71f87dd6e353b8f38c1bc3d51c384937497060c9e5f63b88291b0b021fd64";
const CHILD: &str = "PIXIV_FANBOX_BROWSER_IMPORT_CONTRACT_CHILD";
const SQLITE: &str = "PIXIV_FANBOX_BROWSER_SQLITE_SHELL";
const FIELDS: &[&str] = &[
    "exit",
    "stdout",
    "stderr",
    "output_writes",
    "error_writes",
    "trace",
    "requests",
    "stdin_reads",
    "stdin_bytes",
    "before",
    "after",
    "socket_denied",
    "exec_denied",
    "commands",
    "native_session_hex",
    "native_session_error",
];
const STATE_FIELDS: &[&str] = &[
    "config",
    "database",
    "user_version",
    "application_id",
    "rows",
    "modes",
];
fn fixture() -> Value {
    assert_eq!(
        format!("{:x}", Sha256::digest(FIXTURE)),
        SEAL,
        "connected Go fixture seal"
    );
    serde_json::from_slice(FIXTURE).unwrap()
}
fn keys(value: &Value) -> BTreeSet<&str> {
    value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect()
}
fn field_set(fields: &[&'static str]) -> BTreeSet<&'static str> {
    fields.iter().copied().collect()
}
fn pinned_sqlite(fixture: &Value) -> PathBuf {
    let path = std::env::var_os(SQLITE).map(PathBuf::from).unwrap_or_else(|| {
        SystemHostProcess.look_path(OsStr::new("sqlite3")).expect("set PIXIV_FANBOX_BROWSER_SQLITE_SHELL or place the pinned official SQLite shell on PATH")
    });
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(
        format!("{:x}", Sha256::digest(&bytes)),
        fixture["sqlite_tool"]["sha256"].as_str().unwrap(),
        "pinned SQLite bytes"
    );
    path
}

#[test]
fn connected_browser_fixture_sources_and_every_observation_field_are_sealed() {
    let fixture = fixture();
    assert_eq!(
        fixture["reference"],
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    assert_eq!(fixture["environment"], "linux/amd64");
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for section in ["sources", "support_sources", "reused_fixtures"] {
        for (path, digest) in fixture[section].as_object().unwrap() {
            let bytes = std::fs::read(repository.join(path)).unwrap();
            assert_eq!(
                format!("{:x}", Sha256::digest(&bytes)),
                digest.as_str().unwrap(),
                "sealed {section} {path}"
            );
        }
    }
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(
                std::fs::read(repository.join("internal/cli/migration_fanbox_browser_test.go"))
                    .unwrap()
            )
        ),
        PRODUCER_SHA,
        "genuine connected producer seal"
    );
    for section in ["dependencies", "dependency_sources", "stdlib_sources"] {
        assert!(!fixture[section].as_object().unwrap().is_empty());
    }
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 82);
    let mut names = BTreeSet::new();
    let mut observations = 0;
    for row in fixture["cases"].as_array().unwrap() {
        let case: Case = serde_json::from_value(row.clone()).unwrap();
        assert!(names.insert(case.name.clone()));
        assert_eq!(
            case.steps.len(),
            case.observations.len(),
            "{} step count",
            case.name
        );
        assert_eq!(keys(&case.discovery), field_set(&["profiles", "error"]));
        for profile in case.discovery["profiles"].as_array().unwrap() {
            assert_eq!(keys(profile), field_set(&["ID", "Name", "Path"]));
        }
        for observation in &case.observations {
            assert_eq!(
                keys(observation),
                field_set(FIELDS),
                "{} observation coverage",
                case.name
            );
            assert_eq!(observation["socket_denied"], true);
            assert_eq!(observation["exec_denied"], false);
            for state in ["before", "after"] {
                assert_eq!(keys(&observation[state]), field_set(STATE_FIELDS));
            }
            for command in observation["commands"].as_array().unwrap() {
                assert_eq!(
                    keys(command),
                    if command["program"] == "sqlite3" {
                        field_set(&["program", "args", "parameters"])
                    } else {
                        assert_eq!(command["program"], "secret-tool");
                        field_set(&["program", "args"])
                    }
                );
            }
            observations += 1;
        }
    }
    assert_eq!(observations, 84);
    let first = |name: &str| -> &Value {
        &fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["name"] == name)
            .unwrap()["observations"][0]
    };
    for (name, hex) in [
        ("chrome/matched-invalid-utf8", "78ff79"),
        ("edge/matched-invalid-utf8", "78ff79"),
        ("chrome/decrypted-invalid-utf8", "78ff79"),
        ("chrome/decrypted-nul", "780079"),
        ("chrome/raw-byte-order-factory", "78ff0079"),
        ("chrome/raw-byte-order-proxy", "78ff0079"),
        ("chrome/raw-byte-order-options", "78ff0079"),
        ("chrome/raw-byte-order-identity", "78ff0079"),
    ] {
        assert_eq!(first(name)["native_session_hex"], hex);
        assert_eq!(first(name)["native_session_error"], "");
        assert_eq!(first(name)["exit"], 1);
        assert!(first(name)["requests"].as_array().unwrap().is_empty());
    }
    assert_eq!(
        first("chrome/plaintext-nul-shell-projection")["native_session_hex"],
        "78"
    );
    assert_eq!(
        first("chrome/plaintext-nul-shell-projection")["after"]["rows"][0]["session_hex"],
        "78"
    );
    assert_eq!(
        first("chrome/each-encrypted-row-acquires-password")["commands"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        first("chrome/pre-canceled-discovery-no-command")["native_session_error"],
        "context canceled"
    );
    assert!(
        first("chrome/pre-canceled-discovery-no-command")["commands"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn actual_root_native_browser_sdk_and_saved_state_match_all_84_go_observations() {
    if std::env::var_os(CHILD).is_some() {
        return;
    }
    let fixture = fixture();
    let sqlite = pinned_sqlite(&fixture);
    let home = tempfile::tempdir().unwrap();
    let temp = home.path().join("temp");
    std::fs::create_dir(&temp).unwrap();
    let owned_sqlite = home.path().join("sqlite3-official");
    std::fs::copy(sqlite, &owned_sqlite).unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap());
    child
        .args([
            "--exact",
            "owned_connected_fanbox_browser_import_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env_clear()
        .env(CHILD, "1")
        .env(SQLITE, &owned_sqlite)
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("TMPDIR", &temp)
        .env("TMP", &temp)
        .env("TEMP", &temp)
        .env("XDG_CONFIG_HOME", home.path().join("xdg-config"))
        .env("XDG_DATA_HOME", home.path().join("xdg-data"))
        .env("XDG_CACHE_HOME", home.path().join("xdg-cache"))
        .env("XDG_STATE_HOME", home.path().join("xdg-state"))
        .env("XDG_RUNTIME_DIR", home.path().join("xdg-runtime"))
        .env("APPDATA", home.path().join("appdata"))
        .env("LOCALAPPDATA", home.path().join("local-appdata"))
        .env("PATH", "")
        .env("TZ", "UTC")
        .current_dir(home.path());
    for variable in ["LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH"] {
        if let Some(value) = std::env::var_os(variable) {
            child.env(variable, value);
        }
    }
    let output = child.output().unwrap();
    assert!(
        output.status.success(),
        "connected owned child failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn owned_connected_fanbox_browser_import_child() {
    if std::env::var_os(CHILD).is_none() {
        return;
    }
    let fixture = fixture();
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    assert_eq!(std::env::var_os("USERPROFILE").unwrap(), home.as_os_str());
    assert_eq!(std::env::temp_dir(), home.join("temp"));
    let sqlite = pinned_sqlite(&fixture);
    assert!(sqlite.starts_with(&home));
    let sqlite_bytes = std::fs::read(sqlite).unwrap();
    let network_denied = isolation::deny_network();
    let mut failures = Vec::new();
    let mut executions = 0;
    let mut scenarios = 0;
    for (index, row) in fixture["cases"].as_array().unwrap().iter().enumerate() {
        let case: Case = serde_json::from_value(row.clone()).unwrap();
        let case_home = home.join(format!("case-{index:03}"));
        let (discovery, observed) = observe(&case_home, &case, &sqlite_bytes, network_denied).await;
        if scenarios == 0 {
            let version = owned::Process::new(&case_home).verify_sqlite();
            assert_eq!(
                std::str::from_utf8(&version.stdout).unwrap().trim(),
                fixture["sqlite_tool"]["version"].as_str().unwrap()
            );
            assert!(version.stderr.is_empty());
        }
        if discovery != case.discovery {
            failures.push(format!(
                "{}.discovery\n Rust: {discovery}\n Go: {}",
                case.name, case.discovery
            ));
        }
        assert_eq!(
            observed.len(),
            case.observations.len(),
            "{} observations",
            case.name
        );
        for (step, (actual, expected)) in observed.iter().zip(&case.observations).enumerate() {
            assert_eq!(keys(actual), field_set(FIELDS));
            for canary in [
                home.to_str().unwrap(),
                "/owned-private-path-canary",
                "/owned-private-profile-canary",
                "owned-private-secret-canary",
                "owned-native-session",
                "owned-decrypted-session",
                "owned-password",
                "plaintext-private-canary",
                "synthetic-browser-secret",
                "synthetic-seed-",
                "opaque-secret-canary",
            ] {
                for field in ["stdout", "stderr"] {
                    assert!(
                        !actual[field].as_str().unwrap().contains(canary),
                        "{}[{step}] private source leaked",
                        case.name
                    );
                }
            }
            for field in FIELDS {
                if actual[*field] != expected[*field] {
                    failures.push(format!(
                        "{}[{step}].{field}\n Rust: {}\n Go: {}",
                        case.name, actual[*field], expected[*field]
                    ));
                }
            }
            executions += 1;
        }
        scenarios += 1;
    }
    assert_eq!(scenarios, 82);
    assert_eq!(executions, 84);
    assert!(
        failures.is_empty(),
        "{} connected differences:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn main_dispatches_the_native_browser_through_the_normal_auth_root() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let main = std::fs::read_to_string(source.join("main.rs")).unwrap();
    let browser = std::fs::read_to_string(source.join("fanbox_browser.rs")).unwrap();
    assert!(main.contains("fanbox_auth::prepare_root"));
    assert!(main.contains("fanbox_auth::Data"));
    assert!(main.contains("SystemBrowserProvider::system"));
    assert!(browser.contains("BrowserCookieBackend::system(browser)"));
    assert!(!browser.contains("native browser cookie extraction is not implemented"));
    assert!(!browser.contains("#[cfg(test)]"));
}

#[test]
fn actual_binary_system_factory_rejects_owned_firefox_bytes_before_identity() {
    if std::env::var_os(CHILD).is_some() {
        return;
    }
    use std::os::unix::{ffi::OsStringExt, fs::PermissionsExt, process::CommandExt};
    let fixture = fixture();
    let row = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == "firefox/matched-invalid-utf8")
        .unwrap();
    let case: Case = serde_json::from_value(row.clone()).unwrap();
    let sqlite = std::fs::read(pinned_sqlite(&fixture)).unwrap();
    let home = tempfile::tempdir().unwrap();
    owned::prepare(home.path(), &case, &sqlite);
    let bin = home.path().join("bin");
    std::fs::rename(bin.join("sqlite3"), bin.join("sqlite3-official")).unwrap();
    let wrapper = bin.join("sqlite3");
    std::fs::write(
        &wrapper,
        br#"#!/bin/sh
printf '%s\0' "$@" > "$HOME/sqlite-argv"
exec "$HOME/bin/sqlite3-official" "$@"
"#,
    )
    .unwrap();
    std::fs::set_permissions(wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_pixiv"));
    command
        .args(&case.steps[0].args)
        .env_clear()
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("XDG_CONFIG_HOME", home.path().join("xdg-config"))
        .env("XDG_DATA_HOME", home.path().join("xdg-data"))
        .env("XDG_CACHE_HOME", home.path().join("xdg-cache"))
        .env("XDG_STATE_HOME", home.path().join("xdg-state"))
        .env("XDG_RUNTIME_DIR", home.path().join("xdg-runtime"))
        .env("APPDATA", home.path().join("appdata"))
        .env("LOCALAPPDATA", home.path().join("local-appdata"))
        .env("TMPDIR", home.path().join("temp"))
        .env("TMP", home.path().join("temp"))
        .env("TEMP", home.path().join("temp"))
        .env("PATH", &bin)
        .env("TZ", "UTC")
        .current_dir(home.path())
        .stdin(std::process::Stdio::null());
    for variable in ["LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH"] {
        if let Some(value) = std::env::var_os(variable) {
            command.env(variable, value);
        }
    }
    let filter = binary_network_isolation::network_filter();
    unsafe {
        command.pre_exec(move || binary_network_isolation::apply_network_filter(&filter));
    }
    let output = command.output().unwrap();
    let expected = &case.observations[0];
    assert_eq!(
        output.status.code().unwrap(),
        expected["exit"].as_i64().unwrap() as i32
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        expected["stdout"].as_str().unwrap()
    );
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        expected["stderr"].as_str().unwrap()
    );
    assert!(
        !home.path().join(".pixiv-cli").exists(),
        "binary created account/config state before provider rejection"
    );
    let logged = std::fs::read(home.path().join("sqlite-argv")).unwrap();
    assert_eq!(logged.last(), Some(&0));
    let args = logged[..logged.len() - 1]
        .split(|byte| *byte == 0)
        .map(|arg| std::ffi::OsString::from_vec(arg.to_vec()))
        .collect::<Vec<_>>();
    let actual = owned::command_observation(home.path(), OsStr::new("sqlite3"), &args);
    assert_eq!(serde_json::json!([actual]), expected["commands"]);
}
