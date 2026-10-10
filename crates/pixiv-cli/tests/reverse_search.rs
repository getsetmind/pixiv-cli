#[path = "support/reverse_search_cli.rs"]
mod support;

use pixiv_app::reverse_search::{
    ErrorCode, Evidence, Input as SearchSource, PixivRef, PixivRefType, Provider, ProviderError,
    Request, Response, SearchResult, Searcher, SourceKind,
};
use pixiv_cli_rs::{CommandError, DetailOutput, reverse_search};
use serde_json::{Value, json};
use std::io;
use support::{Arguments, Writer, fixture, string};

#[tokio::test]
async fn image_invocations_connect_go_sources_aggregation_outputs_faults_and_cleanup() {
    let fixture = fixture();
    assert_eq!(
        fixture.reference,
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    assert_eq!(fixture.cases.len(), 148);
    let mut compared = 0;
    for row in fixture.cases {
        if row.observation["requests"].as_array().unwrap().is_empty() {
            continue;
        }
        compared += 1;
        let root = tempfile::tempdir().unwrap();
        let snapshots = root.path().join("snapshots");
        std::fs::create_dir(&snapshots).unwrap();
        for filename in ["image.png", "owned image.png", "ftp:owned.png"] {
            std::fs::write(root.path().join(filename), support::PAYLOAD).unwrap();
        }
        let args = Arguments::parse(&row.input);
        let runtime = support::runtime(&row.input);
        let mode = args.mode(&runtime, &row.input);
        let provider =
            reverse_search::resolve_provider(&args.provider, &runtime.reverse_search_provider)
                .unwrap();
        let state = support::State::new(&row.input);
        let searcher = support::searcher(state.clone(), root.path().to_owned(), snapshots.clone());
        let mut output = Writer::new(
            string(&row.input, "writer"),
            row.input["write_limit"].as_u64().unwrap_or(0) as usize,
        );
        let mut errors = Writer::new(string(&row.input, "warning_writer"), 0);
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
        let result = pixiv_cli_rs::finish_with_cleanup(result, cleanup);
        let exit = support::finish(result, &args, mode, &mut errors);
        assert_eq!(
            exit,
            row.observation["exit"].as_i64().unwrap() as i32,
            "{} exit",
            row.name
        );
        assert_eq!(
            output.text(),
            string(&row.observation, "stdout"),
            "{} stdout",
            row.name
        );
        assert_eq!(
            errors.text(),
            string(&row.observation, "stderr"),
            "{} stderr",
            row.name
        );
        assert_eq!(
            json!(output.writes),
            row.observation["output_writes"],
            "{} writes",
            row.name
        );
        assert_eq!(
            json!(errors.writes),
            row.observation["error_writes"],
            "{} diagnostic writes",
            row.name
        );
        let observed = state.observed.lock().unwrap();
        for field in [
            "requests",
            "search_responses",
            "search_errors",
            "source_requests",
            "source_body_reads",
            "source_body_closes",
            "providers",
            "close_order",
            "searcher_closes",
        ] {
            assert_eq!(
                observed[field], row.observation[field],
                "{} {field}",
                row.name
            );
        }
        drop(observed);
        assert_eq!(
            std::fs::read_dir(&snapshots).unwrap().count(),
            0,
            "{} snapshot cleanup",
            row.name
        );
        if let Some(snapshot) = state.snapshot.lock().unwrap().as_ref() {
            assert_eq!(
                snapshot.open().unwrap_err().to_string(),
                string(&row.observation, "snapshot_reopen_error"),
                "{} snapshot reader",
                row.name
            );
        }
        for filename in ["image.png", "owned image.png", "ftp:owned.png"] {
            assert_eq!(
                std::fs::read(root.path().join(filename)).unwrap(),
                support::PAYLOAD,
                "{} source remains unchanged",
                row.name
            );
        }
        assert!(
            !root.path().join(".pixiv-cli").exists(),
            "{} no accounts were opened",
            row.name
        );
        for secret in [
            "synthetic-source-secret",
            "synthetic-api-key-secret",
            "synthetic-upstream-body-secret",
            "synthetic-csrf-secret",
            "synthetic-location-secret",
        ] {
            assert!(
                !format!("{}{}", output.text(), errors.text()).contains(secret),
                "{} safe diagnostics",
                row.name
            );
        }
        if row.input["consume_records"].as_bool().unwrap_or(false) {
            let records = output
                .text()
                .lines()
                .map(|line| serde_json::from_str::<Value>(line).unwrap())
                .collect::<Vec<_>>();
            assert_eq!(
                json!(records),
                row.observation["pipeline_records"],
                "{} interoperable records",
                row.name
            );
            for record in records {
                let id = record["id"].as_str().unwrap().parse::<i64>().unwrap();
                assert_eq!(
                    pixiv_record::from_identity(
                        id,
                        record["type"].as_str().unwrap(),
                        record["url"].as_str().unwrap()
                    )
                    .unwrap(),
                    record
                );
            }
        }
    }
    assert_eq!(compared, 101);
}

#[test]
fn image_validation_keeps_go_flag_presence_provider_case_and_fixed_precedence() {
    let fixture = fixture();
    let mut compared = 0;
    for row in fixture.cases {
        if !(row.name.starts_with("unsupported-flag/")
            || row.name.starts_with("provider/unknown")
            || row.name.starts_with("provider/SauceNAO")
            || row.name.starts_with("flag-conflict/--json")
            || row.name.starts_with("keyword/provider-rejected/"))
        {
            continue;
        }
        compared += 1;
        let args = Arguments::parse(&row.input);
        let error = reverse_search::validate_input(&args.input()).unwrap_err();
        let mut errors = Writer::new("", 0);
        assert_eq!(
            pixiv_cli_rs::finish_command(Err(error), false, true, &mut errors),
            row.observation["exit"].as_i64().unwrap() as i32,
            "{} exit",
            row.name
        );
        assert_eq!(
            errors.text(),
            string(&row.observation, "stderr"),
            "{} diagnostic",
            row.name
        );
        assert_eq!(
            json!(errors.writes),
            row.observation["error_writes"],
            "{} writes",
            row.name
        );
    }
    assert_eq!(compared, 25);
    let mut input = reverse_search::Input {
        source: "https:owned".into(),
        provider: "unknown".into(),
        changed_flags: vec!["page".into(), "sort".into()],
        ..Default::default()
    };
    assert_eq!(
        reverse_search::validate_input(&input)
            .unwrap_err()
            .to_string(),
        "--sort is not supported for image sources"
    );
    input.changed_flags.push("ndjson".into());
    input.json_changed = true;
    assert_eq!(
        reverse_search::validate_input(&input)
            .unwrap_err()
            .to_string(),
        "--ndjson cannot be used with --json"
    );
    assert_eq!(
        reverse_search::resolve_provider("", "all").unwrap(),
        Provider::All
    );
    assert_eq!(
        reverse_search::resolve_provider("", "").unwrap(),
        Provider::Unspecified
    );
    assert_eq!(
        reverse_search::resolve_provider("saucenao", "all").unwrap(),
        Provider::SauceNao
    );
    assert!(reverse_search::resolve_provider("SauceNAO", "all").is_err());
}

#[test]
fn source_routing_uses_http_prefix_or_regular_file_without_trimming() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("ftp:owned.png");
    std::fs::write(&file, b"owned").unwrap();
    assert!(reverse_search::is_image_source(file.to_str().unwrap()));
    assert!(!reverse_search::is_image_source(
        directory.path().to_str().unwrap()
    ));
    for source in ["http:", "https://", "HTTP://owned.invalid", "https:owned"] {
        assert!(reverse_search::is_image_source(source));
    }
    for source in [
        "keyword",
        "missing.png",
        "ftp://owned.invalid/image.png",
        " https://owned.invalid",
    ] {
        assert!(!reverse_search::is_image_source(source));
    }
    #[cfg(unix)]
    {
        let link = directory.path().join("link");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        assert!(reverse_search::is_image_source(link.to_str().unwrap()));
    }
}

#[test]
fn output_normalizes_outer_nil_arrays_but_preserves_nil_evidence_and_int64_records() {
    let response = Response {
        results: Some(vec![SearchResult {
            pixiv: Some(PixivRef {
                kind: PixivRefType::Artwork,
                id: i64::MAX,
            }),
            title: "<&>\u{2028}\u{2029}".into(),
            ..Default::default()
        }]),
        ..Default::default()
    };
    let mut output = Writer::new("", 0);
    reverse_search::write_response(&response, DetailOutput::Json, &mut output).unwrap();
    let expected = String::from(
        "{\n  \"input\": {\n    \"kind\": \"\",\n    \"sha256\": \"\"\n  },\n  \"providers\": [],\n  \"results\": [\n    {\n      \"pixiv\": {\n        \"type\": \"artwork\",\n        \"id\": 9223372036854775807\n      },\n      \"title\": \"\\u003c\\u0026\\u003e\\u2028\\u2029\",\n      \"evidence\": null\n    }\n  ],\n  \"records\": [\n    {\n      \"id\": \"9223372036854775807\",\n      \"type\": \"artwork\",\n      \"url\": \"https://www.pixiv.net/artworks/9223372036854775807\"\n    }\n  ],\n  \"provider_errors\": [],\n  \"partial\": false\n}\n",
    );
    assert_eq!(output.text(), expected);
    assert_eq!(output.writes, vec![expected.len()]);
    assert!(!reverse_search::has_response_data(&Response::default()));
    assert!(!reverse_search::has_response_data(&Response {
        providers: Some(vec![]),
        results: Some(vec![]),
        provider_errors: Some(vec![]),
        ..Default::default()
    }));
    assert!(reverse_search::has_response_data(&Response {
        input: SearchSource {
            kind: SourceKind::Url,
            sha256: String::new()
        },
        ..Default::default()
    }));
    assert!(reverse_search::has_response_data(&Response {
        partial: true,
        ..Default::default()
    }));
}

#[test]
fn invalid_identity_fails_before_any_output_even_after_valid_results() {
    for kind in [PixivRefType::Other("novel".into()), PixivRefType::Artwork] {
        for mode in [
            DetailOutput::Human,
            DetailOutput::Json,
            DetailOutput::Ndjson,
        ] {
            let response = Response {
                results: Some(vec![
                    SearchResult {
                        pixiv: Some(PixivRef {
                            kind: PixivRefType::Artwork,
                            id: 42,
                        }),
                        ..Default::default()
                    },
                    SearchResult {
                        pixiv: Some(PixivRef {
                            kind: kind.clone(),
                            id: 0,
                        }),
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            };
            let mut output = Writer::new("", 0);
            assert_eq!(
                reverse_search::write_response(&response, mode, &mut output)
                    .unwrap_err()
                    .to_string(),
                "reverse search returned an invalid Pixiv identity"
            );
            assert!(output.writes.is_empty());
        }
    }
}

#[test]
fn go_float_spelling_is_exact_without_integer_identity_float_projection() {
    for (number, expected) in [
        (-0.0, "-0"),
        (1.0, "1"),
        (1e-7, "1e-7"),
        (1e-6, "0.000001"),
        (1e20, "100000000000000000000"),
        (1e21, "1e+21"),
        (1.2345678901234567, "1.2345678901234567"),
    ] {
        let response = Response {
            results: Some(vec![SearchResult {
                evidence: Some(vec![Evidence {
                    provider: Provider::SauceNao,
                    rank: 1,
                    similarity: number,
                    index_id: 5,
                    index_name: String::new(),
                    title: String::new(),
                    author: String::new(),
                    external_urls: vec![],
                }]),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let mut output = Writer::new("", 0);
        reverse_search::write_response(&response, DetailOutput::Json, &mut output).unwrap();
        assert!(
            output
                .text()
                .contains(&format!("\"similarity\": {expected},")),
            "{number}: {}",
            output.text()
        );
    }
    for number in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let response = Response {
            results: Some(vec![SearchResult {
                evidence: Some(vec![Evidence {
                    provider: Provider::SauceNao,
                    rank: 1,
                    similarity: number,
                    index_id: 0,
                    index_name: String::new(),
                    title: String::new(),
                    author: String::new(),
                    external_urls: vec![],
                }]),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let mut output = Writer::new("", 0);
        assert!(
            reverse_search::write_response(&response, DetailOutput::Json, &mut output).is_err()
        );
        assert!(output.writes.is_empty());
    }
}

#[test]
fn partial_warning_has_safe_fallback_and_independent_writer_failure() {
    let mut output = Writer::new("", 0);
    reverse_search::write_warning(
        &Response {
            partial: true,
            ..Default::default()
        },
        Some(&mut output),
    )
    .unwrap();
    assert_eq!(
        output.text(),
        "warning: reverse search completed partially; failed providers: unknown provider\n"
    );
    assert_eq!(output.writes, vec![output.bytes.len()]);
    let error = reverse_search::write_warning::<Vec<u8>>(&Response::default(), None).unwrap_err();
    assert_eq!(
        error.to_string(),
        "reverse search partial warning output is not configured"
    );
    let response = Response {
        provider_errors: Some(vec![
            ProviderError {
                provider: Provider::Ascii2dColor,
                code: ErrorCode::SolverUnavailable,
                message: "private cause".into(),
            },
            ProviderError {
                provider: Provider::SauceNao,
                code: ErrorCode::ProviderFailed,
                message: "private cause".into(),
            },
        ]),
        partial: true,
        ..Default::default()
    };
    let mut output = Writer::new("", 0);
    reverse_search::write_warning(&response, Some(&mut output)).unwrap();
    assert_eq!(
        output.text(),
        "warning: reverse search completed partially; failed providers: ascii2d-color, saucenao\n"
    );
    let mut output = Writer::new("pipe", 0);
    let error = reverse_search::write_warning(&response, Some(&mut output)).unwrap_err();
    assert!(
        matches!(error,CommandError::Output(ref error) if error.kind()==io::ErrorKind::BrokenPipe)
    );
}
