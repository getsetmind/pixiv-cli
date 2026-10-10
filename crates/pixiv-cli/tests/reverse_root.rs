#[path = "support/reverse_search_cli.rs"]
mod producer;
#[path = "support/reverse_root.rs"]
mod support;

use pixiv_app::{execution::Execution, lifecycle::Context};
use pixiv_cli_rs::{finish_command, reverse_search};
use serde_json::json;
use std::{
    process::Stdio,
    sync::{Arc, Mutex},
};
use support::{Arguments, Owned, Reader, Startup, Writer, fixture, text};

#[cfg(target_os = "linux")]
#[test]
fn no_search_process_routes_match_go_usage_source_detection_config_and_state() {
    use pixiv_app::url_handler::{SystemUrlHandler, UrlHandler};
    assert!(!SystemUrlHandler::system().automatic_supported());
    let fixture = fixture();
    let mut compared = 0;
    for index in support::REMAINDER {
        let row = &fixture.cases[index];
        if matches!(
            row.name.as_str(),
            "stdin/read-failure" | "failure/constructor" | "startup/failure"
        ) || row.name.starts_with("keyword/saved-account/")
        {
            continue;
        }
        compared += 1;
        assert!(row.observation["requests"].as_array().unwrap().is_empty());
        assert!(row.input["saved_accounts"].as_array().unwrap().is_empty());
        let owned = Owned::new(row);
        let output = support::run_binary(&owned, row);
        assert_eq!(
            output.status.code(),
            row.observation["exit"].as_i64().map(|code| code as i32),
            "{} exit",
            row.name
        );
        assert_eq!(
            output.stdout,
            text(&row.observation, "stdout").as_bytes(),
            "{} stdout",
            row.name
        );
        assert_eq!(
            output.stderr,
            text(&row.observation, "stderr").as_bytes(),
            "{} stderr",
            row.name
        );
        owned.assert_preserved(row);
        support::assert_empty_accounts(&owned.directory, row);
    }
    assert_eq!(compared, 15);
}

#[test]
fn stdin_resolution_preserves_go_bytes_without_trimming_splitting_or_reading_dash() {
    let fixture = fixture();
    let mut compared = 0;
    for index in support::REMAINDER {
        let row = &fixture.cases[index];
        if !row.name.starts_with("stdin/") {
            continue;
        }
        compared += 1;
        let args = Arguments::parse(row);
        let mut reader = Reader {
            remaining: text(&row.input, "stdin").as_bytes(),
            failure: row.input["stdin_failure"].as_bool().unwrap_or(false),
            reads: 0,
            bytes: 0,
        };
        let result = args.input.resolve_word(&mut reader, false);
        assert_eq!(
            reader.bytes,
            row.observation["stdin_bytes"].as_u64().unwrap() as usize,
            "{} bytes",
            row.name
        );
        match result {
            Ok(source) => {
                let expected = match index {
                    16 => " image.png ",
                    17 => "image.png\nimage.png",
                    19 => {
                        r#"{"id":"42","type":"artwork","url":"https://www.pixiv.net/artworks/42"}"#
                    }
                    20 => "-",
                    _ => panic!("{} unexpectedly resolved stdin", row.name),
                };
                assert_eq!(source, expected, "{} word", row.name);
                assert!(
                    !reverse_search::is_image_source(&source),
                    "{} keyword route",
                    row.name
                );
                if index == 20 {
                    assert_eq!(reader.reads, 0);
                    assert_eq!(reader.remaining, text(&row.input, "stdin").as_bytes());
                }
            }
            Err(error) => {
                assert!(matches!(index, 18 | 21));
                let owned = Owned::new(row);
                let mut diagnostics = Writer::default();
                let exit = finish_command(
                    Err(error),
                    false,
                    args.input.machine_output(),
                    &mut diagnostics,
                );
                support::assert_result(row, &Writer::default(), &diagnostics, exit);
                owned.assert_preserved(row);
                assert_eq!(
                    reader.reads,
                    row.observation["stdin_reads"].as_u64().unwrap() as usize,
                    "{} read failure/EOF",
                    row.name
                );
            }
        }
    }
    assert_eq!(compared, 6);
}

#[test]
fn failing_startup_stops_before_handler_check_config_accounts_or_reverse_search() {
    let fixture = fixture();
    let row = &fixture.cases[129];
    assert_eq!(row.name, "startup/failure");
    let owned = Owned::new(row);
    let hooks = Startup {
        failure: true,
        ..Default::default()
    };
    let mut diagnostics = Writer::default();
    let result = pixiv_cli_rs::startup::run_startup(&Context::new(), &hooks, &mut diagnostics);
    let exit = finish_command(result, false, true, &mut diagnostics);
    support::assert_result(row, &Writer::default(), &diagnostics, exit);
    assert_eq!(*hooks.calls.lock().unwrap(), ["cleanup"]);
    assert_eq!(row.observation["startup_calls"], 1);
    assert_eq!(row.observation["handler_checks"], 0);
    owned.assert_preserved(row);
}

#[tokio::test]
async fn saved_keyword_routes_match_go_default_account_pool_requests_output_and_rotation() {
    const CHILD: &str = "PIXIV_OWNED_REVERSE_ROOT_CHILD";
    let fixture = fixture();
    if std::env::var_os(CHILD).is_none() {
        let owned = Owned::new(&fixture.cases[138]);
        let output = support::wait_owned(
            owned.command(std::env::current_exe().unwrap())
                .args(["--exact", "saved_keyword_routes_match_go_default_account_pool_requests_output_and_rotation", "--nocapture"])
                .env(CHILD, "true")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        assert!(
            output.status.success(),
            "owned saved-keyword comparison failed: {output:?}"
        );
        return;
    }
    let mut compared = 0;
    for index in support::REMAINDER {
        let row = &fixture.cases[index];
        if !row.name.starts_with("keyword/saved-account/") {
            continue;
        }
        compared += 1;
        let owned = Owned::new(row);
        let hooks = Startup::default();
        let context = Context::new();
        let mut diagnostics = Writer::default();
        pixiv_cli_rs::startup::run_startup(&context, &hooks, &mut diagnostics).unwrap();
        owned.store.ensure_defaults().unwrap();
        let runtime = owned.store.current().unwrap().runtime().unwrap();
        let args = Arguments::parse(row);
        let mut reader = Reader {
            remaining: b"must not be read",
            failure: true,
            reads: 0,
            bytes: 0,
        };
        let word = args.input.resolve_word(&mut reader, false).unwrap();
        assert_eq!(
            reader.reads,
            row.observation["stdin_reads"].as_u64().unwrap() as usize
        );
        reverse_search::validate_input(&reverse_search::Input {
            source: word.clone(),
            provider: args.input.provider.clone().unwrap_or_default(),
            ..Default::default()
        })
        .unwrap();
        assert!(!reverse_search::is_image_source(&word));
        let database = owned.seeded_database(row);
        assert_eq!(
            support::accounts(&database.lock().unwrap()),
            row.observation["accounts_before"],
            "{} initial accounts",
            row.name
        );
        let observed = Arc::new(Mutex::new(support::SdkObserved::default()));
        let factory_observed = observed.clone();
        let factory_database = database.clone();
        let body: serde_json::Value =
            serde_json::from_str(text(&row.input, "sdk_search_body")).unwrap();
        let execution = Execution::new(owned.store.clone(), database.clone(), move |connection| {
            factory_observed
                .lock()
                .unwrap()
                .constructors
                .push((connection.proxy().to_owned(), connection.pacing()));
            Ok(support::Sdk {
                body: body.clone(),
                database: factory_database.clone(),
                observed: factory_observed.clone(),
            })
        });
        let mode = args.input.output_mode(runtime.output_json, false).unwrap();
        let request = args
            .options
            .request(&word, chrono::Utc::now().fixed_offset())
            .unwrap();
        let output = support::SharedWriter::default();
        let result = pixiv_cli_rs::search::saved_artwork_search(
            &execution,
            &context,
            request,
            args.options,
            None,
            mode,
            output.clone(),
        )
        .await;
        let exit = finish_command(
            result,
            mode == pixiv_cli_rs::DetailOutput::Ndjson,
            args.input.machine_output(),
            &mut diagnostics,
        );
        support::assert_result(row, &output.0.lock().unwrap(), &diagnostics, exit);
        assert_eq!(
            support::accounts(&database.lock().unwrap()),
            row.observation["accounts_after"],
            "{} rotated accounts",
            row.name
        );
        let observed = observed.lock().unwrap();
        support::assert_sdk_requests(
            &observed.requests,
            &row.observation["sdk_requests"],
            &row.name,
        );
        assert_eq!(
            observed.constructors.len(),
            row.observation["sdk_options"].as_array().unwrap().len(),
            "{} SDK connection construction",
            row.name
        );
        for (proxy, pacing) in &observed.constructors {
            assert!(proxy.is_empty());
            assert!(pacing.is_zero());
        }
        assert_eq!(
            observed.drops,
            observed.constructors.len(),
            "{} owned transport release",
            row.name
        );
        assert_eq!(*hooks.calls.lock().unwrap(), ["cleanup", "handler-check"]);
        assert_eq!(json!(reader.bytes), row.observation["stdin_bytes"]);
        owned.assert_preserved(row);
    }
    assert_eq!(compared, 4);
}

#[tokio::test]
async fn emitted_pipeline_records_are_accepted_unchanged_by_public_read_command_inputs() {
    use pixiv_app::reverse_search::{Request, Searcher};
    use pixiv_cli_rs::{
        CommandError,
        bookmark_lists::{BookmarkListOptions, BookmarkLists},
        bookmark_reads::BookmarkDetailOptions,
    };
    use std::io::{BufRead, Cursor};

    let fixture = producer::fixture();
    assert_eq!(
        fixture.reference,
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    let mut compared = 0;
    for row in fixture.cases {
        if !row.input["consume_records"].as_bool().unwrap_or(false) {
            continue;
        }
        compared += 1;
        let owned = Owned::new(&support::Row {
            name: row.name.clone(),
            input: row.input.clone(),
            observation: row.observation.clone(),
        });
        let args = producer::Arguments::parse(&row.input);
        reverse_search::validate_input(&args.input()).unwrap();
        let runtime = producer::runtime(&row.input);
        let mode = args.mode(&runtime, &row.input);
        let provider =
            reverse_search::resolve_provider(&args.provider, &runtime.reverse_search_provider)
                .unwrap();
        let state = producer::State::new(&row.input);
        let searcher = producer::searcher(
            state.clone(),
            owned.home.path().to_owned(),
            owned.home.path().join("snapshots"),
        );
        let mut output = producer::Writer::new("", 0);
        let mut errors = producer::Writer::new("", 0);
        let result = reverse_search::run(
            &searcher,
            state.context.clone(),
            Request {
                source: args.source.clone(),
                provider,
                pixiv_only: runtime.reverse_search_pixiv_only,
            },
            mode,
            &mut output,
            Some(&mut errors),
        )
        .await;
        let cleanup = searcher.close().await.map_err(CommandError::ReverseSearch);
        let exit = producer::finish(
            pixiv_cli_rs::finish_with_cleanup(result, cleanup),
            &args,
            mode,
            &mut errors,
        );
        assert_eq!(
            exit,
            row.observation["exit"].as_i64().unwrap() as i32,
            "{} producer exit",
            row.name
        );
        assert_eq!(
            output.text(),
            text(&row.observation, "stdout"),
            "{} physical NDJSON",
            row.name
        );
        assert_eq!(
            errors.text(),
            text(&row.observation, "stderr"),
            "{} producer diagnostics",
            row.name
        );
        assert_eq!(json!(output.writes), row.observation["output_writes"]);
        assert_eq!(json!(errors.writes), row.observation["error_writes"]);
        let mut stream = Cursor::new(output.bytes.as_slice());
        let mut physical = Vec::new();
        let mut records = Vec::new();
        let mut line = Vec::new();
        loop {
            line.clear();
            if stream.read_until(b'\n', &mut line).unwrap() == 0 {
                break;
            }
            let record: serde_json::Value = serde_json::from_slice(&line).unwrap();
            let mut input = Cursor::new(line.as_slice());
            let retained = match record["type"].as_str().unwrap() {
                "artwork" => {
                    let mut options = BookmarkDetailOptions::default();
                    options.resolve_source(&mut input, false).unwrap();
                    options.resolve_target(&mut input).unwrap();
                    options.validate().unwrap();
                    assert!(!options.record_pending);
                    assert!(options.sources.is_empty());
                    options.input_record.unwrap()
                }
                "user" => {
                    let mut options = BookmarkLists::List(BookmarkListOptions::default());
                    options.resolve_source(&mut input, false).unwrap();
                    options.resolve_target(&mut input).unwrap();
                    options.validate().unwrap();
                    let BookmarkLists::List(options) = options else {
                        unreachable!()
                    };
                    assert!(!options.record_pending);
                    assert!(options.listing.sources.is_empty());
                    options.input_record.unwrap()
                }
                kind => panic!("{} unexpected emitted identity type {kind}", row.name),
            };
            assert_eq!(
                input.position() as usize,
                line.len(),
                "{} consumed line",
                row.name
            );
            assert_eq!(
                retained.as_bytes(),
                line,
                "{} retained physical bytes",
                row.name
            );
            physical.extend_from_slice(retained.as_bytes());
            records.push(serde_json::from_str::<serde_json::Value>(&retained).unwrap());
        }
        assert_eq!(physical, output.bytes, "{} physical stream order", row.name);
        assert_eq!(
            json!(records),
            row.observation["pipeline_records"],
            "{} accepted identity order",
            row.name
        );
        assert!(row.observation["pipeline_error"].is_null());
        assert!(row.observation["pipeline_stderr"].is_null());
        assert!(!owned.directory.join("pixiv-cli.db").exists());
        assert_eq!(
            std::fs::read_dir(owned.home.path().join("snapshots"))
                .unwrap()
                .count(),
            0
        );
    }
    assert_eq!(compared, 2);
}
