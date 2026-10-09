use clap::Parser;
use pixiv_cli_rs::{
    CommandError, DetailOutput, finish_command,
    recommended::{RecommendedOptions, recommended, saved_recommended},
};
use pixiv_sdk::{
    Client,
    transport::{JsonResponse, Request, Response, Transport},
};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
#[path = "support/json_object_order.rs"]
mod json_object_order;
#[path = "support/saved_account.rs"]
mod saved_account;

#[derive(Parser)]
#[command(args_override_self = true)]
struct Arguments {
    #[command(flatten)]
    options: RecommendedOptions,
    #[arg(long)]
    proxy: Option<String>,
    #[arg(long,num_args=0..=1,require_equals=true,default_missing_value="true")]
    no_proxy: Option<bool>,
}
#[derive(Deserialize)]
struct Case {
    args: Vec<String>,
    bodies: BTreeMap<String, Vec<serde_json::Value>>,
    queries: Vec<String>,
    error: String,
    stdout: String,
    stderr: String,
    exit: i32,
}
#[derive(Clone)]
struct Fixture {
    token: String,
    bodies: BTreeMap<String, Vec<serde_json::Value>>,
    queries: Arc<Mutex<Vec<String>>>,
}
impl Transport for Fixture {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        let response = self.send_json(request).await?;
        Ok(Response {
            status: response.status,
            retry_after: response.retry_after,
            body: serde_json::from_slice(&response.body).unwrap(),
        })
    }
    async fn send_json(&self, request: Request) -> pixiv_sdk::Result<JsonResponse> {
        assert_eq!(request.method.as_str(), "GET");
        assert_eq!(
            request
                .headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case("authorization"))
                .map(|(_, value)| value.as_str()),
            Some(format!("Bearer {}", self.token).as_str())
        );
        let url = url::Url::parse(&request.url).unwrap();
        let path = url.path();
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        let parameters: BTreeMap<_, _> = request.parameters.into_iter().collect();
        for (key, value) in &parameters {
            query.append_pair(key, value);
        }
        self.queries
            .lock()
            .unwrap()
            .push(format!("{path}?{}", query.finish()));
        let pages = &self.bodies[path];
        let index = usize::from(pages.len() > 1 && parameters.contains_key("offset"));
        Ok(JsonResponse {
            status: 200,
            retry_after: None,
            body: serde_json::to_vec(&pages[index]).unwrap(),
        })
    }
}
#[derive(Clone)]
struct Writer(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn equal(actual: &[u8], expected: &str, ndjson: bool) {
    if actual.is_empty() || !expected.starts_with('{') {
        assert_eq!(actual, expected.as_bytes());
    } else if ndjson {
        assert_eq!(
            json_object_order::canonicalize_ndjson(actual),
            json_object_order::canonicalize_ndjson(expected.as_bytes())
        );
    } else {
        assert_eq!(
            json_object_order::canonicalize(actual),
            json_object_order::canonicalize(expected.as_bytes())
        );
    }
}
#[tokio::test]
async fn recommended_matches_frozen_go_command_formats_windows_errors_and_account_execution() {
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("fixtures/cli-recommended.json")).unwrap();
    assert_eq!(cases.len(), 231);
    for case in cases {
        for saved in [false, true] {
            let args = Arguments::try_parse_from(
                std::iter::once("pixiv".to_owned()).chain(case.args.clone()),
            )
            .unwrap();
            let mode = if args.options.ndjson {
                DetailOutput::Ndjson
            } else if args.options.json == Some(true) {
                DetailOutput::Json
            } else {
                DetailOutput::Human
            };
            let queries = Arc::new(Mutex::new(vec![]));
            let output = Arc::new(Mutex::new(vec![]));
            let transport = Fixture {
                token: if saved {
                    "fixture-access-42"
                } else {
                    "fixture-access"
                }
                .into(),
                bodies: case.bodies.clone(),
                queries: queries.clone(),
            };
            let validation = args.options.validate().and_then(|_| {
                if args.proxy.is_some() && args.no_proxy.is_some() {
                    return Err(CommandError::Message(
                        "use either --proxy or --no-proxy, not both",
                    ));
                }
                args.options.output_mode(false, true).map(|_| ())
            });
            let result = if let Err(error) = validation {
                Err(error)
            } else if saved {
                let account = saved_account::saved_execution(transport);
                saved_recommended(
                    &account.execution,
                    &pixiv_app::lifecycle::Context::new(),
                    args.options,
                    None,
                    mode,
                    Writer(output.clone()),
                )
                .await
            } else {
                recommended(
                    &Client::with_transport("fixture-access", transport),
                    &args.options,
                    mode,
                    &mut Writer(output.clone()),
                )
                .await
            };
            assert_eq!(
                result
                    .as_ref()
                    .err()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
                case.error,
                "{:?} saved={saved}",
                case.args
            );
            let mut diagnostics = vec![];
            assert_eq!(
                finish_command(
                    result,
                    mode == DetailOutput::Ndjson,
                    mode != DetailOutput::Human,
                    &mut diagnostics
                ),
                case.exit,
                "{:?}",
                case.args
            );
            equal(
                &output.lock().unwrap(),
                &case.stdout,
                mode == DetailOutput::Ndjson,
            );
            equal(&diagnostics, &case.stderr, false);
            assert_eq!(*queries.lock().unwrap(), case.queries, "{:?}", case.args);
        }
    }
}

#[test]
fn recommended_invalid_utf8_stdin_preserves_go_kind_and_conflict_diagnostics() {
    for values in [vec![], vec!["--type=novel"]] {
        for input in [
            vec![0xff],
            vec![b'i', b'l', b'l', b'u', b's', b't', 0xff, b'\n'],
        ] {
            let mut args =
                Arguments::try_parse_from(std::iter::once("pixiv").chain(values.clone())).unwrap();
            args.options
                .resolve_source(&mut input.as_slice(), false)
                .unwrap();
            let error = args.options.validate().unwrap_err();
            assert_eq!(
                error.to_string(),
                if values.is_empty() {
                    "recommendation kind must be one of: all, illust, manga, novel, user"
                } else {
                    "KIND cannot be combined with --type"
                }
            );
        }
    }
}
