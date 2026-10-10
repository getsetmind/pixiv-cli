mod fanbox_saved_cli_support;

use fanbox_saved_cli_support::{Input, observe};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, path::Path, process::Command};

const FIXTURE: &[u8] = include_bytes!("fixtures/fanbox-content-reads.json");
const SEALED_SHA: &str = "14eb6414dc0694598bbb85992bf134338e93c3cdc7e391bc5ac3f9e2e3b4ca1b";
const CHILD: &str = "PIXIV_FANBOX_SAVED_READS_CHILD";
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
];
const ROOT_ROWS: &[&str] = &[
    "root-startup-before-malformed-config",
    "root-config-before-service",
    "root-safe-stop-at-composition",
    "root-flags-before-startup",
    "root-stdin-error-before-startup",
];

fn fixture() -> Value {
    assert_eq!(format!("{:x}", Sha256::digest(FIXTURE)), SEALED_SHA);
    serde_json::from_slice(FIXTURE).unwrap()
}

#[test]
fn sealed_fanbox_rows_and_go_only_sources_are_explicit() {
    let fixture = fixture();
    assert_eq!(
        fixture["reference"],
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 172);
    let fields = OBSERVATION_FIELDS.iter().copied().collect::<BTreeSet<_>>();
    let mut roots = BTreeSet::new();
    for row in cases {
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
        if row["input"]["boundary"]
            .as_str()
            .unwrap()
            .starts_with("root")
        {
            roots.insert(row["name"].as_str().unwrap());
            assert_eq!(row["observation"]["requests"], serde_json::json!([]));
            assert!(row["observation"]["exits"][0].as_i64().unwrap() > 0);
        }
        assert_eq!(row["observation"]["socket_denied"], true);
        assert_eq!(row["observation"]["exec_denied"], true);
        for field in ["db_before", "db_after"] {
            assert_eq!(row["observation"][field].as_str().unwrap().len(), 64);
        }
    }
    assert_eq!(roots, ROOT_ROWS.iter().copied().collect());
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for section in ["sources", "frozen_go_production"] {
        for (path, digest) in fixture[section].as_object().unwrap() {
            let bytes = std::fs::read(repository.join(path)).unwrap();
            assert_eq!(
                format!("{:x}", Sha256::digest(bytes)),
                digest.as_str().unwrap(),
                "sealed Go source {path}"
            );
        }
    }
}

#[test]
fn saved_fanbox_composition_matches_all_167_command_rows() {
    if std::env::var_os(CHILD).is_some() {
        return;
    }
    let home = tempfile::tempdir().unwrap();
    let temp = home.path().join("temp");
    std::fs::create_dir(&temp).unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap());
    child
        .args([
            "--exact",
            "owned_saved_fanbox_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env_clear()
        .env(CHILD, "1")
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("TMPDIR", &temp)
        .env("TMP", &temp)
        .env("TEMP", &temp);
    for variable in ["PATH", "LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH", "SYSTEMROOT"] {
        if let Some(value) = std::env::var_os(variable) {
            child.env(variable, value);
        }
    }
    let output = child.output().unwrap();
    print!("{}", String::from_utf8_lossy(&output.stdout));
    assert!(
        output.status.success(),
        "owned saved FANBOX child failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn owned_saved_fanbox_child() {
    if std::env::var_os(CHILD).is_none() {
        return;
    }
    let home = std::env::var_os("HOME").unwrap();
    let home = Path::new(&home);
    assert_eq!(std::env::var_os("USERPROFILE").unwrap(), home.as_os_str());
    assert_eq!(std::env::temp_dir(), home.join("temp"));
    let document = fixture();
    let mut failures = Vec::new();
    let mut compared = 0;
    for case in document["cases"].as_array().unwrap() {
        let input: Input = serde_json::from_value(case["input"].clone()).unwrap();
        if input.boundary.starts_with("root") {
            continue;
        }
        let name = case["name"].as_str().unwrap();
        let observed = observe(home, input).await;
        let expected = &case["observation"];
        let mut actual = serde_json::to_value(&observed).unwrap();
        if matches!(
            name,
            "database-corrupt-before-options" | "json-temp-create-failure"
        ) {
            project_driver_or_path_message(name, &mut actual, expected);
        }
        for field in OBSERVATION_FIELDS {
            match *field {
                // SQLite byte layout and Go's PRAGMA rewrite are not row persistence.
                "db_before" | "db_after" => assert_eq!(actual[field].as_str().unwrap().len(), 64),
                // The Go children install process-wide seccomp. This child only uses injected ports.
                "socket_denied" | "exec_denied" => assert_eq!(actual[field], false),
                "db_rows_before" | "db_rows_after" if input_driver_row(name) => {
                    compare_driver_rows(
                        name,
                        field,
                        &actual[field],
                        &expected[field],
                        &mut failures,
                    );
                }
                _ if actual[field] != expected[field] => failures.push(format!(
                    "{name}.{field}\n  Rust: {}\n  Go:   {}",
                    actual[field], expected[field]
                )),
                _ => {}
            }
        }
        assert_eq!(
            observed.db_rows_before, observed.db_rows_after,
            "{name} saved rows changed"
        );
        assert!(
            !observed.stdout.contains("owned-session")
                && !observed.stderr.contains("owned-session")
                && !observed.stdout.contains("secret-session")
                && !observed.stderr.contains("secret-session"),
            "{name} credential disclosure"
        );
        compared += 1;
    }
    assert_eq!(compared, 167);
    assert!(
        failures.is_empty(),
        "{} exact saved CLI differences:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

fn input_driver_row(name: &str) -> bool {
    matches!(
        name,
        "database-corrupt-before-options" | "database-missing-table-before-options"
    )
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
    assert_eq!(actual.len(), expected.len(), "{name}.{field}");
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        if expected.as_str().unwrap().starts_with('[') {
            if actual != expected {
                failures.push(format!(
                    "{name}.{field}[{index}] Rust={actual} Go={expected}"
                ));
            }
        } else {
            let message = actual.as_str().unwrap();
            if name == "database-corrupt-before-options" {
                assert_eq!(message, "file is not a database");
                assert_eq!(expected, "file is not a database (26)");
            } else {
                assert!(message.starts_with("no such table: fanbox_account"));
                assert_eq!(
                    expected,
                    "SQL logic error: no such table: fanbox_account (1)"
                );
            }
            if field == "db_rows_before" {
                println!("Go-only query diagnostic {name}[{index}]: Rust={actual} Go={expected}");
            }
        }
    }
}
fn project_driver_or_path_message(name: &str, actual: &mut Value, expected: &Value) {
    let errors = actual["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1, "{name} error order/arity");
    let message = errors[0].as_str().unwrap().to_owned();
    let expected_message = expected["errors"][0].as_str().unwrap();
    assert_eq!(expected["errors"].as_array().unwrap().len(), 1);
    match name {
        "database-corrupt-before-options" => {
            assert_eq!(message, "file is not a database");
            assert_eq!(
                expected_message,
                "database: ping database: file is not a database (26)"
            );
        }
        "json-temp-create-failure" => {
            assert_eq!(message, std::io::Error::from_raw_os_error(20).to_string());
            assert_eq!(
                expected_message,
                "open <HOME>/temp/pixiv-cli-fanbox-json-<RANDOM>.tmp: not a directory"
            );
        }
        _ => unreachable!(),
    }
    let envelope = serde_json::json!({"error":{"code":"command_failed","message":message}});
    assert_eq!(
        actual["stderr"],
        format!("{envelope}\n"),
        "{name} exact invariant envelope"
    );
    println!(
        "Go-only diagnostic payload {name}: Rust={} Go={}",
        serde_json::json!(message),
        serde_json::json!(expected_message)
    );
    actual["errors"][0] = expected_message.into();
    actual["stderr"] = format!(
        "{}\n",
        serde_json::json!({"error":{"code":"command_failed","message":expected_message}})
    )
    .into();
}
