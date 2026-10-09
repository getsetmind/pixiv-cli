use clap::Parser;
use pixiv_cli_rs::recommended::RecommendedOptions;
use serde::Deserialize;
use std::io::Read;

#[path = "support/recommended_process.rs"]
mod recommended_process;

#[derive(Deserialize)]
struct Case {
    args: Vec<String>,
    input: String,
    word: String,
    bytes: usize,
    error: String,
    pooled: bool,
    startup: serde_json::Value,
}

#[derive(Parser)]
#[command(args_override_self = true)]
struct Arguments {
    #[command(flatten)]
    input: RecommendedOptions,
}

struct Reader<'a> {
    input: &'a [u8],
    bytes: usize,
    allowed: bool,
}
impl Read for Reader<'_> {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        assert!(self.allowed, "explicit KIND must not consume stdin");
        let count = self.input.read(output)?;
        self.bytes += count;
        Ok(count)
    }
}

#[test]
fn recommended_text_value_matches_go_without_trimming_splitting_or_reading_explicit_kind() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/cli-recommended-stdin.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 165);
    for case in cases {
        let mut args =
            Arguments::try_parse_from(std::iter::once("pixiv".to_owned()).chain(case.args.clone()))
                .unwrap();
        let mut reader = Reader {
            input: case.input.as_bytes(),
            bytes: 0,
            allowed: args.input.query.is_empty(),
        };
        let result = args
            .input
            .resolve_source(&mut reader, false)
            .and_then(|_| args.input.validate_arguments())
            .and_then(|_| args.input.validate())
            .map(|_| args.input.query.join(" "));
        assert_eq!(reader.bytes, case.bytes, "{:?} {:?}", case.args, case.input);
        match result {
            Ok(word) => {
                assert!(
                    case.error.is_empty(),
                    "{:?} {:?}: {}",
                    case.args,
                    case.input,
                    case.error
                );
                assert!(case.pooled);
                assert_eq!(word, case.word);
            }
            Err(error) => assert_eq!(
                error.to_string(),
                case.error,
                "{:?} {:?}",
                case.args,
                case.input
            ),
        }
    }
}

#[test]
fn recommended_process_preserves_go_text_stdin_and_startup_side_effects() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/cli-recommended-stdin.json"
    ))
    .unwrap();
    for case in cases {
        recommended_process::assert_startup(&case.startup);
    }
}

#[test]
fn recommended_reader_failure_is_usage_and_explicit_kind_does_not_read() {
    struct Failed;
    impl Read for Failed {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("fixture read failed"))
        }
    }
    for values in [vec![], vec!["--type=novel"]] {
        let mut args = Arguments::try_parse_from(std::iter::once("pixiv").chain(values)).unwrap();
        assert_eq!(
            args.input
                .resolve_source(&mut Failed, false)
                .unwrap_err()
                .to_string(),
            "read stdin value: fixture read failed"
        );
    }
    let mut args = Arguments::try_parse_from(["pixiv", "novel"]).unwrap();
    args.input.resolve_source(&mut Failed, false).unwrap();
    args.input.validate().unwrap();
}
