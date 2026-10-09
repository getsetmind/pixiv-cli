use clap::Parser;
use pixiv_cli_rs::{
    DetailOutput, finish_command,
    user_works::{UserArtworksOptions, UserWorks, UserWorksOptions, saved_user_works, user_works},
};
use pixiv_sdk::{
    Client,
    transport::{Request, Response, Transport},
};
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    io::{self, Write},
    sync::{Arc, Mutex},
};
#[path = "support/json_object_order.rs"]
mod json_object_order;
#[path = "support/saved_account.rs"]
mod saved_account;
#[derive(Parser)]
#[command(args_override_self = true)]
struct ArtworkArgs {
    #[command(flatten)]
    options: UserArtworksOptions,
    #[arg(long)]
    proxy: Option<String>,
    #[arg(long,num_args=0..=1,require_equals=true,default_missing_value="true")]
    no_proxy: Option<bool>,
}
#[derive(Parser)]
#[command(args_override_self = true)]
struct NovelArgs {
    #[command(flatten)]
    options: UserWorksOptions,
    #[arg(long)]
    proxy: Option<String>,
    #[arg(long,num_args=0..=1,require_equals=true,default_missing_value="true")]
    no_proxy: Option<bool>,
}
#[derive(Deserialize)]
struct Case {
    kind: String,
    args: Vec<String>,
    input: String,
    identity: i64,
    mode: String,
    writer: String,
    bodies: Vec<Value>,
    queries: Vec<BTreeMap<String, Vec<String>>>,
    error: String,
    stdout: String,
    stderr: String,
    exit: i32,
    bytes: usize,
    proxy: Option<String>,
    json: Option<bool>,
}
type Queries = Arc<Mutex<Vec<BTreeMap<String, Vec<String>>>>>;
#[derive(Clone)]
struct Fixture {
    bodies: Vec<Value>,
    queries: Queries,
    kind: String,
    identity: i64,
}
impl Transport for Fixture {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        if request.operation == "Open" {
            return Ok(Response {
                status: 200,
                retry_after: None,
                body: serde_json::json!({"access_token":"fixture-access","refresh_token":"fixture-rotated","expires_in":3600,"user":{"id":self.identity}}),
            });
        }
        assert_eq!(request.method.as_str(), "GET");
        assert_eq!(
            request.url,
            format!(
                "https://app-api.pixiv.net/v1/user/{}",
                if self.kind == "artworks" {
                    "illusts"
                } else {
                    "novels"
                }
            )
        );
        let mut query = BTreeMap::<String, Vec<String>>::new();
        for (k, v) in request.parameters {
            query.entry(k).or_default().push(v);
        }
        let index = usize::from(self.bodies.len() > 1 && query.contains_key("offset"));
        self.queries.lock().unwrap().push(query);
        Ok(Response {
            status: 200,
            retry_after: None,
            body: self.bodies[index].clone(),
        })
    }
}
struct Writer {
    out: Arc<Mutex<Vec<u8>>>,
    failure: String,
}
impl Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut out = self.out.lock().unwrap();
        if self.failure == "short" {
            let count = bytes.len().min(10_usize.saturating_sub(out.len()));
            out.extend_from_slice(&bytes[..count]);
            return Err(io::Error::other("short write"));
        }
        out.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn equal(actual: &[u8], expected: &str, mode: DetailOutput, partial: bool) {
    if !partial && !expected.is_empty() && mode == DetailOutput::Json {
        assert_eq!(
            json_object_order::canonicalize(actual),
            json_object_order::canonicalize(expected.as_bytes())
        );
    } else if !partial && mode == DetailOutput::Ndjson {
        assert_eq!(
            json_object_order::canonicalize_ndjson(actual),
            json_object_order::canonicalize_ndjson(expected.as_bytes())
        );
    } else {
        assert_eq!(actual, expected.as_bytes());
    }
}
#[tokio::test]
async fn user_works_preserve_go_formats_logical_windows_duplicates_identity_validation_and_writers()
{
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/cli-user-works.json"
    ))
    .unwrap();
    for case in cases {
        for saved in [false, true] {
            if saved && case.identity == 0 {
                continue;
            }
            let (mut options, proxy, no_proxy) = if case.kind == "artworks" {
                let args = ArtworkArgs::try_parse_from(
                    std::iter::once("pixiv".to_owned()).chain(case.args.clone()),
                )
                .unwrap();
                (UserWorks::Artworks(args.options), args.proxy, args.no_proxy)
            } else {
                let args = NovelArgs::try_parse_from(
                    std::iter::once("pixiv".to_owned()).chain(case.args.clone()),
                )
                .unwrap();
                (UserWorks::Novels(args.options), args.proxy, args.no_proxy)
            };
            let mut reader = io::Cursor::new(case.input.as_bytes());
            let input_result = options.resolve_source(&mut reader, false);
            assert_eq!(reader.position() as usize, case.bytes, "{:?}", case.args);
            let mode = match case.mode.as_str() {
                "json" => DetailOutput::Json,
                "ndjson" | "auto" => DetailOutput::Ndjson,
                _ => DetailOutput::Human,
            };
            let queries = Arc::new(Mutex::new(vec![]));
            let fixture = Fixture {
                bodies: case.bodies.clone(),
                queries: queries.clone(),
                kind: case.kind.clone(),
                identity: case.identity,
            };
            let output = Arc::new(Mutex::new(vec![]));
            let mut writer = Writer {
                out: output.clone(),
                failure: if case.mode == "auto" {
                    String::new()
                } else {
                    case.writer.clone()
                },
            };
            let validation = input_result
                .and_then(|_| options.validate())
                .and_then(|_| {
                    if proxy.is_some() && no_proxy.is_some() {
                        Err(pixiv_cli_rs::CommandError::Message(
                            "use either --proxy or --no-proxy, not both",
                        ))
                    } else {
                        Ok(())
                    }
                })
                .and_then(|_| options.output_mode(false, case.mode != "auto").map(|_| ()));
            let result = if let Err(error) = validation {
                Err(error)
            } else if saved {
                let account = saved_account::saved_execution(fixture);
                saved_user_works(
                    &account.execution,
                    &pixiv_app::lifecycle::Context::new(),
                    options.clone(),
                    None,
                    mode,
                    writer,
                )
                .await
            } else {
                let client = if case.identity > 0 {
                    let credentials = pixiv_sdk::oauth::refresh(&fixture, "fixture-refresh")
                        .await
                        .unwrap();
                    Client::from_credentials(&credentials, fixture)
                } else {
                    Client::with_transport("fixture-access", fixture)
                };
                user_works(&client, &options, mode, &mut writer).await
            };
            assert_eq!(
                result
                    .as_ref()
                    .err()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
                case.error,
                "{} {:?} {}",
                case.kind,
                case.args,
                case.mode
            );
            let mut diagnostics = vec![];
            assert_eq!(
                finish_command(
                    result,
                    case.mode == "ndjson" || case.mode == "auto",
                    case.mode != "human" && case.mode != "auto",
                    &mut diagnostics
                ),
                case.exit
            );
            equal(
                &output.lock().unwrap(),
                &case.stdout,
                mode,
                case.writer == "short" && case.mode != "auto",
            );
            if case.stderr.starts_with('{') {
                assert_eq!(
                    json_object_order::canonicalize(&diagnostics),
                    json_object_order::canonicalize(case.stderr.as_bytes())
                );
            } else {
                assert_eq!(diagnostics, case.stderr.as_bytes());
            }
            assert_eq!(*queries.lock().unwrap(), case.queries, "{:?}", case.args);
            if !case.queries.is_empty() {
                assert_eq!(
                    case.proxy,
                    if no_proxy == Some(true) {
                        Some(String::new())
                    } else {
                        proxy
                    }
                );
                if !options.options().ndjson {
                    assert_eq!(options.options().json, case.json);
                }
            }
        }
    }
}
