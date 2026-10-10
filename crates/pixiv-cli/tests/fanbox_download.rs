#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

mod fanbox_download_support;

use fanbox_download_support::{observe, schema::Input};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::File,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const FIXTURE: &[u8] = include_bytes!("fixtures/fanbox-download.json");
const SEALED_SHA: &str = "0b2793d3c2adb538e1a6951c0308c0cbb8078063ccfe7f08849f24be72d1c94e";
const CHILD: &str = "PIXIV_FANBOX_DOWNLOAD_CONTRACT_CHILD";
const OBSERVATION_FIELDS: &[&str] = &[
    "exits",
    "stdout",
    "stderr",
    "errors",
    "reasons",
    "trace",
    "requests",
    "options",
    "proxy_overrides",
    "stdin_reads",
    "writes",
    "body_closes",
    "lease_closes",
    "unique_clients",
    "output_is_tty",
    "auto_ndjson",
    "idle_closes",
    "db_before",
    "db_after",
    "db_rows_before",
    "db_rows_after",
    "config_after",
    "remaining_temps",
    "socket_denied",
    "exec_denied",
    "files_before",
    "files_after",
    "body_reads",
    "output_writes",
    "reply_count",
];
fn fixture() -> Value {
    assert_eq!(
        format!("{:x}", Sha256::digest(FIXTURE)),
        SEALED_SHA,
        "Go download fixture seal"
    );
    serde_json::from_slice(FIXTURE).unwrap()
}

#[test]
fn sealed_fanbox_download_source_schema_and_all_183_genuine_root_rows_are_explicit() {
    let document = fixture();
    assert_eq!(
        document["reference"],
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    let cases = document["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 183);
    let fields = OBSERVATION_FIELDS.iter().copied().collect::<BTreeSet<_>>();
    let mut names = BTreeSet::new();
    let mut executions = 0;
    for row in cases {
        assert!(names.insert(row["name"].as_str().unwrap()));
        let input: Input = serde_json::from_value(row["input"].clone()).unwrap();
        assert_eq!(input.args[0], "fanbox");
        executions += input.repeat.max(1);
        assert_eq!(
            row["observation"]
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            fields,
            "{} unclassified observation",
            row["name"]
        );
        assert_eq!(
            row["observation"]["exits"].as_array().unwrap().len(),
            input.repeat.max(1)
        );
        assert_eq!(row["observation"]["socket_denied"], true);
        assert_eq!(row["observation"]["exec_denied"], true);
        assert_eq!(row["observation"]["errors"], serde_json::json!([]));
        assert_eq!(row["observation"]["reasons"], serde_json::json!([]));
        assert_eq!(
            row["observation"]["db_rows_before"],
            row["observation"]["db_rows_after"]
        );
        assert_eq!(row["observation"]["config_after"], input.config);
    }
    assert_eq!(executions, 184);
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for section in ["sources", "frozen_go_production", "published_fixtures"] {
        for (path, digest) in document[section].as_object().unwrap() {
            assert_eq!(
                format!(
                    "{:x}",
                    Sha256::digest(std::fs::read(repository.join(path)).unwrap())
                ),
                digest.as_str().unwrap(),
                "sealed {section} {path}"
            );
        }
    }
    assert_eq!(
        document["frozen_go_production"].as_object().unwrap().len(),
        434
    );
    assert_eq!(
        document["published_fixtures"].as_object().unwrap().len(),
        95
    );
}

#[test]
fn saved_fanbox_download_matches_all_184_go_root_executions_in_owned_child() {
    if std::env::var_os(CHILD).is_some() {
        return;
    }
    let home = tempfile::tempdir().unwrap();
    let temp = home.path().join("temp");
    std::fs::create_dir(&temp).unwrap();
    let stdout_path = home.path().join("child.stdout");
    let stderr_path = home.path().join("child.stderr");
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "owned_fanbox_download_contract_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env_clear()
        .env(CHILD, "1")
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
        .env("TZ", "Asia/Tokyo")
        .current_dir(home.path())
        .stdin(Stdio::null())
        .stdout(File::create(&stdout_path).unwrap())
        .stderr(File::create(&stderr_path).unwrap());
    for variable in ["LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH", "SYSTEMROOT"] {
        if let Some(value) = std::env::var_os(variable) {
            command.env(variable, value);
        }
    }
    let mut child = command.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!(
                "owned FANBOX download child timed out:\n{}\n{}",
                std::fs::read_to_string(&stdout_path).unwrap(),
                std::fs::read_to_string(&stderr_path).unwrap()
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let stdout = std::fs::read_to_string(stdout_path).unwrap();
    let stderr = std::fs::read_to_string(stderr_path).unwrap();
    print!("{stdout}");
    assert!(
        status.success(),
        "owned FANBOX download child failed:\n{stdout}\n{stderr}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn owned_fanbox_download_contract_child() {
    if std::env::var_os(CHILD).is_none() {
        return;
    }
    let home = std::env::var_os("HOME").unwrap();
    let home = Path::new(&home);
    assert_eq!(std::env::var_os("USERPROFILE").unwrap(), home.as_os_str());
    assert_eq!(std::env::current_dir().unwrap(), home);
    assert_eq!(std::env::temp_dir(), home.join("temp"));
    unsafe {
        libc::umask(0o022);
    }
    let denied = fanbox_download_support::isolation::deny_external();
    let document = fixture();
    let mut failures = Vec::new();
    let mut executions = 0;
    let mut sqlite_hashes = BTreeSet::new();
    for row in document["cases"].as_array().unwrap() {
        let name = row["name"].as_str().unwrap();
        let input: Input = serde_json::from_value(row["input"].clone()).unwrap();
        let observed = observe(home, input.clone(), denied).await;
        let mut actual = serde_json::to_value(&observed).unwrap();
        let expected = &row["observation"];
        sqlite_hashes.insert((
            actual["db_before"].as_str().unwrap().to_owned(),
            actual["db_after"].as_str().unwrap().to_owned(),
            expected["db_before"].as_str().unwrap().to_owned(),
            expected["db_after"].as_str().unwrap().to_owned(),
        ));
        project_native_writer_error(&mut actual, expected);
        if name == "database-corrupt" {
            project_database_diagnostic(name, &mut actual, expected);
        }
        for field in OBSERVATION_FIELDS {
            match *field {
                "db_before" | "db_after" => {
                    assert_eq!(
                        actual[field].as_str().unwrap().len(),
                        64,
                        "{name} actual SQLite file hash"
                    );
                    assert_eq!(
                        expected[field].as_str().unwrap().len(),
                        64,
                        "{name} Go SQLite file hash"
                    );
                    if input.db_failure == "corrupt" {
                        assert_eq!(
                            actual[field], expected[field],
                            "{name} unchanged corrupt bytes"
                        );
                    }
                }
                "db_rows_before" | "db_rows_after" if !input.db_failure.is_empty() => {
                    compare_driver_rows(
                        name,
                        field,
                        &actual[field],
                        &expected[field],
                        &mut failures,
                    )
                }
                _ if actual[field] != expected[field] => failures.push(format!(
                    "{name}.{field}\n  Rust: {}\n  Go:   {}",
                    actual[field], expected[field]
                )),
                _ => (),
            }
        }
        assert_eq!(
            observed.db_rows_before, observed.db_rows_after,
            "{name} saved rows changed"
        );
        assert_eq!(
            observed.config_after, input.config,
            "{name} saved config changed"
        );
        assert!(
            observed.remaining_temps.is_empty(),
            "{name} temporary output survived"
        );
        for canary in ["owned-session", "secret-session", home.to_str().unwrap()] {
            assert!(
                !observed.stdout.contains(canary) && !observed.stderr.contains(canary),
                "{name} private data leaked"
            );
        }
        for write in &observed.output_writes {
            if let Some(path) = write["bytes"].as_str().unwrap().strip_prefix("saved: ") {
                let path = Path::new(path.trim_end_matches('\n'));
                let normalized = path
                    .components()
                    .filter_map(|component| match component {
                        std::path::Component::CurDir => None,
                        component => Some(component.as_os_str()),
                    })
                    .collect::<std::path::PathBuf>();
                assert!(
                    observed
                        .files_after
                        .iter()
                        .any(|file| file.kind == "file" && Path::new(&file.path) == normalized),
                    "{name} saved output has no published file"
                );
            }
        }
        executions += input.repeat.max(1);
    }
    assert_eq!(executions, 184);
    for (before, after, go_before, go_after) in sqlite_hashes {
        println!(
            "Go/Rust physical SQLite hash projection: Rust={before}/{after} Go={go_before}/{go_after}"
        );
    }
    println!(
        "SQLite physical hashes retained per observation; cross-driver PRAGMA/layout is classified separately from exact saved rows, config and file output"
    );
    assert!(
        failures.is_empty(),
        "{} exact FANBOX download differences:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
fn project_native_writer_error(actual: &mut Value, expected: &Value) {
    for (actual, expected) in actual["output_writes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .zip(expected["output_writes"].as_array().unwrap())
    {
        if expected["error"] == "broken pipe" {
            assert_eq!(
                actual["error"],
                std::io::Error::from_raw_os_error(32).to_string(),
                "actual EPIPE diagnostic"
            );
            actual["error"] = "broken pipe".into();
        }
    }
}
fn project_database_diagnostic(name: &str, actual: &mut Value, expected: &Value) {
    assert_eq!(
        actual["stderr"], "error: file is not a database\n",
        "{name} actual Rust driver diagnostic"
    );
    assert_eq!(
        expected["stderr"], "error: database: ping database: file is not a database (26)\n",
        "{name} fixed Go driver diagnostic"
    );
    println!(
        "Go/Rust driver diagnostic {name}: Rust={} Go={}",
        actual["stderr"], expected["stderr"]
    );
    actual["stderr"] = expected["stderr"].clone();
}
fn compare_driver_rows(
    name: &str,
    field: &str,
    actual: &Value,
    expected: &Value,
    failures: &mut Vec<String>,
) {
    let actual = actual.as_array().unwrap();
    let expected = expected.as_array().unwrap();
    assert_eq!(
        actual.len(),
        expected.len(),
        "{name}.{field} driver query count"
    );
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        if expected.as_str().unwrap().starts_with('[') {
            if actual != expected {
                failures.push(format!(
                    "{name}.{field}[{index}] Rust={actual} Go={expected}"
                ));
            }
        } else {
            match name {
                "database-corrupt" => {
                    assert_eq!(actual, "file is not a database");
                    assert_eq!(expected, "file is not a database (26)");
                }
                "database-missing-table" => {
                    assert_eq!(actual, "no such table: fanbox_account");
                    assert_eq!(
                        expected,
                        "SQL logic error: no such table: fanbox_account (1)"
                    );
                }
                _ => panic!("unclassified driver row {name}.{field}[{index}]"),
            }
            println!(
                "Go/Rust query diagnostic {name}.{field}[{index}]: Rust={actual} Go={expected}"
            );
        }
    }
}

#[test]
fn real_binary_preserves_17_bounded_download_flag_help_stdin_and_saved_account_stops() {
    if std::env::var_os(CHILD).is_some() {
        return;
    }
    let document = fixture();
    let mut compared = 0;
    for row in document["cases"].as_array().unwrap() {
        let name = row["name"].as_str().unwrap();
        if ![
            "flag-rejected-json",
            "flag-rejected-ndjson",
            "flag-rejected-page-1",
            "flag-rejected-limit-1",
            "flag-rejected-output-out",
            "flag-rejected-filename-template-x",
            "flag-rejected-directory-template-x",
            "flag-rejected-record-x",
            "flag-rejected-on-error-continue",
            "flag-rejected-unknown",
            "flag-rejected-x",
            "help-plain",
            "help-positional",
            "help-then-json",
            "help-unknown-then-help",
            "stdin-empty",
            "missing-account-before-options",
        ]
        .contains(&name)
        {
            continue;
        }
        let input: Input = serde_json::from_value(row["input"].clone()).unwrap();
        let home = tempfile::tempdir().unwrap();
        let observed = fanbox_download_support::binary::observe(home.path(), input);
        assert_eq!(
            observed.exit,
            row["observation"]["exits"][0].as_i64().unwrap() as i32,
            "{name} real binary exit: stdout={} stderr={}",
            observed.stdout,
            observed.stderr
        );
        assert_eq!(
            observed.stdout,
            row["observation"]["stdout"].as_str().unwrap(),
            "{name} real binary stdout"
        );
        assert_eq!(
            observed.stderr,
            row["observation"]["stderr"].as_str().unwrap(),
            "{name} real binary stderr"
        );
        compared += 1;
    }
    assert_eq!(compared, 17);
}
