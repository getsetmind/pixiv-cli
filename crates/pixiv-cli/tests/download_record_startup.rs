#![cfg(target_os = "linux")]

#[path = "support/download_record_startup.rs"]
mod support;

use pixiv_app::{
    config::Store,
    database::{Database, PixivAccount},
    execution::Execution,
    lifecycle::Context,
    scheduler::SchedulerError,
};
use pixiv_cli_rs::{
    CommandError,
    download::{DownloadCommand, DownloadRuntime, DownloadSinks},
    finish_command,
};
use serde_json::Value;
use std::{
    io::{self, Write},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use support::text;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/download_record_startup.json")).unwrap()
}

#[test]
fn record_download_native_preflight_matches_all_fourteen_frozen_go_rows() {
    let fixture = fixture();
    let mut compared = 0;
    let mut failures = vec![];
    for row in fixture["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["native"] == true)
    {
        let home = tempfile::tempdir().unwrap();
        let data = home.path().join(".pixiv-cli");
        let config = data.join("config.toml");
        if row["config_blocked"] == true {
            std::fs::create_dir_all(&config).unwrap();
        } else if let Some(before) = row["before"].as_str() {
            std::fs::create_dir_all(&data).unwrap();
            std::fs::write(&config, before).unwrap();
        }
        let mut command = Command::new(env!("CARGO_BIN_EXE_pixiv"));
        command.args(
            row["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap()),
        );
        for name in [
            "PIXIV_ACCESS_TOKEN",
            "PIXIV_REFRESH_TOKEN",
            "HTTPS_PROXY",
            "https_proxy",
            "HTTP_PROXY",
            "http_proxy",
            "ALL_PROXY",
            "all_proxy",
            "NO_PROXY",
            "no_proxy",
            "DOWNLOAD_PATH",
            "FILENAME_TEMPLATE",
            "DIRECTORY_TEMPLATE",
            "PIXIV_REQUEST_INTERVAL",
            "REQUEST_INTERVAL",
            "PIXIV_LOG_LEVEL",
            "PIXIV_LOG_FORMAT",
            "SAUCENAO_API_KEY",
            "PIXIV_CLIENT_DIR",
            "PIXIV_CONFIG_DIR",
            "PIXIV_DATA_DIR",
            "XDG_DATA_HOME",
            "XDG_CONFIG_HOME",
        ] {
            command.env_remove(name);
        }
        let mut child = support::OwnedChild::new(
            command
                .env("HOME", home.path())
                .env("USERPROFILE", home.path())
                .env("PATH", home.path())
                .current_dir(home.path())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let mut input = child.child().stdin.take().unwrap();
        input.write_all(text(row, "input").as_bytes()).unwrap();
        let held = if row["stdin_reads"] == 0 {
            Some(input)
        } else {
            drop(input);
            None
        };
        let until = Instant::now() + Duration::from_secs(5);
        while child.child().try_wait().unwrap().is_none() {
            if Instant::now() > until {
                child.child().kill().unwrap();
                child.child().wait().unwrap();
                failures.push(format!(
                    "{}: waited for unexpected input",
                    text(row, "name")
                ));
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        drop(held);
        let output = child.output().unwrap();
        let normalize = |bytes: &[u8]| {
            String::from_utf8(bytes.to_vec())
                .unwrap()
                .replace(home.path().to_str().unwrap(), "<HOME>")
        };
        let after = if config.is_file() {
            std::fs::read_to_string(&config).unwrap()
        } else {
            String::new()
        };
        if output.status.code() != Some(row["exit"].as_i64().unwrap() as i32)
            || normalize(&output.stdout) != text(row, "stdout")
            || normalize(&output.stderr) != text(row, "stderr")
            || config.is_file() != row["config"].as_bool().unwrap()
            || data.join("pixiv-cli.db").is_file() != row["database"].as_bool().unwrap()
            || after != text(row, "after")
        {
            failures.push(format!("{}: exit={:?} stdout={:?} stderr={:?} config={} database={} after={:?}; expected exit={} stdout={:?} stderr={:?} config={} database={} after={:?}", text(row,"name"), output.status.code(), normalize(&output.stdout), normalize(&output.stderr), config.is_file(), data.join("pixiv-cli.db").is_file(), after, row["exit"], text(row,"stdout"), text(row,"stderr"), row["config"], row["database"], text(row,"after")));
        }
        compared += 1;
    }
    assert_eq!(compared, 14);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn record_download_public_composition_preserves_frozen_root_observables() {
    let fixture = fixture();
    let mut compared = 0;
    let mut excluded = 0;
    let mut failures = vec![];
    for row in fixture["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["native"] != true)
    {
        if support::excluded_composition(row).is_some() {
            excluded += 1;
            continue;
        }
        let home = tempfile::tempdir().unwrap();
        let data = home.path().join(".pixiv-cli");
        let config = data.join("config.toml");
        if let Some(before) = row["before"].as_str() {
            std::fs::create_dir_all(&data).unwrap();
            std::fs::write(&config, before).unwrap();
        }
        let context = Context::new();
        if text(row, "cancel") == "before" {
            context.cancel();
        }
        let mut reader = support::Reader::new(row);
        let args = row["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        let (ndjson, machine) = DownloadCommand::output_policy_requested(&args);
        let out = Arc::new(Mutex::new(support::Writer::new(row, "stdout")));
        let diagnostics = Arc::new(Mutex::new(support::Writer::new(row, "stderr")));
        let calls = Arc::new(Mutex::new(vec![]));
        let ids = Arc::new(Mutex::new(vec![]));
        let store = Store::new(&config);
        let owned = tempfile::tempdir().unwrap();
        let owned_root = owned.path().to_owned();
        let open_count = std::cell::Cell::new(0);
        let result = async {
            let command = DownloadCommand::parse_root(&args, &mut reader, false)?;
            pixiv_cli_rs::startup::run_startup(
                &context,
                &support::Hooks {
                    row: row.clone(),
                    calls: calls.clone(),
                },
                &mut *diagnostics.lock().unwrap(),
            )?;
            store.ensure_defaults().map_err(SchedulerError::from)?;
            store
                .current()
                .and_then(|snapshot| snapshot.runtime())
                .map_err(SchedulerError::from)?;
            let port_failure = text(row, "port_failure").to_owned();
            let pool_failure = text(row, "pool_failure").to_owned();
            let connection_context = context.clone();
            let during = text(row, "cancel") == "during-pool";
            let client_ids = ids.clone();
            let save_root = owned_root.clone();
            let cancel_after = text(row, "cancel") == "after-pool";
            command
                .execute_with_factory(
                    &context,
                    || {
                        let runtime = store
                            .current()
                            .and_then(|snapshot| snapshot.runtime())
                            .map_err(SchedulerError::from)?;
                        Ok(DownloadRuntime {
                            download_path: owned_root
                                .join("outputs")
                                .to_string_lossy()
                                .into_owned(),
                            filename_template: runtime.filename_template,
                            directory_template: runtime.directory_template,
                            output_json: runtime.output_json,
                        })
                    },
                    &mut reader,
                    DownloadSinks {
                        output: out.clone(),
                        error: diagnostics.clone(),
                    },
                    || {
                        open_count.set(open_count.get() + 1);
                        if !port_failure.is_empty() {
                            return Err(CommandError::MessageText(port_failure));
                        }
                        let mut database = Database::open(owned.path()).unwrap();
                        database
                            .save_pixiv_credential(&PixivAccount::new(
                                42,
                                "owned synthetic",
                                b"owned-synthetic-refresh",
                            ))
                            .unwrap();
                        Ok(Execution::new(
                            store.clone(),
                            Arc::new(Mutex::new(database)),
                            move |_| {
                                if during {
                                    connection_context.cancel();
                                    return Err(SchedulerError::Message(
                                        "synthetic canceled action".into(),
                                    ));
                                }
                                if !pool_failure.is_empty() {
                                    return Err(SchedulerError::Message(pool_failure.clone()));
                                }
                                Ok(support::FixtureTransport)
                            },
                        ))
                    },
                    move |client| {
                        Arc::new(support::SaveClient {
                            client,
                            ids: client_ids.clone(),
                            root: save_root.clone(),
                            cancel_after,
                        })
                    },
                )
                .await
        }
        .await;
        let exit = finish_command(result, ndjson, machine, &mut *diagnostics.lock().unwrap());
        let expected_ids = row["ids"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|ids| {
                ids.as_array()
                    .unwrap()
                    .iter()
                    .map(|id| id.as_i64().unwrap())
            })
            .collect::<Vec<_>>();
        let expected_calls = row["calls"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .filter(|call| matches!(*call, "cleanup" | "supported" | "ensure"))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let stdout = String::from_utf8(out.lock().unwrap().bytes.clone()).unwrap();
        let stderr = String::from_utf8(diagnostics.lock().unwrap().bytes.clone())
            .unwrap()
            .replace(home.path().to_str().unwrap(), "<HOME>");
        let after = if config.is_file() {
            std::fs::read_to_string(&config).unwrap()
        } else {
            String::new()
        };
        if exit != row["exit"].as_i64().unwrap() as i32
            || stdout != text(row, "stdout")
            || stderr != text(row, "stderr")
            || *ids.lock().unwrap() != expected_ids
            || *calls.lock().unwrap() != expected_calls
            || config.is_file() != row["config"].as_bool().unwrap()
            || after != text(row, "after")
        {
            failures.push(format!("{}: exit={} stdout={:?} stderr={:?} ids={:?} startup={:?} config={} after={:?}; expected exit={} stdout={:?} stderr={:?} ids={:?} startup={:?} config={} after={:?}",text(row,"name"),exit,stdout,stderr,*ids.lock().unwrap(),*calls.lock().unwrap(),config.is_file(),after,row["exit"],text(row,"stdout"),text(row,"stderr"),expected_ids,expected_calls,row["config"],text(row,"after")));
        }
        if row["stdin_reads"] == 0 {
            assert_eq!(
                (reader.reads, reader.bytes),
                (0, 0),
                "explicit source input {}",
                text(row, "name")
            );
        }
        if !text(row, "port_failure").is_empty() {
            assert_eq!(
                open_count.get(),
                1,
                "cached public execution construction {}",
                text(row, "name")
            );
        }
        assert!(
            !data.join("pixiv-cli.db").exists(),
            "source composition must keep synthetic account state separately owned"
        );
        compared += 1;
    }
    assert_eq!((compared, excluded), (39, 6));
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn fatal_record_errors_match_public_finish_presentation_only() {
    let fixture = fixture();
    let mut compared = 0;
    for row in fixture["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| text(row, "pool_failure").starts_with("fatal"))
    {
        let args = row["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        let (ndjson, machine) = DownloadCommand::output_policy_requested(&args);
        let error = if text(row, "pool_failure") == "fatal-pipe" {
            io::Error::from_raw_os_error(32)
        } else {
            io::Error::other("synthetic fatal pipeline failure")
        };
        let mut diagnostics = vec![];
        let exit = finish_command(
            Err(CommandError::Output(error)),
            ndjson,
            machine,
            &mut diagnostics,
        );
        assert_eq!(
            exit,
            row["exit"].as_i64().unwrap() as i32,
            "{}",
            text(row, "name")
        );
        assert_eq!(
            diagnostics,
            text(row, "stderr").as_bytes(),
            "{}",
            text(row, "name")
        );
        compared += 1;
    }
    assert_eq!(compared, 4);
}
