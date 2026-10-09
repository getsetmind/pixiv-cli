use clap::Parser;
use pixiv_cli_rs::{
    DetailOutput,
    bookmark_reads::{
        BookmarkDetailOptions, BookmarkTagsOptions, bookmark_detail, bookmark_tags,
        saved_bookmark_detail, saved_bookmark_tags,
    },
    finish_command,
};
use pixiv_sdk::{
    Client,
    transport::{JsonResponse, Request, Response, Transport},
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
struct TagsArgs {
    #[command(flatten)]
    options: BookmarkTagsOptions,
    #[arg(long)]
    proxy: Option<String>,
    #[arg(long,num_args=0..=1,require_equals=true,default_missing_value="true")]
    no_proxy: Option<bool>,
}
#[derive(Parser)]
#[command(args_override_self = true)]
struct DetailArgs {
    #[command(flatten)]
    options: BookmarkDetailOptions,
    #[arg(long)]
    proxy: Option<String>,
    #[arg(long,num_args=0..=1,require_equals=true,default_missing_value="true")]
    no_proxy: Option<bool>,
}
#[derive(Deserialize)]
struct Case {
    operation: String,
    kind: String,
    #[serde(default)]
    default_type: bool,
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
    #[serde(default)]
    status: Option<u16>,
    #[serde(default)]
    wire_body: Option<String>,
}
#[derive(Clone)]
enum Options {
    Detail(BookmarkDetailOptions),
    Tags(BookmarkTagsOptions),
}
impl Options {
    fn resolve_source<R: io::Read>(
        &mut self,
        input: &mut R,
        terminal: bool,
    ) -> Result<(), pixiv_cli_rs::CommandError> {
        match self {
            Self::Detail(value) => value.resolve_source(input, terminal),
            Self::Tags(value) => value.resolve_source(input, terminal),
        }
    }
    fn resolve_target<R: io::Read>(
        &mut self,
        input: &mut R,
    ) -> Result<(), pixiv_cli_rs::CommandError> {
        match self {
            Self::Detail(value) => value.resolve_target(input),
            Self::Tags(value) => value.resolve_target(input),
        }
    }
    fn validate(&self) -> Result<(), pixiv_cli_rs::CommandError> {
        match self {
            Self::Detail(value) => value.validate(),
            Self::Tags(value) => value.validate(),
        }
    }
    fn output_mode(
        &self,
        configured_json: bool,
        terminal: bool,
    ) -> Result<DetailOutput, pixiv_cli_rs::CommandError> {
        match self {
            Self::Detail(value) => value.output_mode(configured_json, terminal),
            Self::Tags(value) => value.output_mode(configured_json, terminal),
        }
    }
    // The frozen Go callback returns its configured result directly; real config precedence is checked at process startup.
    fn captured_output_mode(
        &self,
        configured_json: bool,
        terminal: bool,
    ) -> Result<DetailOutput, pixiv_cli_rs::CommandError> {
        let mut effective = self.clone();
        match &mut effective {
            Self::Detail(value) => {
                if value.json.is_some() {
                    value.json = Some(configured_json);
                }
            }
            Self::Tags(value) => {
                if value.listing.json.is_some() {
                    value.listing.json = Some(configured_json);
                }
            }
        }
        effective.output_mode(configured_json, terminal)
    }
    fn json(&self) -> Option<bool> {
        match self {
            Self::Detail(value) => value.json,
            Self::Tags(value) => value.listing.json,
        }
    }
    fn ndjson(&self) -> bool {
        match self {
            Self::Detail(_) => false,
            Self::Tags(value) => value.listing.ndjson,
        }
    }
}
type Queries = Arc<Mutex<Vec<BTreeMap<String, Vec<String>>>>>;
#[derive(Clone)]
struct Fixture {
    bodies: Vec<Value>,
    queries: Queries,
    paths: Arc<Mutex<Vec<String>>>,
    kind: String,
    operation: String,
    status: u16,
    wire_body: Option<String>,
    identity: i64,
}
impl Transport for Fixture {
    async fn send_json(&self, request: Request) -> pixiv_sdk::Result<JsonResponse> {
        let oauth = request.operation == "Open";
        let mut decoded = self.clone();
        decoded.wire_body = None;
        let response = decoded.send(request).await?;
        Ok(JsonResponse {
            status: response.status,
            retry_after: response.retry_after,
            body: if !oauth {
                self.wire_body
                    .as_ref()
                    .map(|body| body.as_bytes().to_vec())
                    .unwrap_or_else(|| serde_json::to_vec(&response.body).unwrap())
            } else {
                serde_json::to_vec(&response.body).unwrap()
            },
        })
    }
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
                && (value == "Bearer fixture-access"
                    || value == &format!("Bearer fixture-access-{}", self.identity))
        ));
        let path = request
            .url
            .strip_prefix("https://app-api.pixiv.net")
            .unwrap();
        assert!(matches!(
            path,
            "/v1/user/bookmark-tags/illust"
                | "/v1/user/bookmark-tags/novel"
                | "/v2/illust/bookmark/detail"
                | "/v2/novel/bookmark/detail"
        ));
        self.paths.lock().unwrap().push(path.to_owned());
        let mut query = BTreeMap::<String, Vec<String>>::new();
        for (k, v) in request.parameters {
            query.entry(k).or_default().push(v);
        }
        let index = if self.operation == "detail" {
            0
        } else {
            usize::from(self.kind == "all" && path.ends_with("novel")) * 2
                + usize::from(query.contains_key("offset"))
        };
        self.queries.lock().unwrap().push(query);
        Ok(Response {
            status: self.status,
            retry_after: None,
            body: match &self.wire_body {
                Some(body) if (200..300).contains(&self.status) => {
                    serde_json::from_slice(body.as_bytes()).map_err(|_| {
                        pixiv_sdk::Error::new(
                            pixiv_sdk::Reason::MalformedUpstreamResponse,
                            request.operation,
                        )
                    })?
                }
                Some(_) => Value::Null,
                None => self.bodies[index].clone(),
            },
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
        if self.failure == "broken" {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "broken pipe"));
        }
        if self.failure == "other" {
            return Err(io::Error::other("fixture write failed"));
        }
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
            json_object_order::canonicalize(expected.as_bytes()),
            "{label}"
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
            json_object_order::canonicalize_ndjson(expected.as_bytes()),
            "{label}"
        );
    } else {
        assert_eq!(actual, expected.as_bytes(), "{label}");
    }
}
#[tokio::test]
async fn bookmark_reads_compare_go_operations_and_reject_all_unsupported_leaf_flag_rows() {
    let mut cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/cli-bookmark-reads.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 1600);
    let records: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/cli-bookmark-reads-records.json"
    ))
    .unwrap();
    assert_eq!(records.len(), 7315);
    cases.extend(records);
    let bodies: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/cli-bookmark-reads-bodies.json"
    ))
    .unwrap();
    assert_eq!(bodies.len(), 582);
    cases.extend(bodies);
    let mut direct_rows = 0;
    let mut saved_rows = 0;
    let mut parse_rows = 0;
    for case in cases {
        if case.error.starts_with("unknown flag:") {
            let args = std::iter::once("pixiv".to_owned())
                .chain((!case.default_type).then(|| format!("--type={}", case.kind)))
                .chain(case.args.clone());
            let error = if case.operation == "detail" {
                match DetailArgs::try_parse_from(args) {
                    Err(error) => error,
                    Ok(_) => panic!("unsupported frozen detail flags accepted: {:?}", case.args),
                }
            } else {
                assert_eq!(case.operation, "tags");
                match TagsArgs::try_parse_from(args) {
                    Err(error) => error,
                    Ok(_) => panic!("unsupported frozen tags flags accepted: {:?}", case.args),
                }
            };
            assert_eq!(
                error.kind(),
                clap::error::ErrorKind::UnknownArgument,
                "{} {:?}",
                case.operation,
                case.args
            );
            let flag = case.error.strip_prefix("unknown flag: ").unwrap();
            assert!(
                error.to_string().contains(flag),
                "{} {:?}: {error}",
                case.operation,
                case.args
            );
            // Rust exposes the leaf option parser, but no equivalent of Go's Cobra leaf renderer; exact diagnostic/exit remain unverified.
            assert_eq!(case.stdout, "");
            assert!(case.paths.is_empty() && case.queries.is_empty() && case.committed.is_empty());
            assert_eq!(case.bytes, 0);
            parse_rows += 1;
        } else {
            for saved in std::iter::once(false).chain((case.identity > 0).then_some(true)) {
                if saved {
                    saved_rows += 1;
                } else {
                    direct_rows += 1;
                }
                let label = format!(
                    "{} {} identity={} saved={saved} {:?} input={:?} writer={}",
                    case.operation, case.mode, case.identity, case.args, case.input, case.writer
                );
                let (mut options, proxy, no_proxy) = if case.operation == "detail" {
                    let args = DetailArgs::try_parse_from(
                        std::iter::once("pixiv".to_owned())
                            .chain((!case.default_type).then(|| format!("--type={}", case.kind)))
                            .chain(case.args.clone()),
                    )
                    .unwrap();
                    (Options::Detail(args.options), args.proxy, args.no_proxy)
                } else {
                    assert_eq!(case.operation, "tags");
                    let args = TagsArgs::try_parse_from(
                        std::iter::once("pixiv".to_owned())
                            .chain((!case.default_type).then(|| format!("--type={}", case.kind)))
                            .chain(case.args.clone()),
                    )
                    .unwrap();
                    (Options::Tags(args.options), args.proxy, args.no_proxy)
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
                    .captured_output_mode(
                        matches!(case.mode.as_str(), "json" | "config_json"),
                        case.mode != "auto",
                    )
                    .unwrap_or(fallback_mode);
                let queries = Arc::new(Mutex::new(vec![]));
                let paths = Arc::new(Mutex::new(vec![]));
                let fixture = Fixture {
                    bodies: case.bodies.clone(),
                    queries: queries.clone(),
                    paths: paths.clone(),
                    kind: case.kind.clone(),
                    operation: case.operation.clone(),
                    status: case.status.unwrap_or(200),
                    wire_body: case.wire_body.clone(),
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
                            .captured_output_mode(
                                matches!(case.mode.as_str(), "json" | "config_json"),
                                case.mode != "auto",
                            )
                            .map(|actual| {
                                assert_eq!(
                                    actual, mode,
                                    "{} {:?} {}",
                                    case.kind, case.args, case.mode
                                );
                            })
                    });
                assert_eq!(reader.input.position() as usize, case.bytes, "{label}");
                let result = if let Err(error) = validation {
                    Err(error)
                } else if saved {
                    let account = if case.identity == 42 {
                        saved_account::saved_execution(fixture)
                    } else {
                        saved_account::saved_execution_for_user(fixture, case.identity)
                    };
                    match options.clone() {
                        Options::Detail(value) => {
                            saved_bookmark_detail(
                                &account.execution,
                                &pixiv_app::lifecycle::Context::new(),
                                value,
                                None,
                                mode,
                                &mut writer,
                            )
                            .await
                        }
                        Options::Tags(value) => {
                            saved_bookmark_tags(
                                &account.execution,
                                &pixiv_app::lifecycle::Context::new(),
                                value,
                                None,
                                mode,
                                writer,
                            )
                            .await
                        }
                    }
                } else {
                    let client = if case.identity > 0 {
                        let credentials = pixiv_sdk::oauth::refresh(&fixture, "fixture-refresh")
                            .await
                            .unwrap();
                        Client::from_credentials(&credentials, fixture)
                    } else {
                        Client::with_transport("fixture-access", fixture)
                    };
                    match &options {
                        Options::Detail(value) => {
                            bookmark_detail(&client, value, mode, &mut writer).await
                        }
                        Options::Tags(value) => {
                            bookmark_tags(&client, value, mode, &mut writer).await
                        }
                    }
                };
                assert_eq!(
                    result
                        .as_ref()
                        .err()
                        .map(ToString::to_string)
                        .unwrap_or_default(),
                    case.error,
                    "{label}"
                );
                let mut diagnostics = vec![];
                assert_eq!(
                    finish_command(
                        result,
                        options.ndjson(),
                        options.ndjson() || options.json().is_some(),
                        &mut diagnostics
                    ),
                    case.exit,
                    "{label}"
                );
                equal(
                    &output.lock().unwrap(),
                    &case.stdout,
                    mode,
                    case.writer == "short",
                    &label,
                );
                if case.stderr.starts_with('{') {
                    serde_json::from_slice::<Value>(&diagnostics).unwrap_or_else(|error| panic!(
                    "{} {:?} {} saved={saved} input={:?}: invalid actual diagnostic JSON {error}: {}",
                    case.kind, case.args, case.mode, case.input, String::from_utf8_lossy(&diagnostics)
                ));
                    assert_eq!(
                        json_object_order::canonicalize(&diagnostics),
                        json_object_order::canonicalize(case.stderr.as_bytes()),
                        "{label}"
                    );
                } else {
                    assert_eq!(diagnostics, case.stderr.as_bytes(), "{label}");
                }
                assert_eq!(*queries.lock().unwrap(), case.queries, "{label}");
                assert_eq!(*paths.lock().unwrap(), case.paths, "{label}");
                // Go's private callback bit is observed through replay in the pool contract, not a test-only production API.
                assert!(case.committed.len() <= 1);
                if !case.queries.is_empty() {
                    assert_eq!(
                        case.proxy,
                        if no_proxy == Some(true) {
                            Some(String::new())
                        } else {
                            proxy
                        },
                        "{label}"
                    );
                    if !options.ndjson() {
                        assert_eq!(options.json(), case.json, "{label}");
                    }
                }
            }
        }
    }
    assert_eq!(parse_rows, 869);
    assert_eq!(direct_rows, 8628);
    assert_eq!(direct_rows + parse_rows, 1600 + 7315 + 582);
    // Credentialless clients are exercised at the direct public boundary; saved execution supplies an identity.
    assert_eq!(saved_rows, 8592);
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
