#[path = "support/ugoira_owner.rs"]
mod fixture;

use clap::{CommandFactory, FromArgMatches, Parser};
use pixiv_cli_rs::{
    CommandError,
    ugoira::{UgoiraOptions, ugoira},
};
use pixiv_sdk::Client;
use std::sync::{Arc, Mutex};

#[derive(Parser)]
#[command(name = "ugoira", args_override_self = true)]
struct Arguments {
    #[command(flatten)]
    options: UgoiraOptions,
    #[arg(long)]
    proxy: Option<String>,
    #[arg(long, num_args = 0..=1, require_equals = true, default_missing_value = "true")]
    no_proxy: Option<bool>,
}
fn parse(args: &[String]) -> Result<Arguments, clap::Error> {
    let matches = pixiv_cli_rs::ugoira::configure_command(Arguments::command())
        .try_get_matches_from(std::iter::once("ugoira".into()).chain(args.iter().cloned()))?;
    Arguments::from_arg_matches(&matches)
}
fn proxy(arguments: &Arguments) -> Result<Option<&str>, CommandError> {
    if arguments.proxy.is_some() && arguments.no_proxy.is_some() {
        return Err(CommandError::Message(
            "use either --proxy or --no-proxy, not both",
        ));
    }
    Ok(if arguments.no_proxy == Some(true) {
        Some("")
    } else {
        arguments.proxy.as_deref()
    })
}

fn assert_parser_error(error: &clap::Error, expected: &str, name: &str) {
    let argument = error
        .get(clap::error::ContextKind::InvalidArg)
        .expect("the rejected flag must be identified")
        .to_string();
    let normalized = pixiv_cli_rs::ugoira::argument_error(error).unwrap();
    if let Some(flag) = expected.strip_prefix("unknown flag: ") {
        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::UnknownArgument,
            "{name}"
        );
        assert_eq!(argument.split('=').next().unwrap(), flag, "{name}");
        assert_eq!(
            normalized.to_string(),
            format!("unknown option '{flag}'"),
            "{name}"
        );
        assert!(matches!(normalized, CommandError::Usage(_)), "{name}");
    } else if expected.starts_with("unknown shorthand flag:") {
        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::UnknownArgument,
            "{name}"
        );
        assert_eq!(argument, "-1", "{name}");
        assert_eq!(expected, "unknown shorthand flag: '1' in -1", "{name}");
        assert_eq!(normalized.to_string(), "unknown option '-1'", "{name}");
        assert!(matches!(normalized, CommandError::Usage(_)), "{name}");
    } else {
        assert!(
            matches!(
                error.kind(),
                clap::error::ErrorKind::ValueValidation | clap::error::ErrorKind::InvalidValue
            ),
            "{name}: {error}"
        );
        let value = error
            .get(clap::error::ContextKind::InvalidValue)
            .unwrap()
            .to_string();
        assert_eq!(
            value,
            if name == "missing-proxy-value" {
                ""
            } else {
                "invalid"
            },
            "{name}"
        );
        assert_eq!(normalized.to_string(), expected, "{name}");
        assert!(matches!(normalized, CommandError::MessageText(_)), "{name}");
    }
}

#[test]
fn owner_options_keep_exact_source_validation_flag_presence_and_supported_surface() {
    let mut exact = 0;
    let mut parser_rejections = 0;
    let mut go_only = vec![];
    for case in fixture::cases() {
        let input = &case.input;
        let expected = &case.result;
        if input.typed_metadata.is_some()
            || input.missing.is_some()
            || input.pool_error.is_some()
            || (input.json_error.is_some() && input.name != "source-before-json-resolver-error")
        {
            go_only.push(input.name.as_str().to_owned());
            continue;
        }
        assert_eq!(expected.input_reads, 0, "{}", input.name);
        let arguments = match parse(&input.args) {
            Ok(arguments) => arguments,
            Err(error) => {
                if !expected.help_output.is_empty() {
                    assert_eq!(
                        error.kind(),
                        clap::error::ErrorKind::DisplayHelp,
                        "{}",
                        input.name
                    );
                } else {
                    // Cobra owner diagnostics precede the root's shared unknown-option normalization.
                    assert_parser_error(&error, &expected.error, &input.name);
                    parser_rejections += 1;
                }
                continue;
            }
        };
        let validated = arguments.options.validate_arguments().and_then(|()| {
            proxy(&arguments)?;
            arguments.options.artwork_id().map_err(|error| {
                assert_eq!(
                    Some(fixture::error_dto(&error)),
                    expected.error_dto,
                    "{}",
                    input.name
                );
                CommandError::Usage(error.to_string())
            })
        });
        match validated {
            Err(error) => {
                assert_eq!(error.to_string(), expected.error, "{}", input.name);
                assert_eq!(
                    matches!(error, CommandError::Usage(_)),
                    expected.usage,
                    "{}",
                    input.name
                );
            }
            Ok(_) => {
                assert_eq!(
                    expected.json_overrides,
                    vec![arguments.options.json],
                    "{}",
                    input.name
                );
                assert_eq!(
                    expected.proxy_overrides,
                    vec![proxy(&arguments).unwrap().map(str::to_owned)],
                    "{}",
                    input.name
                );
                let json = arguments.options.output_json(input.configured_json);
                if let Some(write) = expected.writes.first() {
                    assert_eq!(json, write.input.starts_with('{'), "{}", input.name);
                }
            }
        }
        exact += 1;
    }
    assert_eq!((exact, parser_rejections, go_only.len()), (78, 10, 7));
    assert_eq!(
        go_only,
        vec![
            "json-resolver-error",
            "pool-error",
            "missing-json-resolver",
            "missing-pool",
            "missing-fetchers",
            "typed-stable-archive-groups-human",
            "typed-stable-archive-groups-json",
        ]
    );
    let mut command = pixiv_cli_rs::ugoira::configure_command(Arguments::command());
    command.build();
    let mut flags = command
        .get_arguments()
        .filter_map(|argument| argument.get_long())
        .collect::<Vec<_>>();
    flags.sort();
    assert_eq!(flags, vec!["help", "json", "no-proxy", "proxy"]);
}

#[tokio::test]
async fn genuine_metadata_owner_keeps_kind_preflight_safe_dto_order_timing_and_write_boundaries() {
    let mut compared = 0;
    for case in fixture::cases() {
        if case.result.requests.is_empty() || case.input.typed_metadata.is_some() {
            continue;
        }
        let arguments = parse(&case.input.args).unwrap();
        let id = arguments.options.artwork_id().unwrap();
        let requests = Arc::new(Mutex::new(vec![]));
        let client = Client::with_transport(
            "fixture-access",
            fixture::ApiTransport {
                input: case.input.clone(),
                requests: requests.clone(),
            },
        );
        let mut output = fixture::ObservingWriter::new(&case.input.writer);
        let result = ugoira(
            &client,
            id,
            arguments.options.output_json(case.input.configured_json),
            &mut output,
        )
        .await;
        let error = result.as_ref().err();
        assert_eq!(
            error.map(ToString::to_string).unwrap_or_default(),
            case.result.error,
            "{}",
            case.input.name
        );
        assert_eq!(
            error
                .and_then(CommandError::sdk_error)
                .map(fixture::error_dto),
            case.result.error_dto,
            "{}",
            case.input.name
        );
        assert_eq!(
            String::from_utf8(output.output).unwrap(),
            case.result.stdout,
            "{}",
            case.input.name
        );
        assert_eq!(output.writes, case.result.writes, "{}", case.input.name);
        assert_eq!(
            *requests.lock().unwrap(),
            case.result.requests,
            "{}",
            case.input.name
        );
        assert_eq!(
            case.result.callback_committed,
            vec![false],
            "{}",
            case.input.name
        );
        compared += 1;
    }
    assert_eq!(compared, 60);
}
