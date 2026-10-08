use crate::{IllustReference, illust_detail, illust_detail_tool};
use futures_util::{
    StreamExt,
    future::{AbortHandle, Abortable},
    stream::FuturesUnordered,
};
use pixiv_sdk::{Client, transport::Transport};
use serde_json::{Value, json};
use std::{collections::BTreeMap, future::Future, io, pin::Pin};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

type ResponseFuture<'a> = Pin<Box<dyn Future<Output = Value> + 'a>>;
type ToolFuture<'a> = Pin<Box<dyn Future<Output = crate::CallToolResult> + 'a>>;

enum ToolInput {
    Detail(IllustReference),
    Search(Box<crate::SearchIllustInput>),
}

#[derive(Default)]
struct Session {
    initialized: bool,
    log_level: String,
}

pub async fn serve<T: Transport, R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    client: &Client<T>,
    input: R,
    output: &mut W,
) -> io::Result<()> {
    let invoke = |input| -> ToolFuture<'_> {
        Box::pin(async move {
            match input {
                ToolInput::Detail(input) => illust_detail(client, input).await,
                ToolInput::Search(input) => crate::search_illust(client, *input).await,
            }
        })
    };
    serve_with(&invoke, input, output).await
}

pub async fn serve_saved<T: Transport + 'static, R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    execution: &pixiv_app::execution::Execution<T>,
    input: R,
    output: &mut W,
) -> io::Result<()> {
    serve_saved_with_proxy(execution, None, input, output).await
}

pub async fn serve_saved_with_proxy<
    T: Transport + 'static,
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
>(
    execution: &pixiv_app::execution::Execution<T>,
    proxy: Option<&str>,
    input: R,
    output: &mut W,
) -> io::Result<()> {
    let invoke = |input| -> ToolFuture<'_> {
        Box::pin(async move {
            let context = pixiv_app::lifecycle::Context::new();
            match input {
                ToolInput::Detail(input) => {
                    crate::saved_illust_detail_with_proxy(execution, &context, input, proxy).await
                }
                ToolInput::Search(input) => {
                    crate::search::saved_search_illust(execution, &context, *input, proxy).await
                }
            }
        })
    };
    serve_with(&invoke, input, output).await
}

async fn serve_with<'a, R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    invoke: &impl Fn(ToolInput) -> ToolFuture<'a>,
    input: R,
    output: &mut W,
) -> io::Result<()> {
    let mut lines = BufReader::new(input).lines();
    let mut session = Session::default();
    let mut pending: FuturesUnordered<ResponseFuture<'_>> = FuturesUnordered::new();
    let mut eof = false;
    let mut inflight = BTreeMap::new();
    loop {
        tokio::select! {
            line = lines.next_line(), if !eof => {
                let Some(line) = line? else { eof = true; continue; };
                if line.trim().is_empty() { continue; }
                let message: Value = serde_json::from_str(&line).map_err(|error| io::Error::new(io::ErrorKind::InvalidData,error))?;
                if message.get("id").is_none() {
                    if message["method"] == "notifications/cancelled"
                        && let Some(id) = message["params"].get("requestId")
                        && let Some(handle) = inflight.get(&id.to_string())
                    {
                        AbortHandle::abort(handle);
                    }
                    continue;
                }
                let id = message["id"].clone();
                let method = message["method"].as_str().unwrap_or_default();
                let params = message.get("params");
                let response = match method {
                    "initialize" => {
                        if params.is_none() || params.is_some_and(Value::is_null) {
                            write_response(output,protocol_error(id,-32600,"invalid request: missing required \"params\"".into())).await?;
                            continue;
                        }
                        session.initialized = true;
                        let version = params.and_then(|params| params["protocolVersion"].as_str()).unwrap_or_default();
                        let version = match version { "2025-06-18" | "2025-03-26" | "2024-11-05" => version, _ => "2025-06-18" };
                        success(id, json!({"protocolVersion":version,"capabilities":{"logging":{},"tools":{"listChanged":true}},"instructions":"Pixiv MCP server for searching, browsing, and downloading Pixiv content.","serverInfo":{"name":"pixiv-cli","version":"3.0.0"}}))
                    }
                    "ping" => success(id, json!({})),
                    _ if !session.initialized => protocol_error(id,0,format!("method {method:?} is invalid during session initialization")),
                    "tools/list" => success(id,json!({"tools":[illust_detail_tool(),crate::search_illust_tool()]})),
                    "logging/setLevel" => {
                        if params.is_none() || params.is_some_and(Value::is_null) {
                            protocol_error(id,-32600,"invalid request: missing required \"params\"".into())
                        } else {
                            session.log_level = params.and_then(|params|params["level"].as_str()).unwrap_or_default().into();
                            success(id,json!({}))
                        }
                    }
                    "tools/call" => {
                        if let Some(params) = params.filter(|value| !value.is_null()) {
                            let name = params["name"].as_str().unwrap_or_default();
                            if !matches!(name, "illust_detail" | "search_illust") {
                                protocol_error(id,-32602,format!("unknown tool {name:?}"))
                            } else {
                                let input = if name == "search_illust" {
                                    crate::search::decode(params.get("arguments")).map(|input| ToolInput::Search(Box::new(input)))
                                } else { decode_reference(params.get("arguments")).map(ToolInput::Detail) };
                                match input {
                                    Ok(input) => {
                                        let (handle, registration) = AbortHandle::new_pair();
                                        inflight.insert(id.to_string(), handle);
                                        pending.push(Box::pin(async move {
                                            let is_search = matches!(&input, ToolInput::Search(_));
                                            let result = match Abortable::new(invoke(input), registration).await {
                                                Ok(result) => result,
                                                Err(_) => {
                                                    let message = pixiv_sdk::Error::new(pixiv_sdk::Reason::UpstreamUnavailable,if is_search { "SearchArtworks" } else { "Artwork" }).with_detail("pixiv upstream transport failed").to_string();
                                                    if is_search { crate::search::failure(message) } else { crate::failure(message) }
                                                },
                                            };
                                            success(id,serde_json::to_value(result).expect("tool result is serializable"))
                                        }));
                                        continue;
                                    }
                                    Err(message) => protocol_error(id,-32602,message),
                                }
                            }
                        } else {
                            protocol_error(id,-32600,"invalid request: missing required \"params\"".into())
                        }
                    }
                    _ => protocol_error(id,0,format!("JSON RPC not handled: {method:?} unsupported")),
                };
                write_response(output,response).await?;
            }
            Some(response) = pending.next(), if !pending.is_empty() => {
                inflight.remove(&response["id"].to_string());
                write_response(output,response).await?;
            }
            else => break,
        }
    }
    output.flush().await
}

async fn write_response<W: AsyncWrite + Unpin>(output: &mut W, response: Value) -> io::Result<()> {
    let mut encoded = serde_json::to_vec(&response)?;
    encoded.push(b'\n');
    output.write_all(&encoded).await?;
    output.flush().await
}
fn success(id: Value, result: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":result})
}
fn protocol_error(id: Value, code: i32, message: String) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}

fn decode_reference(arguments: Option<&Value>) -> Result<IllustReference, String> {
    let arguments = arguments.unwrap_or(&Value::Null);
    if arguments.is_null() {
        return Ok(IllustReference::default());
    }
    let Some(arguments) = arguments.as_object() else {
        return Err(format!(
            "invalid params: validating \"arguments\": unmarshaling arguments: json: cannot unmarshal {} into Go value of type map[string]interface {{}}",
            value_type(arguments)
        ));
    };
    let extra = arguments
        .keys()
        .filter(|key| !matches!(key.as_str(), "illust_id" | "url"))
        .cloned()
        .collect::<Vec<_>>();
    if !extra.is_empty() {
        let extra = extra
            .iter()
            .map(|key| format!("{key:?}"))
            .collect::<Vec<_>>()
            .join(" ");
        return Err(format!(
            "invalid params: validating \"arguments\": validating root: unexpected additional properties [{extra}]"
        ));
    }
    let mut input = IllustReference::default();
    if let Some(value) = arguments.get("illust_id") {
        if value.as_f64().is_none_or(|number| number.fract() != 0.0) {
            return Err(type_error("illust_id", value, "integer"));
        }
        let decimal = value
            .as_f64()
            .expect("integer argument fits float64")
            .to_string();
        input.illust_id=decimal.parse().map_err(|_|format!("invalid params: json: cannot unmarshal number {decimal} into Go struct field illustReferenceIn.illust_id of type int64"))?;
    }
    if let Some(value) = arguments.get("url") {
        input.url = value
            .as_str()
            .ok_or_else(|| type_error("url", value, "string"))?
            .into();
    }
    Ok(input)
}
pub(crate) fn value_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
        Value::Number(number) if number.as_f64().is_some_and(|number| number.fract() == 0.0) => {
            "integer"
        }
        Value::Number(_) => "number",
    }
}
fn type_error(field: &str, value: &Value, expected: &str) -> String {
    let display = match value {
        Value::Null => "<invalid reflect.Value>".into(),
        Value::String(value) => value.clone(),
        _ => value.to_string(),
    };
    format!(
        "invalid params: validating \"arguments\": validating root: validating /properties/{field}: type: {display} has type {:?}, want {expected:?}",
        value_type(value)
    )
}
