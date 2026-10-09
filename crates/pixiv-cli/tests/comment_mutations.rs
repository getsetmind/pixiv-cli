use clap::Parser;
use pixiv_cli_rs::{
    comment_mutations::{CommentMutation, mutation, saved_mutation},
    finish_command,
};
use pixiv_sdk::{
    Client,
    transport::{JsonResponse, Request, Response, Transport},
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    io::{self, Write},
    sync::{Arc, Mutex},
};
#[allow(dead_code)]
#[path = "support/json_object_order.rs"]
mod json_object_order;
#[path = "support/saved_account.rs"]
mod saved_account;
#[derive(Parser, Debug)]
#[command(args_override_self = true)]
struct Arguments {
    #[command(subcommand)]
    action: CommentMutation,
}
type Forms = Arc<Mutex<Vec<BTreeMap<String, Vec<String>>>>>;
#[derive(Clone)]
struct Fixture {
    bodies: Vec<Value>,
    forms: Forms,
    paths: Arc<Mutex<Vec<String>>>,
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
        assert_eq!(request.method.as_str(), "POST");
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
            "/v1/illust/comment/add"
                | "/v1/illust/comment/delete"
                | "/v1/novel/comment/add"
                | "/v1/novel/comment/delete"
        ));
        self.paths.lock().unwrap().push(path.to_owned());
        let mut query = BTreeMap::<String, Vec<String>>::new();
        for (k, v) in request.parameters {
            query.entry(k).or_default().push(v);
        }
        let index = 0;
        self.forms.lock().unwrap().push(query);
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
        if self.failure == "short_success" {
            let count = bytes.len().min(10);
            out.extend_from_slice(&bytes[..count]);
            return Ok(count);
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

#[tokio::test]
async fn comment_mutations_preserve_frozen_go_outputs_inputs_wire_and_saved_accounts() {
    let cases: Vec<Value> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/cli-comment-mutations.json"
    ))
    .unwrap();
    for case in cases {
        let operation = case["operation"].as_str().unwrap();
        let args = std::iter::once("pixiv".to_owned())
            .chain(std::iter::once(operation.to_owned()))
            .chain(
                (case["default_type"] != true)
                    .then(|| format!("--type={}", case["kind"].as_str().unwrap())),
            )
            .chain(
                case["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap().to_owned()),
            )
            .collect::<Vec<_>>();
        if case["error"].as_str().unwrap().starts_with("unknown flag:") {
            let error = Arguments::try_parse_from(args).unwrap_err();
            assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
            let flag = case["error"]
                .as_str()
                .unwrap()
                .strip_prefix("unknown flag: ")
                .unwrap();
            assert!(error.to_string().contains(flag));
            assert_eq!(case["stdout"], "");
            assert!(
                case["paths"].as_array().unwrap().is_empty()
                    && case["forms"].as_array().unwrap().is_empty()
                    && case["committed"].as_array().unwrap().is_empty()
            );
            assert_eq!(case["bytes"], 0);
            // Go's isolated Cobra renderer differs from executable startup; exact startup diagnostics are compared in the process suite.
            continue;
        }
        for saved in [false, true] {
            let mut action = Arguments::try_parse_from(args.clone()).unwrap().action;
            let mut reader = Reader {
                input: io::Cursor::new(case["input"].as_str().unwrap().as_bytes().to_vec()),
                failed: case["read_error"] == true,
            };
            let resolved = action.resolve_source(&mut reader, false);
            let forms = Arc::new(Mutex::new(vec![]));
            let paths = Arc::new(Mutex::new(vec![]));
            let fixture = Fixture {
                bodies: serde_json::from_value(case["bodies"].clone()).unwrap(),
                forms: forms.clone(),
                paths: paths.clone(),
                status: case["status"].as_u64().unwrap_or(200) as u16,
                wire_body: None,
                identity: 42,
            };
            let output = Arc::new(Mutex::new(vec![]));
            let mut writer = Writer {
                out: output.clone(),
                failure: case["writer"].as_str().unwrap().to_owned(),
            };
            let json = matches!(case["mode"].as_str().unwrap(), "json" | "config_json");
            let result = if let Err(error) = resolved {
                Err(error)
            } else if saved {
                let account = saved_account::saved_execution(fixture);
                saved_mutation(
                    &account.execution,
                    &pixiv_app::lifecycle::Context::new(),
                    action.clone(),
                    json,
                    &mut writer,
                )
                .await
            } else {
                mutation(
                    &Client::with_transport("fixture-access", fixture),
                    &action,
                    json,
                    &mut writer,
                )
                .await
            };
            let label = format!("{operation} {:?} saved={saved}", case["args"]);
            assert_eq!(
                result
                    .as_ref()
                    .err()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
                case["error"].as_str().unwrap(),
                "{label}"
            );
            let mut diagnostics = vec![];
            assert_eq!(
                finish_command(
                    result,
                    false,
                    action.input().json.is_some(),
                    &mut diagnostics
                ),
                case["exit"].as_i64().unwrap() as i32,
                "{label}"
            );
            assert_eq!(
                *output.lock().unwrap(),
                case["stdout"].as_str().unwrap().as_bytes(),
                "{label}"
            );
            let stderr = case["stderr"].as_str().unwrap();
            if stderr.starts_with('{') {
                assert_eq!(
                    json_object_order::canonicalize(&diagnostics),
                    json_object_order::canonicalize(stderr.as_bytes()),
                    "{label}"
                );
            } else {
                assert_eq!(diagnostics, stderr.as_bytes(), "{label}");
            }
            assert_eq!(
                serde_json::to_value(&*forms.lock().unwrap()).unwrap(),
                case["forms"],
                "{label}"
            );
            assert_eq!(
                serde_json::to_value(&*paths.lock().unwrap()).unwrap(),
                case["paths"],
                "{label}"
            );
            assert_eq!(
                reader.input.position(),
                case["bytes"].as_u64().unwrap(),
                "{label}"
            );
            if !paths.lock().unwrap().is_empty() {
                assert_eq!(
                    action.proxy_override().unwrap(),
                    case["proxy"].as_str(),
                    "{label}"
                );
                assert_eq!(action.input().json, case["json"].as_bool(), "{label}");
            }
        }
    }
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
