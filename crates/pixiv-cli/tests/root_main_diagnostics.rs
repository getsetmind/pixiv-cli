#![cfg(target_os = "linux")]

#[allow(dead_code)]
#[path = "support/updater_main_owned.rs"]
mod owned;

use chrono::DateTime;
use owned::{OwnedHome, text};
use pixiv_app::database::{Database, PixivAccount};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::process::Output;

const DEBUG_JSON: [(&str, &str); 2] = [("PIXIV_LOG_LEVEL", "debug"), ("PIXIV_LOG_FORMAT", "json")];
const CONFIG: &str = "[update]\ncheck_enabled=false\n";
const CLOCK: &str = "2000-01-02T03:04:05Z";

fn fixture() -> Value {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/diagnostics-root.json")).unwrap();
    assert_eq!(
        fixture["frozen_go"],
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    fixture
}

fn normalize_json_clock(bytes: &[u8]) -> String {
    let mut normalized = String::new();
    for line in text(bytes).split_inclusive('\n') {
        if line.starts_with("{\"time\":") {
            let fields: Value = serde_json::from_str(line).unwrap();
            let stamp = fields["time"].as_str().unwrap();
            DateTime::parse_from_rfc3339(stamp).unwrap();
            let prefix = format!("{{\"time\":{}", serde_json::to_string(stamp).unwrap());
            let remainder = line.strip_prefix(&prefix).unwrap();
            normalized.push_str(&format!("{{\"time\":\"{CLOCK}\"{remainder}"));
        } else {
            normalized.push_str(line);
        }
    }
    normalized
}

fn normalize_text_clock(bytes: &[u8]) -> String {
    text(bytes)
        .split_inclusive('\n')
        .map(|line| {
            let Some(rest) = line.strip_prefix("[Pixiv CLI] ") else {
                return line.to_owned();
            };
            chrono::NaiveTime::parse_from_str(&rest[..8], "%H:%M:%S").unwrap();
            format!("[Pixiv CLI] 03:04:05 {}", &rest[9..])
        })
        .collect()
}

fn root_records(output: &Output, operation: &str) -> Vec<Value> {
    text(&output.stderr)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|record| record["operation"] == operation)
        .collect()
}

fn assert_root_lifecycle(output: &Output, operation: &str, failed: bool) {
    let records = root_records(output, operation);
    assert_eq!(records.len(), 2, "{operation}: {}", text(&output.stderr));
    let module = if operation.starts_with("pixiv fanbox") {
        "FANBOX CLI"
    } else {
        "Pixiv CLI"
    };
    let mut expected_start = json!({"time":CLOCK,"level":"DEBUG","module":module,"kind":"started","operation":operation});
    let mut expected_finish = json!({"time":CLOCK,"level":"DEBUG","module":module,"kind":if failed {"failed"} else {"completed"},"operation":operation});
    if failed {
        expected_finish["reason"] = "command failed".into();
    }
    expected_start["time"] = records[0]["time"].clone();
    expected_finish["time"] = records[1]["time"].clone();
    DateTime::parse_from_rfc3339(records[0]["time"].as_str().unwrap()).unwrap();
    DateTime::parse_from_rfc3339(records[1]["time"].as_str().unwrap()).unwrap();
    assert_eq!(records[0], expected_start, "{operation}");
    assert_eq!(records[1], expected_finish, "{operation}");
    let lines = text(&output.stderr);
    let first = lines.lines().next().unwrap();
    assert_eq!(serde_json::from_str::<Value>(first).unwrap(), records[0]);
    let tail: Vec<_> = lines.lines().collect();
    let terminal = tail
        .iter()
        .position(|line| serde_json::from_str::<Value>(line).ok().as_ref() == Some(&records[1]))
        .unwrap();
    if failed {
        assert_eq!(
            terminal,
            tail.len() - 2,
            "terminal diagnostic must precede the error envelope: {operation}"
        );
    } else {
        assert_eq!(
            terminal,
            tail.len() - 1,
            "terminal diagnostic must be last: {operation}"
        );
    }
}

#[test]
fn actual_fanbox_root_matches_frozen_module_and_error_bytes_after_timestamp_validation() {
    let fixture = fixture();
    let row = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "fanbox/module")
        .unwrap();
    let home = OwnedHome::new(row["input"]["config_before"].as_str());
    let args: Vec<_> = row["input"]["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|arg| arg.as_str().unwrap())
        .collect();
    let output = home.run_with_environment(&args, "", &DEBUG_JSON);
    assert_eq!(
        output.status.code(),
        row["observation"]["exit"].as_i64().map(|n| n as i32)
    );
    assert_eq!(text(&output.stdout), row["observation"]["stdout"]);
    assert_eq!(
        normalize_json_clock(&output.stderr),
        row["observation"]["stderr"]
    );
    home.assert_config(row["input"]["config_before"].as_str().unwrap());
    home.assert_no_database_or_cache();
}

#[test]
fn actual_auth_list_matches_frozen_success_bytes_and_database_access_after_process_exit() {
    let fixture = fixture();
    let row = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "cleanup/success-before-completed")
        .unwrap();
    let home = OwnedHome::new(row["input"]["config_before"].as_str());
    let directory = home.config().parent().unwrap().to_owned();
    let mut database = Database::open(&directory).unwrap();
    database
        .save_pixiv_credential(&PixivAccount::new(
            42,
            "owned account",
            b"synthetic-refresh-42",
        ))
        .unwrap();
    database.close().unwrap();
    let output = home.run_with_environment(
        &["auth", "list", "--json"],
        "",
        &[("PIXIV_LOG_LEVEL", "debug"), ("PIXIV_LOG_FORMAT", "text")],
    );
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(text(&output.stdout), row["observation"]["stdout"]);
    assert_eq!(
        normalize_text_clock(&output.stderr),
        row["observation"]["stderr"]
    );
    home.assert_config(row["input"]["config_before"].as_str().unwrap());
    let database = Database::open(directory).unwrap();
    let account = database.get_pixiv(42).unwrap();
    assert_eq!(account.credential_revision, 1);
    assert_eq!(account.refresh_token_copy(), b"synthetic-refresh-42");
    database.close().unwrap();
}

#[test]
fn ordinary_root_families_emit_one_lifecycle_with_their_canonical_command_path() {
    let rows: &[(&[&str], &str)] = &[
        (&["detail", "invalid", "--json"], "pixiv detail"),
        (
            &["user", "detail", "invalid", "--json"],
            "pixiv user detail",
        ),
        (&["search", "owned", "--json", "--ndjson"], "pixiv search"),
        (
            &["user", "search", "owned", "--json", "--ndjson"],
            "pixiv user search",
        ),
        (
            &["novel", "search", "owned", "--json", "--ndjson"],
            "pixiv novel search",
        ),
        (&["ugoira", "invalid", "--json"], "pixiv ugoira"),
        (
            &["download", "123", "--proxy=", "--no-proxy", "--json"],
            "pixiv download",
        ),
        (&["auth", "use", "42", "--json"], "pixiv auth use"),
        (&["auth", "check", "42", "--json"], "pixiv auth check"),
        (
            &["dic", "search", "owned", "--page=0", "--json"],
            "pixiv dic search",
        ),
        (
            &["fanbox", "creators", "--kind=invalid", "--json"],
            "pixiv fanbox creators",
        ),
        (
            &["fanbox", "download", "123", "--proxy=", "--no-proxy"],
            "pixiv fanbox download",
        ),
        (&["mcp", "--proxy=", "--no-proxy"], "pixiv mcp"),
        (
            &["fanbox", "mcp", "--proxy=", "--no-proxy"],
            "pixiv fanbox mcp",
        ),
        (
            &["update", "--check", "--json", "--proxy=ftp://owned.invalid"],
            "pixiv update",
        ),
    ];
    for (args, operation) in rows {
        let home = OwnedHome::new(Some(CONFIG));
        let output = home.run_with_environment(args, "", &DEBUG_JSON);
        assert!(
            output
                .status
                .code()
                .is_some_and(|code| code == 1 || code == 2),
            "{operation}"
        );
        assert_root_lifecycle(&output, operation, true);
        home.assert_config(CONFIG);
    }
}

#[test]
fn help_parser_version_quiet_config_and_no_ensure_config_owners_skip_root_diagnostics() {
    let rows: &[&[&str]] = &[
        &["--help"],
        &["--version"],
        &["--unknown"],
        &["update", "--help"],
        &["update", "--unknown"],
        &["detail", "--help"],
        &["detail", "1", "--unknown"],
        &["search", "--help"],
        &["ugoira", "--help"],
        &["download", "--help"],
        &["download", "--unknown"],
        &["auth", "list", "--help"],
        &["auth", "export", "42"],
        &["auth", "_callback", "invalid"],
        &["fanbox", "creators", "--help"],
        &["fanbox", "auth", "list"],
        &["dic", "search", "--help"],
        &["config", "get", "request_interval"],
    ];
    for args in rows {
        let home = OwnedHome::new(Some(CONFIG));
        let output = home.run_with_environment(args, "", &DEBUG_JSON);
        let diagnostic_lines: Vec<_> = text(&output.stderr)
            .lines()
            .filter(|line| {
                line.starts_with("{\"time\":")
                    || line.starts_with("[Pixiv CLI]")
                    || line.starts_with("[FANBOX CLI]")
            })
            .map(str::to_owned)
            .collect();
        assert!(
            diagnostic_lines.is_empty(),
            "{args:?}: {diagnostic_lines:?}"
        );
        home.assert_config(CONFIG);
    }
}

#[test]
fn non_debug_runtime_stays_silent_but_invalid_runtime_still_stops_before_business() {
    let home = OwnedHome::new(Some(CONFIG));
    let output = home.run_with_environment(
        &["update", "--check", "--proxy=ftp://owned.invalid"],
        "",
        &[("PIXIV_LOG_LEVEL", "info"), ("PIXIV_LOG_FORMAT", "json")],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(root_records(&output, "pixiv update").is_empty());
    home.assert_no_database_or_cache();
    let malformed = "[owned-malformed\n";
    let home = OwnedHome::new(Some(malformed));
    let output = home.run_with_environment(&["update", "--check"], "", &DEBUG_JSON);
    assert_eq!(output.status.code(), Some(1));
    assert!(root_records(&output, "pixiv update").is_empty());
    assert!(text(&output.stderr).contains("toml:"));
    home.assert_config(malformed);
    home.assert_no_database_or_cache();
}

#[test]
fn dictionary_root_canonical_path_matches_all_frozen_scalar_flag_collision_rows() {
    let bytes = include_bytes!("fixtures/diagnostics-dictionary-path.json");
    assert_eq!(
        format!("{:x}", Sha256::digest(bytes)),
        "ed195c676a6c8e044cee7e76ee5b61f35ff8d2193ea4c160c856987ae6d8893c",
    );
    let fixture: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(fixture["schema_version"], 1);
    assert_eq!(
        fixture["frozen_go"],
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    let rows = fixture["cases"].as_array().unwrap();
    assert_eq!(rows.len(), 8);
    for row in rows {
        assert_eq!(row["http_attempts"], 0, "{}", row["name"]);
        let before = row["input"]["config_before"].as_str().unwrap();
        let home = OwnedHome::new(Some(before));
        let args: Vec<_> = row["input"]["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|arg| arg.as_str().unwrap())
            .collect();
        let output = home.run_with_environment(&args, "", &DEBUG_JSON);
        let observation = &row["observation"];
        assert_eq!(
            output.status.code(),
            observation["exit"].as_i64().map(|n| n as i32),
            "{}",
            row["name"]
        );
        assert_eq!(
            text(&output.stdout),
            observation["stdout"],
            "{} stdout",
            row["name"]
        );
        assert_eq!(
            normalize_json_clock(&output.stderr),
            observation["stderr"],
            "{} stderr",
            row["name"]
        );
        home.assert_config(observation["config_after"].as_str().unwrap());
        home.assert_no_database_or_cache();
    }
}
