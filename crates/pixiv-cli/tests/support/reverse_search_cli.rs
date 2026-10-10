use pixiv_app::{
    config,
    reverse_search::{
        ASCII2DClient, ASCII2DSession, Aggregator, AggregatorDependencies, CallerContext,
        Dependencies, Error, ErrorCode, Facade, Loader, Provider, ProviderClient, ProviderResponse,
        Request, ReverseFuture, SearchOutcome, Searcher, Snapshot, SourceLoader,
        SourceLoaderOptions,
    },
};
use pixiv_cli_rs::{CommandError, DetailOutput, reverse_search};
use pixiv_sdk::{
    context::{Context, ContextError},
    fanbox::transport::{
        BodyFuture, ExternalError, RawBody, RawRead, RawRequest, RawResponse, RawTransport,
        TransportFuture,
    },
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{self, Read, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
};

pub const PAYLOAD: &[u8] = b"owned synthetic reverse-search payload\n";

#[derive(Deserialize)]
pub struct Fixture {
    pub reference: String,
    pub cases: Vec<Row>,
}
#[derive(Deserialize)]
pub struct Row {
    pub name: String,
    pub input: Value,
    pub observation: Value,
}
pub fn fixture() -> Fixture {
    serde_json::from_str(include_str!("../fixtures/reverse-search-cli.json")).unwrap()
}
pub fn string<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or("")
}
pub fn external(message: &str) -> Error {
    Error::external(io::Error::other(message.to_owned()))
}

pub struct Arguments {
    pub source: String,
    pub provider: String,
    pub changed: Vec<String>,
    pub json: Option<bool>,
    pub ndjson: bool,
}
impl Arguments {
    pub fn parse(input: &Value) -> Self {
        let args = input["args"].as_array().unwrap();
        let mut result = Self {
            source: String::new(),
            provider: String::new(),
            changed: Vec::new(),
            json: None,
            ndjson: false,
        };
        let mut words = Vec::new();
        let mut index = 1;
        while index < args.len() {
            let argument = args[index].as_str().unwrap();
            if let Some(flag) = argument.strip_prefix("--") {
                let (name, value) = flag.split_once('=').unwrap_or((flag, "true"));
                result.changed.push(name.to_owned());
                match name {
                    "json" => result.json = Some(value == "true"),
                    "ndjson" => result.ndjson = value == "true",
                    "provider" => {
                        if flag.contains('=') {
                            result.provider = value.to_owned();
                        } else {
                            index += 1;
                            result.provider = args[index].as_str().unwrap().to_owned();
                        }
                    }
                    _ => {}
                }
            } else {
                words.push(argument);
            }
            index += 1;
        }
        result.source = if words.is_empty() {
            string(input, "stdin").trim_end_matches('\n').to_owned()
        } else {
            words.join(" ")
        };
        result
    }
    pub fn mode(&self, runtime: &config::RuntimeConfig, input: &Value) -> DetailOutput {
        if self.ndjson {
            DetailOutput::Ndjson
        } else if self.json.unwrap_or(runtime.output_json) {
            DetailOutput::Json
        } else if self.json.is_none() && input["output_pipe"].as_bool().unwrap_or(false) {
            DetailOutput::Ndjson
        } else {
            DetailOutput::Human
        }
    }
    pub fn input(&self) -> reverse_search::Input {
        reverse_search::Input {
            source: self.source.clone(),
            provider: self.provider.clone(),
            changed_flags: self.changed.clone(),
            ndjson: self.ndjson,
            json_changed: self.json.is_some(),
        }
    }
}

pub struct Writer {
    pub bytes: Vec<u8>,
    pub writes: Vec<usize>,
    mode: String,
    remaining: usize,
}
impl Writer {
    pub fn new(mode: &str, remaining: usize) -> Self {
        Self {
            bytes: Vec::new(),
            writes: Vec::new(),
            mode: mode.into(),
            remaining,
        }
    }
    pub fn text(&self) -> &str {
        std::str::from_utf8(&self.bytes).unwrap()
    }
}
impl Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.writes.push(bytes.len());
        if self.mode.is_empty() {
            self.bytes.extend_from_slice(bytes);
            return Ok(bytes.len());
        }
        let count = self.remaining.min(bytes.len());
        self.bytes.extend_from_slice(&bytes[..count]);
        self.remaining -= count;
        if count == bytes.len() {
            return Ok(count);
        }
        match self.mode.as_str() {
            "short" => Ok(count),
            "pipe" => Err(io::Error::new(io::ErrorKind::BrokenPipe, "broken pipe")),
            "short-error" => Err(io::Error::new(io::ErrorKind::WriteZero, "short write")),
            _ => Err(io::Error::other("owned writer failure")),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub struct State {
    pub input: Value,
    pub context: Arc<Context>,
    pub observed: Mutex<Value>,
    pub snapshot: Mutex<Option<Arc<Snapshot>>>,
}
impl State {
    pub fn new(input: &Value) -> Arc<Self> {
        let context = Arc::new(Context::new());
        if string(input, "cancel") == "before" {
            context.cancel();
        }
        let providers = ["ascii2d", "ascii2d-bovw", "ascii2d-color", "saucenao"]
            .into_iter()
            .map(|key| {
                (
                    key.to_owned(),
                    json!({"preflights":0,"searches":0,"uploads":0,"payloads":[],"closes":0}),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        Arc::new(Self {
            input: input.clone(),
            context,
            observed: Mutex::new(
                json!({"requests":[],"search_responses":[],"search_errors":[],"source_requests":[],"source_body_reads":0,"source_body_closes":0,"providers":providers,"close_order":[],"searcher_closes":0}),
            ),
            snapshot: Mutex::new(None),
        })
    }
    pub fn count(&self, provider: &str, field: &str) {
        let mut observed = self.observed.lock().unwrap();
        let value = &mut observed["providers"][provider][field];
        *value = json!(value.as_u64().unwrap() + 1);
    }
    pub fn failure(&self, key: &str) -> Result<(), Error> {
        match string(&self.input["failures"], key) {
            "" => Ok(()),
            "classified" => Err(Error::new(
                ErrorCode::SolverUnavailable,
                "ascii2d challenge solver is unavailable",
                Some(external(
                    "synthetic-api-key-secret synthetic-upstream-body-secret synthetic-csrf-secret synthetic-location-secret",
                )),
            )),
            "missing-key" => Err(Error::new(
                ErrorCode::MissingCredential,
                "SauceNAO API key is required",
                Some(external("synthetic-api-key-secret")),
            )),
            "cancel" => {
                self.context.cancel();
                Err(ContextError::Canceled.into())
            }
            _ => Err(external(
                "synthetic-upstream-body-secret synthetic-csrf-secret synthetic-location-secret",
            )),
        }
    }
    fn payload(&self, provider: &str, snapshot: Arc<Snapshot>) {
        let mut reads = Vec::new();
        for _ in 0..2 {
            let mut bytes = Vec::new();
            snapshot.open().unwrap().read_to_end(&mut bytes).unwrap();
            reads.push(
                bytes
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>(),
            );
        }
        let observation = json!({"kind":snapshot.kind(),"sha256":snapshot.sha256(),"size":snapshot.size(),"reads_hex":reads});
        self.observed.lock().unwrap()["providers"][provider]["payloads"]
            .as_array_mut()
            .unwrap()
            .push(observation);
        *self.snapshot.lock().unwrap() = Some(snapshot);
    }
    fn reply(&self, provider: &Provider) -> ProviderResponse {
        let value = &self.input["provider_replies"][provider.as_str()];
        ProviderResponse {
            provider: serde_json::from_value(value["provider"].clone()).unwrap(),
            matches: if value["matches"].is_null() {
                Vec::new()
            } else {
                serde_json::from_value(value["matches"].clone()).unwrap()
            },
            quota: serde_json::from_value(value["quota"].clone()).unwrap(),
        }
    }
    fn close_provider(&self, provider: &str) -> Result<(), Error> {
        self.count(provider, "closes");
        self.observed.lock().unwrap()["close_order"]
            .as_array_mut()
            .unwrap()
            .push(json!(provider));
        if !string(&self.input["failures"], &format!("{provider}-close")).is_empty() {
            Err(external(if provider == "saucenao" {
                "owned SauceNAO close failure"
            } else {
                "owned ascii2d close failure"
            }))
        } else {
            Ok(())
        }
    }
}
struct Sauce(Arc<State>);
impl ProviderClient for Sauce {
    fn preflight(&self, _: CallerContext) -> ReverseFuture<'_, Result<(), Error>> {
        Box::pin(async {
            self.0.count("saucenao", "preflights");
            self.0.failure("saucenao-preflight")
        })
    }
    fn search(
        &self,
        _: CallerContext,
        snapshot: Arc<Snapshot>,
    ) -> ReverseFuture<'_, Result<ProviderResponse, Error>> {
        Box::pin(async move {
            self.0.count("saucenao", "searches");
            self.0.payload("saucenao", snapshot);
            self.0.failure("saucenao-search")?;
            Ok(self.0.reply(&Provider::SauceNao))
        })
    }
    fn close(&self) -> ReverseFuture<'_, Result<(), Error>> {
        Box::pin(async { self.0.close_provider("saucenao") })
    }
}
struct Ascii(Arc<State>);
impl ASCII2DClient for Ascii {
    fn preflight(&self, _: CallerContext) -> ReverseFuture<'_, Result<(), Error>> {
        Box::pin(async {
            self.0.count("ascii2d", "preflights");
            self.0.failure("ascii2d-preflight")
        })
    }
    fn upload(
        &self,
        _: CallerContext,
        snapshot: Arc<Snapshot>,
    ) -> ReverseFuture<'_, Result<Arc<dyn ASCII2DSession>, Error>> {
        Box::pin(async move {
            self.0.count("ascii2d", "uploads");
            self.0.payload("ascii2d", snapshot);
            self.0.failure("ascii2d-upload")?;
            Ok(Arc::new(Ascii(self.0.clone())) as Arc<dyn ASCII2DSession>)
        })
    }
    fn close(&self) -> ReverseFuture<'_, Result<(), Error>> {
        Box::pin(async { self.0.close_provider("ascii2d") })
    }
}
impl ASCII2DSession for Ascii {
    fn search(
        &self,
        _: CallerContext,
        provider: Provider,
    ) -> ReverseFuture<'_, Result<ProviderResponse, Error>> {
        Box::pin(async move {
            self.0.count(provider.as_str(), "searches");
            self.0.failure(&format!("{}-search", provider.as_str()))?;
            Ok(self.0.reply(&provider))
        })
    }
}
struct SourceTransport(Arc<State>);
impl RawTransport for SourceTransport {
    fn send(
        &self,
        request: RawRequest,
    ) -> TransportFuture<'_, Result<Option<RawResponse>, ExternalError>> {
        Box::pin(async move {
            assert_eq!(request.method, "GET");
            assert_eq!(
                url::Url::parse(&request.url).unwrap().host_str(),
                Some("owned-source.invalid")
            );
            assert!(request.body.is_none());
            self.0.observed.lock().unwrap()["source_requests"]
                .as_array_mut()
                .unwrap()
                .push(json!({"method":request.method,"url":request.url,"headers":request.headers}));
            if !string(&self.0.input["failures"], "source-transport").is_empty() {
                return Err(Box::new(io::Error::other(
                    "synthetic-source-secret synthetic-upstream-body-secret",
                )) as ExternalError);
            }
            Ok(Some(RawResponse {
                status: self.0.input["source_status"]
                    .as_u64()
                    .filter(|value| *value > 0)
                    .unwrap_or(200) as u16,
                headers: BTreeMap::from([("Content-Type".into(), vec!["image/png".into()])]),
                content_length: PAYLOAD.len() as i64,
                body: Some(Box::new(SourceBody {
                    state: self.0.clone(),
                    position: 0,
                })),
            }))
        })
    }
}
struct SourceBody {
    state: Arc<State>,
    position: usize,
}
impl RawBody for SourceBody {
    fn read<'a>(&'a mut self, output: &'a mut [u8]) -> BodyFuture<'a, RawRead> {
        Box::pin(async move {
            {
                let mut observed = self.state.observed.lock().unwrap();
                let value = &mut observed["source_body_reads"];
                *value = json!(value.as_u64().unwrap() + 1);
            }
            if !string(&self.state.input["failures"], "source-read").is_empty() {
                return RawRead {
                    count: 0,
                    eof: false,
                    error: Some(Box::new(io::Error::other("synthetic-upstream-body-secret"))),
                };
            }
            let count = output.len().min(PAYLOAD.len() - self.position);
            output[..count].copy_from_slice(&PAYLOAD[self.position..self.position + count]);
            self.position += count;
            if string(&self.state.input, "cancel") == "source" {
                self.state.context.cancel();
            }
            RawRead {
                count,
                eof: count == 0,
                error: None,
            }
        })
    }
    fn close(&mut self) -> BodyFuture<'_, Result<(), ExternalError>> {
        Box::pin(async {
            let mut observed = self.state.observed.lock().unwrap();
            let value = &mut observed["source_body_closes"];
            *value = json!(value.as_u64().unwrap() + 1);
            Ok(())
        })
    }
}
struct OwnedLoader {
    loader: Loader,
    root: PathBuf,
}
impl SourceLoader for OwnedLoader {
    fn load<'a>(
        &'a self,
        context: CallerContext,
        source: &'a str,
    ) -> ReverseFuture<'a, Result<Arc<Snapshot>, Error>> {
        Box::pin(async move {
            let source = if source.to_ascii_lowercase().starts_with("http:")
                || source.to_ascii_lowercase().starts_with("https:")
            {
                source.to_owned()
            } else {
                self.root.join(source).to_str().unwrap().to_owned()
            };
            self.loader.load(context, &source).await
        })
    }
}
pub struct ObservedSearcher {
    facade: Facade,
    state: Arc<State>,
}
impl Searcher for ObservedSearcher {
    fn search(&self, context: CallerContext, request: Request) -> ReverseFuture<'_, SearchOutcome> {
        Box::pin(async move {
            self.state.observed.lock().unwrap()["requests"].as_array_mut().unwrap().push(json!({"Source":request.source,"Provider":request.provider,"PixivOnly":request.pixiv_only}));
            let outcome = self.facade.search(context, request).await;
            let mut observed = self.state.observed.lock().unwrap();
            observed["search_responses"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::to_value(&outcome.response).unwrap());
            observed["search_errors"]
                .as_array_mut()
                .unwrap()
                .push(json!(
                    outcome
                        .error
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_default()
                ));
            outcome
        })
    }
    fn close(&self) -> ReverseFuture<'_, Result<(), Error>> {
        Box::pin(async {
            {
                let mut observed = self.state.observed.lock().unwrap();
                let value = &mut observed["searcher_closes"];
                *value = json!(value.as_u64().unwrap() + 1);
            }
            self.facade.close().await
        })
    }
}
pub fn searcher(state: Arc<State>, root: PathBuf, snapshots: PathBuf) -> ObservedSearcher {
    let loader = OwnedLoader {
        loader: Loader::new(SourceLoaderOptions {
            temp_dir: snapshots,
            http_transport: Some(Arc::new(SourceTransport(state.clone()))),
            ..SourceLoaderOptions::default()
        }),
        root,
    };
    let aggregator = Aggregator::new(AggregatorDependencies {
        sauce_nao: Some(Arc::new(Sauce(state.clone()))),
        ascii2d: Some(Arc::new(Ascii(state.clone()))),
    });
    ObservedSearcher {
        facade: Facade::new(Dependencies {
            sources: Some(Arc::new(loader)),
            payloads: Some(Arc::new(aggregator)),
        }),
        state,
    }
}
pub fn runtime(input: &Value) -> config::RuntimeConfig {
    let environment = input["environment_overrides"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(key, value)| (key.clone(), value.as_str().unwrap().to_owned()));
    config::Snapshot::parse(string(input, "config_before"), environment)
        .unwrap()
        .runtime()
        .unwrap()
}
pub fn finish(
    result: Result<(), CommandError>,
    args: &Arguments,
    mode: DetailOutput,
    errors: &mut Writer,
) -> i32 {
    pixiv_cli_rs::finish_command(
        result,
        mode == DetailOutput::Ndjson,
        args.json.is_some() || args.ndjson,
        errors,
    )
}
