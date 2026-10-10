#![cfg(target_os = "linux")]

use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Child, Command, Output, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

const MALFORMED: &str = "[unfinished\n";
const NO_ACCOUNT: &str = "fanbox:: unauthorized: no fanbox account is authenticated";

struct OwnedHome(tempfile::TempDir);
impl OwnedHome {
    fn new(config: Option<&str>) -> Self {
        let home = Self(tempfile::tempdir().unwrap());
        if let Some(config) = config {
            std::fs::create_dir_all(home.data()).unwrap();
            std::fs::write(home.data().join("config.toml"), config).unwrap();
        }
        home
    }
    fn data(&self) -> std::path::PathBuf {
        self.0.path().join(".pixiv-cli")
    }
    fn spawn(&self, args: &[&str]) -> OwnedChild {
        let mut command = Command::new(env!("CARGO_BIN_EXE_pixiv"));
        command
            .args(args)
            .env_clear()
            .env("HOME", self.0.path())
            .env("USERPROFILE", self.0.path())
            .env("PATH", self.0.path())
            .current_dir(self.0.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for key in [
            "TMPDIR",
            "TEMP",
            "TMP",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
            "XDG_RUNTIME_DIR",
        ] {
            let path = self.0.path().join(key);
            std::fs::create_dir(&path).unwrap();
            command.env(key, path);
        }
        OwnedChild(Some(command.spawn().unwrap()))
    }
    fn run(&self, args: &[&str], input: &str, keep_input_open: bool) -> Output {
        let mut owned = self.spawn(args);
        let mut stdin = owned.0.as_mut().unwrap().stdin.take().unwrap();
        if let Err(error) = stdin.write_all(input.as_bytes()) {
            assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
        }
        if keep_input_open {
            owned.await_exit();
            drop(stdin);
        } else {
            drop(stdin);
            owned.await_exit();
        }
        owned.0.take().unwrap().wait_with_output().unwrap()
    }
    fn assert_config(&self, expected: &str) {
        assert_eq!(
            std::fs::read_to_string(self.data().join("config.toml")).unwrap(),
            expected
        );
    }
    fn assert_no_database(&self) {
        assert!(!self.data().join("pixiv-cli.db").exists());
    }
}

struct OwnedChild(Option<Child>);
impl OwnedChild {
    fn await_exit(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.0.as_mut().unwrap().try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "FANBOX child exceeded its bound");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).unwrap()
}

#[test]
fn fanbox_binary_replays_all_frozen_help_routes_before_startup_and_config() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/fanbox-help-routing.json")).unwrap();
    let rows = fixture["cases"].as_array().unwrap();
    assert_eq!(rows.len(), 22);
    for row in rows {
        let home = OwnedHome::new(Some(MALFORMED));
        let args = row["input"]["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|arg| arg.as_str().unwrap())
            .collect::<Vec<_>>();
        let output = home.run(&args, "", true);
        let expected = &row["observation"];
        assert_eq!(
            output.status.code(),
            expected["exit"].as_i64().map(|code| code as i32),
            "{}",
            row["name"]
        );
        assert_eq!(
            text(&output.stdout),
            expected["stdout"].as_str().unwrap(),
            "{}",
            row["name"]
        );
        assert_eq!(
            text(&output.stderr),
            expected["stderr"].as_str().unwrap(),
            "{}",
            row["name"]
        );
        home.assert_config(MALFORMED);
        home.assert_no_database();
    }
}

#[test]
fn fanbox_binary_root_help_advertises_the_product_group() {
    let home = OwnedHome::new(Some(MALFORMED));
    let output = home.run(&["--help"], "", true);
    assert_eq!(output.status.code(), Some(0));
    assert!(text(&output.stdout).contains("fanbox"));
    assert!(text(&output.stdout).contains("Browse and download Pixiv FANBOX content"));
    assert!(output.stderr.is_empty());
    home.assert_config(MALFORMED);
    home.assert_no_database();
}

#[test]
fn fanbox_binary_argument_failures_precede_startup_config_and_account_store() {
    let rows: &[(&[&str], i32, &str, bool)] = &[
        (
            &["fanbox", "home", "--unknown"],
            2,
            "error: unknown option '--unknown'\n",
            true,
        ),
        (
            &["fanbox", "home", "extra"],
            1,
            "error: usage: pixiv fanbox home\n",
            true,
        ),
        (
            &["fanbox", "post", "123", "456"],
            1,
            "error: usage: pixiv fanbox post POST_ID\n",
            true,
        ),
        (
            &["fanbox", "posts", "one", "two"],
            1,
            "error: usage: pixiv fanbox posts SOURCE\n",
            true,
        ),
        (
            &["fanbox", "tags", "one", "two"],
            1,
            "error: usage: pixiv fanbox tags CREATOR\n",
            true,
        ),
        (
            &["fanbox", "creators", "extra"],
            1,
            "error: usage: pixiv fanbox creators --kind supporting|following\n",
            true,
        ),
        (
            &["fanbox", "supporting", "extra"],
            1,
            "error: usage: pixiv fanbox supporting\n",
            true,
        ),
        (
            &["fanbox", "mcp", "extra"],
            1,
            "error: usage: pixiv fanbox mcp\n",
            true,
        ),
        (
            &["fanbox", "post"],
            1,
            "error: usage: pixiv fanbox post POST_ID\n",
            false,
        ),
        (
            &["fanbox", "posts"],
            1,
            "error: usage: pixiv fanbox posts SOURCE\n",
            false,
        ),
        (
            &["fanbox", "tags"],
            1,
            "error: usage: pixiv fanbox tags CREATOR\n",
            false,
        ),
    ];
    for (args, exit, stderr, keep_input_open) in rows {
        let home = OwnedHome::new(Some(MALFORMED));
        let output = home.run(args, "", *keep_input_open);
        assert_eq!(output.status.code(), Some(*exit), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        assert_eq!(text(&output.stderr), *stderr, "{args:?}");
        home.assert_config(MALFORMED);
        home.assert_no_database();
    }
}

#[test]
fn fanbox_binary_malformed_runtime_stops_all_reads_and_mcp_before_store_open() {
    for args in [
        vec!["fanbox", "creators", "--json"],
        vec!["fanbox", "home", "--json"],
        vec!["fanbox", "supporting", "--json"],
        vec!["fanbox", "post", "123", "--json"],
        vec!["fanbox", "posts", "owned", "--json"],
        vec!["fanbox", "tags", "owned", "--json"],
        vec!["fanbox", "mcp"],
    ] {
        let home = OwnedHome::new(Some(MALFORMED));
        let output = home.run(&args, "", true);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        if args.contains(&"--json") {
            let error: Value = serde_json::from_slice(&output.stderr).unwrap();
            assert_eq!(error["error"]["code"], "command_failed");
            assert_eq!(error["error"]["message"], "toml: expected character ]");
        } else {
            assert_eq!(text(&output.stderr), "error: toml: expected character ]\n");
        }
        home.assert_config(MALFORMED);
        home.assert_no_database();
    }
}

#[test]
fn fanbox_binary_all_saved_reads_stop_at_missing_account_without_native_open() {
    let config = "[fanbox.network]\nproxy_url = 'socks5://native-must-not-open.invalid:65535'\n";
    for args in [
        vec!["fanbox", "creators", "--json"],
        vec!["fanbox", "home", "--json"],
        vec!["fanbox", "supporting", "--json"],
        vec!["fanbox", "post", "123", "--json"],
        vec!["fanbox", "posts", "owned", "--json"],
        vec!["fanbox", "tags", "owned", "--json"],
        vec!["--proxy=", "fanbox", "home", "--json"],
    ] {
        let home = OwnedHome::new(Some(config));
        let output = home.run(&args, "", true);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(output.stdout.is_empty());
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stderr).unwrap(),
            json!({"error":{"code":"unauthorized","message":NO_ACCOUNT}}),
            "{args:?}"
        );
        assert!(home.data().join("pixiv-cli.db").exists());
        home.assert_config(config);
    }
}

#[test]
fn fanbox_binary_mcp_proxy_conflict_stops_before_service_and_store() {
    let home = OwnedHome::new(Some(""));
    let output = home.run(&["--proxy=", "fanbox", "mcp", "--no-proxy=false"], "", true);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        text(&output.stderr),
        "error: use either --proxy or --no-proxy, not both\n"
    );
    home.assert_config("");
    home.assert_no_database();
}

#[test]
fn fanbox_binary_mcp_serves_real_schemas_and_missing_account_errors_then_eof() {
    let home = OwnedHome::new(Some(""));
    let mut owned = home.spawn(&["--proxy=", "fanbox", "mcp"]);
    let child = owned.0.as_mut().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (sender, received) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if sender.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let mut exchange = |frame: Value| {
        writeln!(stdin, "{frame}").unwrap();
        stdin.flush().unwrap();
        let line = received
            .recv_timeout(Duration::from_secs(5))
            .expect("FANBOX MCP response");
        serde_json::from_str::<Value>(&line).unwrap()
    };
    let initialized = exchange(
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"owned-binary-contract","version":"1"}}}),
    );
    assert_eq!(
        initialized["result"]["serverInfo"]["name"],
        "pixiv-cli-fanbox"
    );
    let tools = exchange(json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}));
    assert_eq!(tools["result"], pixiv_mcp::fanbox::tools());
    assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 11);
    let result = exchange(
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"fanbox_home","arguments":{}}}),
    );
    assert_eq!(result["result"]["isError"], true);
    assert!(
        result["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains(NO_ACCOUNT)
    );
    drop(stdin);
    owned.await_exit();
    reader.join().unwrap();
    let mut child = owned.0.take().unwrap();
    assert!(child.wait().unwrap().success());
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(stderr.is_empty(), "{stderr}");
    assert!(home.data().join("pixiv-cli.db").exists());
    home.assert_config("");
}
