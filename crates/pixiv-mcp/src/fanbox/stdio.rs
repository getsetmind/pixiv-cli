use super::Server;
use futures_util::{StreamExt, stream::FuturesUnordered};
use pixiv_app::lifecycle::Context;
use serde_json::{Value, json};
use std::{collections::BTreeMap, future::Future, io, pin::Pin};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, BufReader};

type Pending<'a> = Pin<Box<dyn Future<Output = (Value, Value)> + Send + 'a>>;

pub async fn serve<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    server: &Server,
    context: Option<&Context>,
    input: R,
    output: &mut W,
) -> io::Result<()> {
    let background = Context::background();
    let root = context.unwrap_or(&background);
    let mut lines = BufReader::new(input).lines();
    let mut pending: FuturesUnordered<Pending<'_>> = FuturesUnordered::new();
    let mut inflight = BTreeMap::<String, Context>::new();
    let mut initialized = false;
    let mut eof = false;
    let mut closing = None;
    let mut failure = None;
    macro_rules! respond {
        ($response:expr) => {
            if let Err(error) = crate::stdio::write_response(output, $response).await {
                failure = Some(error);
                eof = true;
                if root.error().is_none() {
                    for request in inflight.values() {
                        request.cancel();
                    }
                }
            }
        };
    }
    loop {
        if pending.is_empty() {
            if let Some(error) = failure.take() {
                return Err(error);
            }
            if let Some(error) = closing.or_else(|| root.error()) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, error));
            }
            if eof {
                return Ok(());
            }
        }
        tokio::select! {
            error=root.cancelled(), if closing.is_none() && context.is_some()=>{closing=Some(error);}
            line=lines.next_line(), if !eof=>{
                let line=match line {Ok(line)=>line,Err(error)=>{failure=Some(error);eof=true;continue;}};
                let Some(line)=line else{eof=true;continue;};
                if line.trim().is_empty(){continue;}
                let message:Value=match serde_json::from_str(&line) {Ok(message)=>message,Err(error)=>{failure=Some(io::Error::new(io::ErrorKind::InvalidData,error));eof=true;continue;}};
                if message.get("id").is_none(){
                    if message["method"]=="notifications/cancelled" && let Some(request)=inflight.get(&message["params"]["requestId"].to_string()){request.cancel();}
                    continue;
                }
                if closing.is_some(){continue;}
                let id=message["id"].clone();
                let method=message["method"].as_str().unwrap_or_default();
                let params=message.get("params").filter(|params|!params.is_null());
                let response=match method {
                    "initialize"=>{
                        let Some(params)=params else{respond!(crate::stdio::protocol_error(id,-32600,"invalid request: missing required \"params\"".into()));continue;};
                        initialized=true;
                        crate::stdio::success(id,super::initialize(params["protocolVersion"].as_str().unwrap_or_default()))
                    }
                    "ping"=>crate::stdio::success(id,json!({})),
                    _ if !initialized=>crate::stdio::protocol_error(id,0,format!("method {method:?} is invalid during session initialization")),
                    "tools/list"=>crate::stdio::success(id,super::tools()),
                    "logging/setLevel"=>{
                        if params.is_none(){crate::stdio::protocol_error(id,-32600,"invalid request: missing required \"params\"".into())}else{crate::stdio::success(id,json!({}))}
                    }
                    "tools/call"=>{
                        let Some(params)=params else{respond!(crate::stdio::protocol_error(id,-32600,"invalid request: missing required \"params\"".into()));continue;};
                        let name=params["name"].as_str().unwrap_or_default().to_owned();
                        let args=params.get("arguments").cloned().unwrap_or(Value::Null);
                        let request=root.child();
                        inflight.insert(id.to_string(),request.clone());
                        pending.push(Box::pin(async move {
                            let result=server.call(&request,&name,&args).await;
                            request.cancel();
                            let response=match result {Ok(result)=>crate::stdio::success(id.clone(),result),Err(error)=>crate::stdio::protocol_error(id.clone(),-32602,error)};
                            (id,response)
                        }));
                        continue;
                    }
                    _=>crate::stdio::protocol_error(id,-32601,format!("method not found: {method:?}")),
                };
                respond!(response);
            }
            response=pending.next(), if !pending.is_empty()=>{
                if let Some((id,response))=response{
                    inflight.remove(&id.to_string());
                    if !eof&&closing.is_none(){respond!(response);}
                }
            }
        }
    }
}
