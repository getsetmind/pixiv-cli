#![cfg(unix)]
#[path = "support/cli_interrupt.rs"]
mod support;
use pixiv_cli_rs::interrupt::OwnedSignalContext;
use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
fn rows() -> Vec<serde_json::Value> {
    serde_json::from_str(include_str!("fixtures/cli_interrupt.json")).unwrap()
}
#[test]
#[ignore = "owned signal child invoked by parent"]
fn cli_interrupt_child() {
    let name = std::env::var("PIXIV_INTERRUPT_CHILD").unwrap();
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
    let runtime = tokio::runtime::Runtime::new().unwrap();
    if name == "blocked-stdin" {
        runtime.block_on(async {
            let owner = OwnedSignalContext::new().unwrap();
            let context = owner.context();
            let marker = home.join("cancel-observed");
            tokio::spawn(async move {
                context.cancelled().await;
                std::fs::write(marker, b"cancelled").unwrap();
            });
            println!("READY");
            let mut line = String::new();
            std::io::stdin().read_line(&mut line).unwrap();
            assert!(owner.context().error().is_some());
        });
        std::process::exit(0);
    }
    let row = rows().into_iter().find(|r| r["name"] == name).unwrap();
    let exit = runtime.block_on(async {
        let owner = OwnedSignalContext::new().unwrap();
        let context = owner.context();
        let path = home.join("config.toml");
        std::fs::write(&path, "").unwrap();
        let mut db = pixiv_app::database::Database::open(&home).unwrap();
        db.save_pixiv_credential(&pixiv_app::database::PixivAccount::new(
            42,
            "synthetic",
            b"synthetic-refresh",
        ))
        .unwrap();
        let db = Arc::new(Mutex::new(db));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let dropped_pending = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let transport = support::Fixture {
            stage: row["stage"].as_str().unwrap().into(),
            requests: requests.clone(),
            dropped_pending: dropped_pending.clone(),
        };
        let execution = pixiv_app::execution::Execution::new(
            pixiv_app::config::Store::new(path),
            db.clone(),
            move |_| Ok(transport.clone()),
        );
        let destination = home.join("downloads");
        let source = "https://i.pximg.net/assets/interrupt.png?signature=synthetic";
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(source.as_bytes());
        let hash = digest[..6]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let final_path = destination.join(format!("interrupt-{hash}.png"));
        if row["existing"] == true {
            std::fs::create_dir_all(&destination).unwrap();
            std::fs::write(&final_path, b"previous").unwrap();
        }
        let mut args = vec!["download".into(), source.into()];
        if row["mode"] != "text" {
            args.push(format!("--{}", row["mode"].as_str().unwrap()));
        }
        let command =
            pixiv_cli_rs::download::DownloadCommand::parse(&args, &mut &b""[..], false).unwrap();
        let out = Arc::new(Mutex::new(Vec::<u8>::new()));
        let diagnostics = Arc::new(Mutex::new(Vec::<u8>::new()));
        let result = command
            .execute_with_factory(
                &context,
                || {
                    Ok(pixiv_cli_rs::download::DownloadRuntime {
                        download_path: destination.to_string_lossy().into_owned(),
                        ..Default::default()
                    })
                },
                &mut &b""[..],
                pixiv_cli_rs::download::DownloadSinks {
                    output: out.clone(),
                    error: diagnostics.clone(),
                },
                || Ok(execution),
                |client| Arc::new(support::SaveClient(client)),
            )
            .await;
        let mut stderr = diagnostics.lock().unwrap().clone();
        let exit = pixiv_cli_rs::finish_command(
            result,
            row["mode"] == "ndjson",
            row["mode"] != "text",
            &mut stderr,
        );
        let account = db.lock().unwrap().get_pixiv(42).unwrap();
        let mut actual = row.clone();
        actual["stdout"] = String::from_utf8(out.lock().unwrap().clone())
            .unwrap()
            .into();
        actual["stderr"] = String::from_utf8(stderr).unwrap().into();
        actual["exit"] = exit.into();
        actual["requests"] = serde_json::to_value(requests.lock().unwrap().clone()).unwrap();
        actual["exists"] = final_path.exists().into();
        actual["final"] = std::fs::read_to_string(&final_path)
            .unwrap_or_default()
            .into();
        actual["temporary_files"] = std::fs::read_dir(&destination)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|e| {
                        e.file_name()
                            .to_string_lossy()
                            .starts_with(".atomic-write-")
                    })
                    .count()
            })
            .unwrap_or(0)
            .into();
        actual["refresh_token"] = String::from_utf8(account.refresh_token_copy())
            .unwrap()
            .into();
        actual["credential_revision"] = account.credential_revision.into();
        assert_eq!(
            dropped_pending.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "owned pending SPI future/body must be dropped"
        );
        actual.as_object_mut().unwrap().remove("closed_connections");
        std::fs::write(
            home.join("result.json"),
            serde_json::to_vec(&actual).unwrap(),
        )
        .unwrap();
        exit
    });
    std::process::exit(exit);
}
fn wait_exit(child: &mut std::process::Child) -> std::process::ExitStatus {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() > end {
            child.kill().unwrap();
            panic!("signal child failed to terminate before timeout");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
#[test]
fn actual_owned_sigint_preserves_frozen_download_output_credentials_and_atomic_files() {
    for mut row in rows()
        .into_iter()
        .filter(|r| !r["stage"].as_str().unwrap().starts_with("mcp-"))
    {
        let home = tempfile::tempdir().unwrap();
        let mut child = support::OwnedChild(
            Command::new(std::env::current_exe().unwrap())
                .args(["--ignored", "--exact", "cli_interrupt_child", "--nocapture"])
                .env("PIXIV_INTERRUPT_CHILD", row["name"].as_str().unwrap())
                .env("HOME", home.path())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if line.unwrap() == "READY" {
                    let _ = tx.send(());
                    break;
                }
            }
        });
        if rx.recv_timeout(Duration::from_secs(10)).is_err() {
            child.kill().unwrap();
            panic!("bounded READY failed for {}", row["name"]);
        }
        if row["stage"] == "body" {
            let end = Instant::now() + Duration::from_secs(5);
            loop {
                let ready = std::fs::read_dir(home.path().join("downloads"))
                    .map(|entries| {
                        entries.filter_map(Result::ok).any(|e| {
                            e.file_name()
                                .to_string_lossy()
                                .starts_with(".atomic-write-")
                                && e.metadata().unwrap().len() == 4
                        })
                    })
                    .unwrap_or(false);
                if ready {
                    break;
                }
                if Instant::now() > end {
                    child.kill().unwrap();
                    panic!("partial atomic write not ready");
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
        assert_eq!(wait_exit(&mut child).code(), Some(1));
        let actual: serde_json::Value =
            serde_json::from_slice(&std::fs::read(home.path().join("result.json")).unwrap())
                .unwrap();
        row.as_object_mut().unwrap().remove("closed_connections");
        assert_eq!(actual, row);
    }
}
fn native_cli_mcp_lifecycle(stage: &str, interrupt: bool) {
    let row = rows().into_iter().find(|r| r["stage"] == stage).unwrap();
    let home = tempfile::tempdir().unwrap();
    let mut child = support::OwnedChild(
        Command::new(env!("CARGO_BIN_EXE_pixiv"))
            .arg("mcp")
            .env("HOME", home.path())
            .env_remove("PIXIV_ACCESS_TOKEN")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut input = Some(child.stdin.take().unwrap());
    writeln!(input.as_mut().unwrap(),"{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{{}},\"clientInfo\":{{\"name\":\"fixture\",\"version\":\"0\"}}}}}}").unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let (tail_tx, tail_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let _ = tx.send(line);
        let mut tail = String::new();
        reader.read_to_string(&mut tail).unwrap();
        let _ = tail_tx.send(tail);
    });
    let response = rx
        .recv_timeout(Duration::from_secs(10))
        .unwrap_or_else(|_| {
            child.kill().unwrap();
            panic!("native MCP initialize timeout")
        });
    let got: serde_json::Value = serde_json::from_str(&response).unwrap();
    let expected: serde_json::Value =
        serde_json::from_str(row["stdout"].as_str().unwrap()).unwrap();
    assert_eq!(got, expected);
    if interrupt {
        assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
    } else {
        drop(input.take());
    }
    assert_eq!(
        wait_exit(&mut child).code(),
        Some(row["exit"].as_i64().unwrap() as i32)
    );
    assert_eq!(tail_rx.recv_timeout(Duration::from_secs(5)).unwrap(), "");
    let mut diagnostics = String::new();
    use std::io::Read;
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut diagnostics)
        .unwrap();
    assert_eq!(diagnostics, row["stderr"].as_str().unwrap());
    drop(input);
}
#[test]
fn native_cli_idle_mcp_exits_on_sigint_without_stdin_eof() {
    native_cli_mcp_lifecycle("mcp-idle", true);
}
#[test]
fn native_cli_mcp_exits_successfully_after_initialized_stdin_eof() {
    native_cli_mcp_lifecycle("mcp-eof", false);
}
#[tokio::test]
async fn owner_drop_cancels_only_its_context_and_leaves_unrelated_context_live() {
    let unrelated = pixiv_app::lifecycle::Context::new();
    let owner = OwnedSignalContext::new().unwrap();
    let context = owner.context();
    assert_eq!(context.error(), None);
    drop(owner);
    assert_eq!(
        context.error(),
        Some(pixiv_app::lifecycle::ContextError::Canceled)
    );
    assert_eq!(unrelated.error(), None);
}

#[test]
fn signal_watcher_cancels_while_the_root_thread_blocks_on_synchronous_stdin() {
    let home = tempfile::tempdir().unwrap();
    let mut child = support::OwnedChild(
        Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "cli_interrupt_child", "--nocapture"])
            .env("PIXIV_INTERRUPT_CHILD", "blocked-stdin")
            .env("HOME", home.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut input = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if line.unwrap() == "READY" {
                let _ = tx.send(());
                break;
            }
        }
    });
    if rx.recv_timeout(Duration::from_secs(10)).is_err() {
        child.kill().unwrap();
        panic!("blocked stdin child READY timeout");
    }
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
    let end = Instant::now() + Duration::from_secs(5);
    while !home.path().join("cancel-observed").exists() {
        if Instant::now() > end {
            child.kill().unwrap();
            panic!("root cancellation waited for stdin EOF");
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(child.try_wait().unwrap().is_none());
    writeln!(input).unwrap();
    assert_eq!(wait_exit(&mut child).code(), Some(0));
}
