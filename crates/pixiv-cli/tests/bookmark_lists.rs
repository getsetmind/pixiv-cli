use clap::Parser;
use pixiv_cli_rs::{
    DetailOutput,
    bookmark_lists::{
        BookmarkListOptions, BookmarkLists, UserBookmarksOptions, bookmark_lists,
        saved_bookmark_lists,
    },
    finish_command,
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
struct ListArgs {
    #[command(flatten)]
    options: BookmarkListOptions,
    #[arg(long)]
    proxy: Option<String>,
    #[arg(long,num_args=0..=1,require_equals=true,default_missing_value="true")]
    no_proxy: Option<bool>,
}
#[derive(Parser)]
#[command(args_override_self = true)]
struct UserArgs {
    #[command(flatten)]
    options: UserBookmarksOptions,
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
    #[serde(default)]
    input_bytes: Option<String>,
    #[serde(default)]
    read_error: bool,
    identity: i64,
    mode: String,
    writer: String,
    bodies: Vec<Value>,
    queries: Vec<BTreeMap<String, Vec<String>>>,
    paths: Vec<String>,
    committed: Vec<bool>,
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
    paths: Arc<Mutex<Vec<String>>>,
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
        assert!(request.headers.iter().any(
            |(key, value)| key.eq_ignore_ascii_case("authorization")
                && matches!(
                    value.as_str(),
                    "Bearer fixture-access" | "Bearer fixture-access-42"
                )
        ));
        let path = request
            .url
            .strip_prefix("https://app-api.pixiv.net")
            .unwrap();
        assert!(matches!(
            path,
            "/v1/user/bookmarks/illust" | "/v1/user/bookmarks/novel"
        ));
        self.paths.lock().unwrap().push(path.to_owned());
        let mut query = BTreeMap::<String, Vec<String>>::new();
        for (k, v) in request.parameters {
            query.entry(k).or_default().push(v);
        }
        let index = usize::from(self.kind == "all" && path.ends_with("novel")) * 2
            + usize::from(query.contains_key("max_bookmark_id"));
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
fn equal(actual: &[u8], expected: &str, mode: DetailOutput, partial: bool, label: &str) {
    if !partial && !expected.is_empty() && mode == DetailOutput::Json {
        serde_json::from_slice::<Value>(actual).unwrap_or_else(|error| {
            panic!(
                "{label}: invalid actual JSON {error}: {}",
                String::from_utf8_lossy(actual)
            )
        });
        serde_json::from_str::<Value>(expected)
            .unwrap_or_else(|error| panic!("{label}: invalid expected JSON {error}: {expected}"));
        assert_eq!(
            json_object_order::canonicalize(actual),
            json_object_order::canonicalize(expected.as_bytes())
        );
    } else if !partial && mode == DetailOutput::Ndjson {
        for (side, bytes) in [("actual", actual), ("expected", expected.as_bytes())] {
            for line in bytes
                .split(|byte| *byte == b'\n')
                .filter(|line| !line.is_empty())
            {
                serde_json::from_slice::<Value>(line).unwrap_or_else(|error| {
                    panic!(
                        "{label}: invalid {side} NDJSON {error}: {}",
                        String::from_utf8_lossy(line)
                    )
                });
            }
        }
        assert_eq!(
            json_object_order::canonicalize_ndjson(actual),
            json_object_order::canonicalize_ndjson(expected.as_bytes())
        );
    } else {
        assert_eq!(actual, expected.as_bytes());
    }
}
#[tokio::test]
async fn bookmark_lists_preserve_frozen_go_text_records_windows_identity_and_writers() {
    let mut cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/cli-bookmark-lists.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 1873);
    let records: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/cli-bookmark-lists-records.json"
    ))
    .unwrap();
    assert_eq!(records.len(), 1296);
    cases.extend(records);
    let mut direct_rows = 0;
    let mut saved_rows = 0;
    for case in cases {
        for saved in [false, true] {
            if saved && case.identity == 0 {
                continue;
            }
            if saved {
                saved_rows += 1;
            } else {
                direct_rows += 1;
            }
            let (mut options, proxy, no_proxy) = if case.kind == "user" {
                let args = UserArgs::try_parse_from(
                    std::iter::once("pixiv".to_owned()).chain(case.args.clone()),
                )
                .unwrap();
                (BookmarkLists::User(args.options), args.proxy, args.no_proxy)
            } else {
                let args = ListArgs::try_parse_from(
                    std::iter::once("pixiv".to_owned())
                        .chain([format!("--type={}", case.kind)])
                        .chain(case.args.clone()),
                )
                .unwrap();
                (BookmarkLists::List(args.options), args.proxy, args.no_proxy)
            };
            let input = case
                .input_bytes
                .as_deref()
                .map(decode_base64)
                .unwrap_or_else(|| case.input.as_bytes().to_vec());
            let mut reader = Reader {
                input: io::Cursor::new(input),
                failed: case.read_error,
            };
            let input_result = options.resolve_source(&mut reader, false);
            let fallback_mode = match case.mode.as_str() {
                "json" => DetailOutput::Json,
                "ndjson" | "auto" => DetailOutput::Ndjson,
                _ => DetailOutput::Human,
            };
            let mode = options
                .output_mode(false, case.mode != "auto")
                .unwrap_or(fallback_mode);
            let queries = Arc::new(Mutex::new(vec![]));
            let paths = Arc::new(Mutex::new(vec![]));
            let fixture = Fixture {
                bodies: case.bodies.clone(),
                queries: queries.clone(),
                paths: paths.clone(),
                kind: case.kind.clone(),
                identity: case.identity,
            };
            let output = Arc::new(Mutex::new(vec![]));
            let mut writer = Writer {
                out: output.clone(),
                failure: case.writer.clone(),
            };
            let validation = input_result
                .and_then(|_| options.resolve_target(&mut reader))
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
                .and_then(|_| {
                    options
                        .output_mode(false, case.mode != "auto")
                        .map(|actual| {
                            assert_eq!(actual, mode, "{} {:?} {}", case.kind, case.args, case.mode);
                        })
                });
            assert_eq!(
                reader.input.position() as usize,
                case.bytes,
                "{} {:?}",
                case.kind,
                case.args
            );
            let result = if let Err(error) = validation {
                Err(error)
            } else if saved {
                let account = saved_account::saved_execution(fixture);
                saved_bookmark_lists(
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
                bookmark_lists(&client, &options, mode, &mut writer).await
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
                    options.options().ndjson
                        || case.mode == "auto"
                            && case.kind == "user"
                            && options.options().json.is_none(),
                    options.options().ndjson || options.options().json.is_some(),
                    &mut diagnostics
                ),
                case.exit
            );
            equal(
                &output.lock().unwrap(),
                &case.stdout,
                mode,
                case.writer == "short",
                &format!(
                    "{} {:?} {} saved={saved} input={:?}",
                    case.kind, case.args, case.mode, case.input
                ),
            );
            if case.stderr.starts_with('{') {
                serde_json::from_slice::<Value>(&diagnostics).unwrap_or_else(|error| panic!(
                    "{} {:?} {} saved={saved} input={:?}: invalid actual diagnostic JSON {error}: {}",
                    case.kind, case.args, case.mode, case.input, String::from_utf8_lossy(&diagnostics)
                ));
                assert_eq!(
                    json_object_order::canonicalize(&diagnostics),
                    json_object_order::canonicalize(case.stderr.as_bytes())
                );
            } else {
                assert_eq!(diagnostics, case.stderr.as_bytes());
            }
            assert_eq!(
                *queries.lock().unwrap(),
                case.queries,
                "{} {:?}",
                case.kind,
                case.args
            );
            assert_eq!(
                *paths.lock().unwrap(),
                case.paths,
                "{} {:?}",
                case.kind,
                case.args
            );
            // Go's private callback bit is observed through replay in the pool contract, not a test-only production API.
            assert!(case.committed.len() <= 1);
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
    assert_eq!(direct_rows, 3169);
    // Credentialless clients are exercised at the direct public boundary; saved execution supplies an identity.
    assert_eq!(saved_rows, 3121);
}

struct Reader {
    input: io::Cursor<Vec<u8>>,
    failed: bool,
}
impl io::Read for Reader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if self.failed {
            Err(io::Error::other("fixture read failed"))
        } else {
            io::Read::read(&mut self.input, bytes)
        }
    }
}
fn decode_base64(value: &str) -> Vec<u8> {
    let mut result = vec![];
    let mut bits = 0_u32;
    let mut count = 0;
    for byte in value.bytes().take_while(|byte| *byte != b'=') {
        let digit = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => panic!("invalid frozen base64 input"),
        };
        bits = (bits << 6) | u32::from(digit);
        count += 6;
        if count >= 8 {
            count -= 8;
            result.push((bits >> count) as u8);
        }
    }
    result
}
