use super::{
    io::decode_hex,
    schema::{Case, State, Step},
    state,
};
use pixiv_app::{database::Database, fanbox_account::Account, lifecycle::Context};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    io::Write,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Serialize)]
pub struct BinaryObservation {
    pub exit: i32,
    pub stdout: String,
    pub stderr: String,
    pub after: State,
}

pub fn observe(home: &Path, step: &Step, before: &State) -> BinaryObservation {
    assert!(!step.release && !step.canceled && !step.cancel_on_request);
    let route = pixiv_cli_rs::fanbox::command_route(&step.args);
    assert!(
        route.path.last().is_none_or(|leaf| leaf != "import")
            || step.args.iter().any(|arg| arg == "--help")
    );
    let start = state::now();
    seed(home, before);
    let mut created = BTreeMap::new();
    assert_eq!(
        serde_json::to_value(state::snapshot(home, start, &mut created)).unwrap(),
        serde_json::to_value(before).unwrap(),
        "binary saved-state seed"
    );
    let temp = home.join("temp");
    std::fs::create_dir(&temp).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_pixiv"));
    command
        .args(&step.args)
        .env_clear()
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("TMPDIR", &temp)
        .env("TMP", &temp)
        .env("TEMP", &temp)
        .env("XDG_CONFIG_HOME", home.join("xdg-config"))
        .env("XDG_DATA_HOME", home.join("xdg-data"))
        .env("XDG_CACHE_HOME", home.join("xdg-cache"))
        .env("XDG_STATE_HOME", home.join("xdg-state"))
        .env("XDG_RUNTIME_DIR", home.join("xdg-runtime"))
        .env("APPDATA", home.join("appdata"))
        .env("LOCALAPPDATA", home.join("local-appdata"))
        .env("PATH", "")
        .env("TZ", "UTC")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for variable in ["LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH", "SYSTEMROOT"] {
        if let Some(value) = std::env::var_os(variable) {
            command.env(variable, value);
        }
    }
    let mut child = command.spawn().unwrap();
    if let Err(error) = child
        .stdin
        .take()
        .unwrap()
        .write_all(&decode_hex(&step.input_hex))
    {
        assert_eq!(
            error.kind(),
            std::io::ErrorKind::BrokenPipe,
            "write binary stdin"
        );
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!(
                "owned auth binary timed out: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    BinaryObservation {
        exit: output
            .status
            .code()
            .expect("binary stopped without exit code"),
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: String::from_utf8(output.stderr).unwrap(),
        after: state::snapshot(home, start, &mut created),
    }
}

fn seed(home: &Path, before: &State) {
    state::seed(
        home,
        &Case {
            name: "binary owned saved-state seed".into(),
            config_before: before.config.clone(),
            seed: vec![],
            steps: vec![],
            observations: vec![],
        },
    );
    if !before.database {
        assert!(before.rows.is_empty());
        return;
    }
    let mut database = Database::open(home.join(".pixiv-cli")).unwrap();
    for row in &before.rows {
        assert_eq!(
            row["credential_revision"], 1,
            "bounded binary seeds use initial credentials"
        );
        let mut account = Account::new(
            row["user_id"].as_i64().unwrap(),
            row["display_name"].as_str().unwrap(),
            row["creator_id"].as_str().unwrap(),
            &decode_hex(row["session_hex"].as_str().unwrap()),
        );
        account.sort_order = row["sort_order"].as_i64().unwrap();
        account.validated_at = 1_700_000_000;
        database
            .save_fanbox_credential(&Context::background(), &account)
            .unwrap();
    }
    database.close().unwrap();
}
