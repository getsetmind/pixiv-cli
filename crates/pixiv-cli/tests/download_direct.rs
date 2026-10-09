use pixiv_cli_rs::download::DownloadCommand;

#[test]
fn direct_download_command_preserves_the_frozen_help() {
    let rows: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/download_direct.json")).unwrap();
    let row = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "help")
        .unwrap();
    let command =
        DownloadCommand::parse(&["download".into(), "--help".into()], &mut &b""[..], false)
            .unwrap();
    let DownloadCommand::Help(text) = command else {
        panic!("help must not execute")
    };
    assert_eq!(text, row["stdout"].as_str().unwrap());
}

#[allow(dead_code)]
#[path = "support/download_fixture.rs"]
mod fixture;

#[tokio::test]
async fn direct_download_command_matches_all_frozen_go_files_outputs_and_errors() {
    use pixiv_app::lifecycle::Context;
    use pixiv_cli_rs::download::DownloadRuntime;
    use std::sync::{Arc, Mutex};
    let rows: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("fixtures/download_direct.json")).unwrap();
    for row in rows {
        let name = row["name"].as_str().unwrap();
        let root = tempfile::tempdir().unwrap();
        let replace = |s: &str| s.replace("$ROOT", &root.path().display().to_string());
        let normalize = |s: &str| s.replace(&root.path().display().to_string(), "$ROOT");
        if let Some(setup) = row["setup"].as_object() {
            for (name, body) in setup {
                let path = root.path().join(name);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, body.as_str().unwrap()).unwrap();
            }
        }
        let mut input = std::io::Cursor::new(row["input"].as_str().unwrap().as_bytes());
        let mut args = vec!["download".to_owned()];
        if let Some(values) = row["args"].as_array() {
            args.extend(values.iter().map(|v| replace(v.as_str().unwrap())));
        }
        let context = Context::new();
        if row["cancel_before"] == true {
            context.cancel();
        }
        let requests = Arc::new(Mutex::new(Vec::new()));
        let transport = fixture::Fixture {
            failure: row["failure"].as_str().unwrap().into(),
            context: context.clone(),
            requests: requests.clone(),
        };
        let account_directory = tempfile::tempdir().unwrap();
        let account_path = account_directory.path().join("config.toml");
        std::fs::write(&account_path, "").unwrap();
        let mut database = pixiv_app::database::Database::open(account_directory.path()).unwrap();
        database
            .save_pixiv_credential(&pixiv_app::database::PixivAccount::new(
                42,
                "fixture",
                b"fixture-refresh",
            ))
            .unwrap();
        let stdout = Arc::new(Mutex::new(Vec::new()));
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let out = Arc::new(Mutex::new(fixture::ObservedWriter {
            bytes: stdout.clone(),
            fail: row["writer_failure"] == "stdout",
        }));
        let err_out = Arc::new(Mutex::new(fixture::ObservedWriter {
            bytes: stderr.clone(),
            fail: row["writer_failure"] == "stderr",
        }));
        let published = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let publication = published.clone();
        let invoked = std::cell::Cell::new(false);
        let runtime_called = std::cell::Cell::new(false);
        let proxy_overrides = std::cell::RefCell::new(Vec::<String>::new());
        let parsed = DownloadCommand::parse(&args, &mut input, false);
        let result = match parsed {
            Err(error) => Err(error),
            Ok(command) => {
                command
                    .execute_with_factory(
                        &context,
                        || {
                            runtime_called.set(true);
                            Ok(DownloadRuntime {
                                download_path: replace(row["runtime_path"].as_str().unwrap()),
                                filename_template: "{author} - {title}_{id}".into(),
                                directory_template: String::new(),
                                output_json: row["runtime_json"].as_bool().unwrap(),
                            })
                        },
                        &mut input,
                        pixiv_cli_rs::download::DownloadSinks {
                            output: out,
                            error: err_out,
                        },
                        || {
                            invoked.set(true);
                            proxy_overrides.borrow_mut().push(
                                command
                                    .proxy_override()
                                    .unwrap()
                                    .unwrap_or("<unset>")
                                    .to_owned(),
                            );
                            Ok(pixiv_app::execution::Execution::new(
                                pixiv_app::config::Store::new(account_path),
                                Arc::new(Mutex::new(database)),
                                move |_| Ok(transport.clone()),
                            ))
                        },
                        move |client| {
                            Arc::new(fixture::PublicationClient {
                                inner: fixture::SaveClient(client),
                                published: published.clone(),
                            })
                        },
                    )
                    .await
            }
        };
        assert_eq!(
            normalize(&String::from_utf8(stdout.lock().unwrap().clone()).unwrap()),
            row["stdout"].as_str().unwrap(),
            "stdout {name}"
        );
        assert_eq!(
            normalize(&String::from_utf8(stderr.lock().unwrap().clone()).unwrap()),
            row["stderr"].as_str().unwrap(),
            "stderr {name}"
        );
        assert_eq!(
            result
                .as_ref()
                .err()
                .map(|e| normalize(&e.to_string()))
                .unwrap_or_default(),
            row["error"].as_str().unwrap(),
            "error {name}"
        );
        assert_eq!(
            runtime_called.get(),
            row["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|event| event == "runtime"),
            "runtime ordering {name}"
        );
        assert_eq!(
            serde_json::to_value(proxy_overrides.into_inner()).unwrap(),
            row["proxy_overrides"],
            "proxy {name}"
        );
        assert_eq!(
            matches!(&result, Err(pixiv_cli_rs::CommandError::Pipeline)),
            row["pipeline"].as_bool().unwrap(),
            "pipeline variant {name}"
        );
        let cause = match &result {
            Err(pixiv_cli_rs::CommandError::App(error)) if error.is_canceled() => "cancel",
            _ => "",
        };
        assert_eq!(cause, row["cause"].as_str().unwrap(), "typed cause {name}");
        assert_eq!(
            serde_json::to_value(if invoked.get() {
                vec![publication.load(std::sync::atomic::Ordering::SeqCst)]
            } else {
                vec![]
            })
            .unwrap(),
            row["committed"],
            "committed {name}"
        );
        assert_eq!(
            serde_json::to_value(&*requests.lock().unwrap()).unwrap(),
            row["requests"],
            "wire {name}"
        );
        fn files(
            root: &std::path::Path,
            dir: &std::path::Path,
            found: &mut std::collections::BTreeMap<String, String>,
        ) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let entry = entry.unwrap();
                if entry.file_type().unwrap().is_dir() {
                    files(root, &entry.path(), found)
                } else {
                    found.insert(
                        entry
                            .path()
                            .strip_prefix(root)
                            .unwrap()
                            .to_string_lossy()
                            .replace('\\', "/"),
                        std::fs::read_to_string(entry.path()).unwrap(),
                    );
                }
            }
        }
        let mut actual = std::collections::BTreeMap::new();
        files(root.path(), root.path(), &mut actual);
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            row["files"],
            "disk {name}"
        );
    }
}

#[test]
fn direct_download_help_does_not_require_configuration_or_startup() {
    let command =
        DownloadCommand::parse(&["download".into(), "-h".into()], &mut &b""[..], true).unwrap();
    assert!(!command.requires_config());
    assert!(!command.requires_startup());
}

#[test]
fn error_output_policy_tracks_successfully_parsed_flags_without_reading_input() {
    for (args, want) in [
        (vec!["download", "--json=false"], (false, true)),
        (vec!["download", "--ndjson=false"], (false, false)),
        (vec!["download", "--ndjson", "--json=false"], (true, true)),
        (vec!["download", "--json=false", "--unknown"], (false, true)),
        (vec!["download", "--unknown", "--json"], (false, false)),
        (vec!["download", "--json=bad"], (false, false)),
        (vec!["download", "--quality", "--json"], (false, false)),
        (vec!["download", "-j=false", "--unknown"], (false, true)),
    ] {
        let args = args.into_iter().map(str::to_owned).collect::<Vec<_>>();
        assert_eq!(
            DownloadCommand::output_policy_requested(&args),
            want,
            "{args:?}"
        );
    }
}

#[test]
fn rooted_download_help_matches_the_real_go_root_command() {
    let rows: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("fixtures/download_startup.json")).unwrap();
    let row = rows
        .iter()
        .find(|row| row["name"] == "download-help")
        .unwrap();
    let args = row["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    let command = DownloadCommand::parse(&args, &mut &b""[..], false).unwrap();
    assert_eq!(
        command.render_help("pixiv download").unwrap(),
        row["stdout"].as_str().unwrap()
    );
}

#[test]
fn root_flag_diagnostics_preserve_the_go_owner_translation() {
    let rows: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("fixtures/download_startup.json")).unwrap();
    let row = rows
        .iter()
        .find(|row| row["name"] == "unknown-flag")
        .unwrap();
    let args = row["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    let error = DownloadCommand::parse_root(&args, &mut &b""[..], false)
        .err()
        .unwrap();
    assert_eq!(format!("error: {error}\n"), row["stderr"].as_str().unwrap());
}
