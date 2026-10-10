#[path = "support/dictionary_owner.rs"]
mod fixture;

use pixiv_app::lifecycle::ContextError;
use pixiv_cli_rs::{
    CommandError,
    dictionary::{DictionaryCommand, service::Client},
    finish_command,
};
use sha2::{Digest, Sha256};
use std::{
    io,
    sync::{Arc, Mutex},
};

#[test]
fn frozen_dictionary_owner_sources_and_complete_descriptor_surface_remain_unchanged() {
    let fixture = fixture::fixture();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    assert_eq!(fixture.sources.len(), 9);
    for (path, expected) in fixture.sources {
        let body = std::fs::read(root.join(&path)).unwrap();
        assert_eq!(format!("{:x}", Sha256::digest(body)), expected, "{path}");
    }
    assert_eq!(fixture.commands.len(), 3);
    let surface = fixture.commands.into_iter().map(|command| {
        assert_eq!(command["aliases"], serde_json::json!([]));
        assert_eq!(command["inherited"], serde_json::json!([]));
        assert_eq!(command["requirements"], serde_json::json!({"StartupHooks":true,"EnsureConfig":true,"AutomaticUpdate":true,"MCP":false}));
        (command["use"].as_str().unwrap().to_owned(), command["flags"].as_array().unwrap().iter().map(|flag|flag["name"].as_str().unwrap().to_owned()).collect::<Vec<_>>())
    }).collect::<Vec<_>>();
    assert_eq!(
        surface,
        vec![
            ("dic".into(), vec!["help".into()]),
            (
                "article <title|url>".into(),
                vec![
                    "help".into(),
                    "json".into(),
                    "lang".into(),
                    "no-counters".into()
                ]
            ),
            (
                "search <query>".into(),
                vec![
                    "help".into(),
                    "json".into(),
                    "limit".into(),
                    "ndjson".into(),
                    "page".into()
                ]
            ),
        ]
    );
}

#[test]
fn parser_preserves_cobra_discovery_exact_arity_scalar_errors_partial_flags_and_help() {
    let mut help = 0;
    let mut root_unknown = 0;
    let mut parser_errors = 0;
    for case in fixture::fixture().rows {
        let input = &case.input;
        let expected = &case.result;
        assert_eq!(expected.input_reads, 0, "{}", input.name);
        assert!(!input.input.is_empty(), "{}", input.name);
        assert_eq!(
            DictionaryCommand::output_policy_requested(&input.args),
            (
                expected.ndjson_flag,
                expected.json_changed || expected.ndjson_flag
            ),
            "{}",
            input.name
        );
        match DictionaryCommand::parse(&input.args) {
            Err(error) => {
                assert_eq!(error.to_string(), expected.error, "{}", input.name);
                assert!(!matches!(error, CommandError::Usage(_)), "{}", input.name);
                assert!(expected.requests.is_empty(), "{}", input.name);
                assert!(expected.json_overrides.is_empty(), "{}", input.name);
                let root = DictionaryCommand::parse_root(&input.args).unwrap_err();
                if let Some(flag) = expected.error.strip_prefix("unknown flag: ") {
                    assert!(matches!(root, CommandError::Usage(_)), "{}", input.name);
                    assert_eq!(
                        root.to_string(),
                        format!("unknown option '{flag}'"),
                        "{}",
                        input.name
                    );
                    root_unknown += 1;
                } else if expected.error.starts_with("unknown shorthand flag: ") {
                    let flag = expected.error.split('\'').nth(1).unwrap();
                    assert!(matches!(root, CommandError::Usage(_)), "{}", input.name);
                    assert_eq!(
                        root.to_string(),
                        format!("unknown option '-{flag}'"),
                        "{}",
                        input.name
                    );
                    root_unknown += 1;
                } else {
                    assert_eq!(root.to_string(), expected.error, "{}", input.name);
                    assert!(!matches!(root, CommandError::Usage(_)), "{}", input.name);
                }
                parser_errors += 1;
            }
            Ok(command) => {
                if let Some(text) = command.render_help("dic") {
                    assert_eq!(text, expected.help_output, "{}", input.name);
                    assert_eq!(
                        command.requires_runtime(),
                        input.name == "group-bare",
                        "{}",
                        input.name
                    );
                    help += 1;
                } else {
                    assert!(command.requires_runtime(), "{}", input.name);
                    assert_eq!(
                        command.ndjson_output(),
                        expected.ndjson_flag,
                        "{}",
                        input.name
                    );
                    assert_eq!(
                        command.machine_output(),
                        expected.json_changed || expected.ndjson_flag,
                        "{}",
                        input.name
                    );
                    if let Err(error) = command.validate_options() {
                        assert_eq!(error.to_string(), expected.error, "{}", input.name);
                        assert_eq!(
                            matches!(error, CommandError::Usage(_)),
                            expected.usage,
                            "{}",
                            input.name
                        );
                        assert!(expected.requests.is_empty(), "{}", input.name);
                    }
                }
            }
        }
        assert!(
            !expected.json_flag || expected.json_changed,
            "{}",
            input.name
        );
        assert!(
            matches!(
                expected.selected.as_str(),
                "dic" | "dic article" | "dic search"
            ),
            "{}",
            input.name
        );
    }
    assert_eq!(help, 6);
    assert!(parser_errors > 25);
    assert!(root_unknown > 10);
}

#[tokio::test]
async fn real_anonymous_client_owner_matches_all_outputs_fetch_resolver_order_and_writer_boundaries()
 {
    let mut compared = 0;
    let mut go_only = vec![];
    for case in fixture::fixture().rows {
        let input = &case.input;
        let expected = &case.result;
        let command = match DictionaryCommand::parse(&input.args) {
            Ok(command) => command,
            Err(_) => continue,
        };
        if command.render_help("dic").is_some() {
            continue;
        }
        if matches!(
            input.reader_mode.as_str(),
            "missing-reader" | "injected-reader-error"
        ) && command.validate_options().is_ok()
        {
            go_only.push(input.name.clone());
            continue;
        }
        let context = fixture::context(&input.context);
        let observed = Arc::new(Mutex::new(fixture::Observed::default()));
        let transport = fixture::ApiTransport {
            responses: input.responses.clone(),
            context_identity: &context as *const _ as usize,
            observed: observed.clone(),
        };
        let client = Client::new((input.reader_mode == "real-client").then_some(transport));
        let mut output = fixture::ObservingWriter {
            mode: input.writer.clone(),
            output: vec![],
            observed: observed.clone(),
        };
        let retained = observed.clone();
        let result = command
            .execute(&client, &context, &mut output, |override_value| {
                if input.missing_json_out {
                    return Ok(override_value.unwrap_or(false));
                }
                let mut state = retained.lock().unwrap();
                state.events.push("json-resolve".into());
                state.json_overrides.push(override_value);
                if !input.json_error.is_empty() {
                    return Err(CommandError::MessageText(input.json_error.clone()));
                }
                Ok(override_value.unwrap_or(input.configured_json))
            })
            .await;
        let error = result.as_ref().err();
        assert_eq!(
            error.map(ToString::to_string).unwrap_or_default(),
            expected.error,
            "{}",
            input.name
        );
        assert_eq!(
            error.is_some_and(|error| matches!(error, CommandError::Usage(_))),
            expected.usage,
            "{}",
            input.name
        );
        assert_eq!(error.is_some_and(|error|matches!(error, CommandError::Output(cause) if cause.kind()==io::ErrorKind::BrokenPipe)), expected.error_broken_pipe, "{}", input.name);
        assert_eq!(
            error.and_then(|error| fixture::context_source(error)),
            expected
                .error_canceled
                .then_some(ContextError::Canceled)
                .or(expected
                    .error_deadline
                    .then_some(ContextError::DeadlineExceeded)),
            "{}",
            input.name
        );
        assert_eq!(
            error.is_some_and(|error| fixture::source_is::<fixture::SyntheticCause>(error)),
            expected.error_cause,
            "{}",
            input.name
        );
        assert_eq!(
            error.and_then(CommandError::sdk_error).is_some(),
            !expected.sdk_reason.is_empty(),
            "{}",
            input.name
        );
        let classified = error
            .and_then(|error| fixture::source::<pixiv_cli_rs::dictionary::service::Error>(error));
        assert_eq!(
            classified
                .map(|error| error.code().as_str())
                .unwrap_or("unknown"),
            expected.dic_code,
            "{}",
            input.name
        );
        assert_eq!(
            classified.map(|error| error.status_code()).unwrap_or(0),
            expected.http_status,
            "{}",
            input.name
        );
        if !matches!(expected.dic_code.as_str(), "unknown") {
            let error = error.expect("typed dictionary error must be retained");
            assert!(matches!(error, CommandError::State(_)), "{}", input.name);
            assert!(
                fixture::source_is::<pixiv_cli_rs::dictionary::service::Error>(error),
                "{}",
                input.name
            );
            assert_eq!(error.code(), "command_failed", "{}", input.name);
        } else {
            assert_eq!(expected.http_status, 0, "{}", input.name);
        }
        assert_eq!(
            String::from_utf8(output.output).unwrap(),
            expected.stdout,
            "{}",
            input.name
        );
        let state = observed.lock().unwrap();
        assert_eq!(state.requests, expected.requests, "{}", input.name);
        assert_eq!(state.events, expected.events, "{}", input.name);
        assert_eq!(
            state.json_overrides, expected.json_overrides,
            "{}",
            input.name
        );
        assert_eq!(state.writes, expected.writes, "{}", input.name);
        assert!(expected.diagnostics.is_empty(), "{}", input.name);
        compared += 1;
    }
    assert_eq!(
        go_only,
        vec![
            "article-injected-reader-port-failure",
            "article-missing-reader-port",
            "search-injected-reader-port-failure",
            "search-missing-reader-port"
        ]
    );
    assert!(compared > 150);
}

#[tokio::test]
async fn owner_errors_keep_command_failed_envelopes_and_only_explicit_ndjson_suppresses_epipe() {
    for name in [
        "article-upstream-404",
        "search-upstream-500",
        "writer-article-json-broken-pipe",
        "writer-search-ndjson-broken-pipe",
    ] {
        let case = fixture::fixture()
            .rows
            .into_iter()
            .find(|case| case.input.name == name)
            .unwrap();
        let command = DictionaryCommand::parse_root(&case.input.args).unwrap();
        let context = fixture::context(&case.input.context);
        let observed = Arc::new(Mutex::new(fixture::Observed::default()));
        let client = Client::new(Some(fixture::ApiTransport {
            responses: case.input.responses.clone(),
            context_identity: &context as *const _ as usize,
            observed: observed.clone(),
        }));
        let mut output = fixture::ObservingWriter {
            mode: case.input.writer.clone(),
            output: vec![],
            observed,
        };
        let result = command
            .execute(&client, &context, &mut output, |value| {
                Ok(value.unwrap_or(false))
            })
            .await;
        let mut diagnostics = Vec::new();
        let exit = finish_command(
            result,
            command.ndjson_output(),
            command.machine_output(),
            &mut diagnostics,
        );
        if command.ndjson_output() {
            assert_eq!(exit, 0, "{name}");
            assert!(diagnostics.is_empty(), "{name}");
        } else {
            assert_eq!(exit, 1, "{name}");
            let envelope: serde_json::Value = serde_json::from_slice(&diagnostics).unwrap();
            assert_eq!(
                envelope,
                serde_json::json!({"error":{"code":"command_failed","message":case.result.error}}),
                "{name}"
            );
        }
    }
}

#[test]
fn embedded_dictionary_help_matches_the_real_go_root_bytes() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/cli-dictionary-startup.json")).unwrap();
    let mut compared = 0;
    for row in fixture["rows"].as_array().unwrap() {
        if row["stdout"].as_str().unwrap().is_empty() {
            continue;
        }
        let args = row["args"]
            .as_array()
            .unwrap()
            .iter()
            .skip(1)
            .map(|value| value.as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        if let Ok(command) = DictionaryCommand::parse_root(&args)
            && let Some(text) = command.render_help("pixiv dic")
        {
            assert_eq!(text, row["stdout"].as_str().unwrap(), "{}", row["name"]);
            compared += 1;
        }
    }
    assert!(compared >= 4);
}

struct HelpWriter {
    mode: String,
    stream: &'static str,
    output: Vec<u8>,
    writes: Vec<serde_json::Value>,
    events: Arc<Mutex<Vec<String>>>,
}

impl std::io::Write for HelpWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let call = self.writes.len() + 1;
        let chosen = match self.mode.as_str() {
            "fail-first" | "partial-error-first" | "broken-pipe-first" => call == 1,
            "fail-second" | "partial-error-second" | "broken-pipe-second" => call == 2,
            "fail-third" | "partial-error-third" | "broken-pipe-third" => call == 3,
            _ => false,
        };
        let error = if chosen {
            Some(if self.mode.starts_with("broken-pipe-") {
                io::ErrorKind::BrokenPipe
            } else {
                io::ErrorKind::Other
            })
        } else {
            None
        };
        let count = if chosen {
            if self.mode.starts_with("partial-error-") {
                bytes.len().min(5)
            } else {
                0
            }
        } else {
            match self.mode.as_str() {
                "short-nil" => bytes.len() / 2,
                "zero-nil" => 0,
                _ => bytes.len(),
            }
        };
        let message = match error {
            Some(io::ErrorKind::BrokenPipe) => "broken pipe",
            Some(_) => "synthetic dictionary help writer failure",
            None => "",
        };
        self.output.extend_from_slice(&bytes[..count]);
        self.events
            .lock()
            .unwrap()
            .push(format!("{}:{call}", self.stream));
        self.writes.push(serde_json::json!({"input":String::from_utf8(bytes.to_vec()).unwrap(),"count":count,"error":message}));
        match error {
            Some(kind) => Err(io::Error::new(kind, message)),
            None => Ok(count),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn real_go_help_writer_contract_keeps_three_writes_ignores_short_counts_and_all_failures() {
    let captured = include_bytes!("fixtures/cli-dictionary-help-writer.json");
    assert_eq!(
        format!("{:x}", Sha256::digest(captured)),
        "6c3f89c690725aa70d5da16967e21d4462d4a14275115548776a5941f4898f0c"
    );
    let fixture: serde_json::Value = serde_json::from_slice(captured).unwrap();
    assert_eq!(
        fixture["reference"],
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    assert_eq!(fixture["cobra"]["version"], "v1.10.1");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for (path, expected) in fixture["sources"].as_object().unwrap() {
        assert_eq!(
            format!(
                "{:x}",
                Sha256::digest(std::fs::read(root.join(path)).unwrap())
            ),
            expected.as_str().unwrap(),
            "{path}"
        );
    }
    let rows = fixture["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 56);
    for case in rows {
        let input = &case["input"];
        let expected = &case["result"];
        let name = input["name"].as_str().unwrap();
        let args = input["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        let command = DictionaryCommand::parse(&args).unwrap();
        assert!(command.render_help("dic").is_some(), "{name}");
        let events = Arc::new(Mutex::new(vec![]));
        let mut output = HelpWriter {
            mode: input["stdout_writer"].as_str().unwrap().into(),
            stream: "stdout",
            output: vec![],
            writes: vec![],
            events: events.clone(),
        };
        let mut diagnostics = HelpWriter {
            mode: input["stderr_writer"].as_str().unwrap().into(),
            stream: "stderr",
            output: vec![],
            writes: vec![],
            events: events.clone(),
        };
        let client = Client::<fixture::ApiTransport>::new(None);
        let result = command
            .execute(
                &client,
                &pixiv_app::lifecycle::Context::new(),
                &mut output,
                |_| panic!("help must not resolve JSON"),
            )
            .await;
        assert_eq!(
            result
                .as_ref()
                .err()
                .map(ToString::to_string)
                .unwrap_or_default(),
            expected["error"].as_str().unwrap(),
            "{name}"
        );
        assert_eq!(expected["error_broken_pipe"], false, "{name}");
        assert_eq!(
            finish_command(result, false, false, &mut diagnostics),
            0,
            "{name}"
        );
        assert_eq!(
            String::from_utf8(output.output).unwrap(),
            expected["stdout"].as_str().unwrap(),
            "{name}"
        );
        assert_eq!(
            String::from_utf8(diagnostics.output).unwrap(),
            expected["stderr"].as_str().unwrap(),
            "{name}"
        );
        assert_eq!(
            serde_json::Value::Array(output.writes),
            expected["stdout_writes"],
            "{name}"
        );
        assert_eq!(
            serde_json::Value::Array(diagnostics.writes),
            expected["stderr_writes"],
            "{name}"
        );
        assert_eq!(
            serde_json::json!(*events.lock().unwrap()),
            expected["events"],
            "{name}"
        );
        for key in [
            "input_reads",
            "reader_calls",
            "json_out_calls",
            "usage_error_calls",
        ] {
            assert_eq!(expected[key], 0, "{name} {key}");
        }
        let mut complete = Vec::new();
        command.write_help("dic", &mut complete);
        assert_eq!(
            String::from_utf8(complete).unwrap(),
            command.render_help("dic").unwrap(),
            "{name}"
        );
    }
}
