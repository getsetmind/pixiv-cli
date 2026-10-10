mod fanbox_auth_support;

use fanbox_auth_support::{observe, schema::Case};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, path::Path, process::Command};

const FIXTURE: &[u8] = include_bytes!("fixtures/fanbox-auth.json");
const SEALED_SHA: &str = "1d3bab3ca00810abace1ebb34125f61df2d692db548baa53e31ecd0b814c2351";
const CHILD: &str = "PIXIV_FANBOX_AUTH_CONTRACT_CHILD";
const OBSERVATION_FIELDS: &[&str] = &[
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
        SEALED_SHA,
        "Go auth fixture seal"
    );
    serde_json::from_slice(FIXTURE).unwrap()
}

#[test]
fn sealed_fanbox_auth_source_schema_and_all_233_scenarios_are_explicit() {
    let fixture = fixture();
    assert_eq!(
        fixture["reference"],
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    assert_eq!(fixture["environment"], "linux/amd64");
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 233);
    let observation_fields = OBSERVATION_FIELDS.iter().copied().collect::<BTreeSet<_>>();
    let state_fields = STATE_FIELDS.iter().copied().collect::<BTreeSet<_>>();
    let mut executions = 0;
    let mut names = BTreeSet::new();
    for row in cases {
        let case: Case = serde_json::from_value(row.clone()).unwrap();
        assert!(
            names.insert(case.name.clone()),
            "duplicate scenario {}",
            case.name
        );
        assert_eq!(
            case.steps.len(),
            case.observations.len(),
            "{} unmatched steps",
            case.name
        );
        executions += case.steps.len();
        for observation in row["observations"].as_array().unwrap() {
            assert_eq!(
                observation
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect::<BTreeSet<_>>(),
                observation_fields,
                "{} unclassified observation",
                case.name
            );
            assert_eq!(observation["socket_denied"], true);
            assert_eq!(observation["exec_denied"], true);
            for field in ["before", "after"] {
                assert_eq!(
                    observation[field]
                        .as_object()
                        .unwrap()
                        .keys()
                        .map(String::as_str)
                        .collect::<BTreeSet<_>>(),
                    state_fields,
                    "{} {field} unclassified state",
                    case.name
                );
            }
        }
    }
    assert_eq!(executions, 255);
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for section in ["sources", "reused_fixtures"] {
        for (path, digest) in fixture[section].as_object().unwrap() {
            let bytes = std::fs::read(repository.join(path)).unwrap();
            assert_eq!(
                format!("{:x}", Sha256::digest(bytes)),
                digest.as_str().unwrap(),
                "sealed {section} {path}"
            );
        }
    }
    for section in ["dependencies", "dependency_sources", "stdlib_sources"] {
        assert!(
            !fixture[section].as_object().unwrap().is_empty(),
            "unsealed {section}"
        );
    }
    assert!(
        fixture["limitations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value.as_str().unwrap().contains("native browser providers"))
    );
}

#[test]
fn real_fanbox_auth_service_sdk_sqlite_config_and_all_255_cli_executions_match_go() {
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
            "owned_fanbox_auth_contract_child",
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
        .env("TZ", "UTC");
    for variable in ["LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH", "SYSTEMROOT"] {
        if let Some(value) = std::env::var_os(variable) {
            child.env(variable, value);
        }
    }
    let output = child.output().unwrap();
    print!("{}", String::from_utf8_lossy(&output.stdout));
    assert!(
        output.status.success(),
        "owned FANBOX auth child failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn owned_fanbox_auth_contract_child() {
    if std::env::var_os(CHILD).is_none() {
        return;
    }
    let home = std::env::var_os("HOME").unwrap();
    let home = Path::new(&home);
    assert_eq!(std::env::var_os("USERPROFILE").unwrap(), home.as_os_str());
    assert_eq!(std::env::temp_dir(), home.join("temp"));
    let fixture = fixture();
    let mut failures = Vec::new();
    let mut scenarios = 0;
    let mut executions = 0;
    for row in fixture["cases"].as_array().unwrap() {
        let case: Case = serde_json::from_value(row.clone()).unwrap();
        let observations = observe(home, &case).await;
        assert_eq!(
            observations.len(),
            case.observations.len(),
            "{} execution count",
            case.name
        );
        for (index, (actual, expected)) in observations.iter().zip(&case.observations).enumerate() {
            for canary in [
                home.to_str().unwrap(),
                "/owned-private-path-canary",
                "synthetic-browser-secret",
                "synthetic-seed-",
                "opaque-secret-canary",
            ] {
                assert!(
                    !actual.stdout.contains(canary) && !actual.stderr.contains(canary),
                    "{}[{index}] private source leaked",
                    case.name
                );
            }
            let actual = serde_json::to_value(actual).unwrap();
            let expected = serde_json::to_value(expected).unwrap();
            for field in OBSERVATION_FIELDS {
                if matches!(*field, "socket_denied" | "exec_denied") {
                    // Go seccomp installation is source-only; Rust uses only injected dependency ports.
                    assert_eq!(
                        actual[*field], false,
                        "{}[{index}] falsely claimed physical seccomp",
                        case.name
                    );
                    continue;
                }
                if actual[*field] != expected[*field] {
                    failures.push(format!(
                        "{}[{index}].{field}\n  Rust: {}\n  Go:   {}",
                        case.name, actual[*field], expected[*field]
                    ));
                }
            }
            executions += 1;
        }
        scenarios += 1;
    }
    assert_eq!(scenarios, 233);
    assert_eq!(executions, 255);
    assert!(
        failures.is_empty(),
        "{} exact FANBOX auth differences:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn main_uses_normal_fanbox_auth_composition_without_runtime_or_config_initialization() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let main = std::fs::read_to_string(root.join("main.rs")).unwrap();
    let auth = std::fs::read_to_string(root.join("fanbox_auth.rs")).unwrap();
    assert!(
        main.contains("fanbox_auth::prepare_root"),
        "main must use the tested production auth root preparation"
    );
    assert!(
        main.contains("fanbox_auth::Data"),
        "main must dispatch all auth leaves through normal source dependencies"
    );
    let prepare = auth
        .split("pub fn prepare_root")
        .nth(1)
        .expect("normal root preparation");
    assert!(
        !prepare.contains("ensure_defaults"),
        "auth root must not ensure config"
    );
    assert!(
        !prepare.contains("fanbox_runtime("),
        "auth root must not eagerly load runtime"
    );
    assert!(
        !auth.contains("#[cfg(test)]"),
        "auth source must not contain test-only branches"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn genuine_binary_auth_help_and_saved_management_match_30_bounded_go_states() {
    let fixture = fixture();
    let witnesses = [
        ("help/import", 0),
        ("help/list", 0),
        ("help/status", 0),
        ("help/use", 0),
        ("help/remove", 0),
        ("route/auth-bare", 0),
        ("route/unknown-child", 0),
        ("flags/list_--json=false", 0),
        ("flags/list_--json=true", 0),
        ("uid/status/2b30303031", 0),
        ("uid/use/2b30303031", 0),
        ("uid/remove/2b30303031", 0),
        ("config/use-auto-empty-absent-file", 0),
        ("config/stale-explicit-status-uid-succeeds", 0),
        ("workflow/import-list-status-use-auto-remove-reimport", 0),
        ("workflow/import-list-status-use-auto-remove-reimport", 1),
        ("workflow/import-list-status-use-auto-remove-reimport", 3),
        ("workflow/import-list-status-use-auto-remove-reimport", 4),
        ("workflow/import-list-status-use-auto-remove-reimport", 6),
        ("workflow/import-list-status-use-auto-remove-reimport", 8),
        ("workflow/import-list-status-use-auto-remove-reimport", 9),
        ("workflow/import-list-status-use-auto-remove-reimport", 10),
        ("workflow/import-list-status-use-auto-remove-reimport", 11),
        ("workflow/import-list-status-use-auto-remove-reimport", 12),
        ("parse/list_1", 0),
        ("parse/use_1_2", 0),
        ("workflow/import-list-status-use-auto-remove-reimport", 17),
        ("workflow/import-list-status-use-auto-remove-reimport", 18),
        ("config/preserve-comments-unknown-keys", 1),
        ("config/preserve-comments-unknown-keys", 2),
    ];
    for (name, index) in witnesses {
        let row = fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["name"] == name)
            .unwrap();
        let case: Case = serde_json::from_value(row.clone()).unwrap();
        let home = tempfile::tempdir().unwrap();
        let expected = &case.observations[index];
        let actual =
            fanbox_auth_support::binary::observe(home.path(), &case.steps[index], &expected.before);
        assert_eq!(actual.exit, expected.exit, "{name}[{index}] binary exit");
        assert_eq!(
            actual.stdout, expected.stdout,
            "{name}[{index}] binary stdout"
        );
        assert_eq!(
            actual.stderr, expected.stderr,
            "{name}[{index}] binary stderr"
        );
        assert_eq!(
            serde_json::to_value(actual.after).unwrap(),
            serde_json::to_value(&expected.after).unwrap(),
            "{name}[{index}] binary state"
        );
        assert!(
            !actual.stdout.contains(home.path().to_str().unwrap())
                && !actual.stderr.contains(home.path().to_str().unwrap()),
            "{name}[{index}] binary path disclosure"
        );
    }
}
