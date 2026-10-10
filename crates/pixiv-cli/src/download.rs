use crate::CommandError;
use pixiv_app::{
    config::Store,
    database::Database,
    download::{
        DownloadAttempt, DownloadReport, DownloadRequest, DownloadSaveClient,
        NativeDownloadSaveClient, download_sources, parse_pages, validate_quality,
        validate_ugoira_format,
    },
    execution::Execution,
    facade::UseOutcome,
    lifecycle::Context,
    scheduler::SchedulerError,
};
use pixiv_sdk::{Client, models::UgoiraFrame, transport::Transport};
use serde::Serialize;
use std::{
    io::{self, BufRead, Read, Write},
    sync::{Arc, Mutex},
};

pub struct DownloadSinks<W, E> {
    pub output: Arc<Mutex<W>>,
    pub error: Arc<Mutex<E>>,
}

#[derive(Clone, Copy)]
pub struct DownloadLifecycle<'a> {
    pub context: &'a Context,
    pub post_success: crate::update::PostSuccess<'a>,
}

#[derive(Clone, Default)]
pub struct DownloadRuntime {
    pub download_path: String,
    pub filename_template: String,
    pub directory_template: String,
    pub output_json: bool,
}
#[derive(Clone)]
pub struct DownloadOptions {
    sources: Vec<String>,
    record_prefix: Option<Vec<u8>>,
    download_path: Option<String>,
    output: Option<String>,
    filename_template: Option<String>,
    pages: String,
    quality: String,
    ugoira_mode: String,
    on_error: String,
    json: Option<bool>,
    ndjson: Option<bool>,
    proxy: Option<String>,
    no_proxy: Option<bool>,
}
pub enum DownloadCommand {
    Run(Box<DownloadOptions>),
    Help(String),
}
#[derive(Clone)]
struct Prepared {
    sources: Vec<String>,
    request: DownloadRequest,
    machine: bool,
    ndjson: bool,
    fail_fast: bool,
}
#[derive(Clone, Copy)]
enum Presentation {
    Report,
    Record,
}
fn usage(value: impl Into<String>) -> CommandError {
    CommandError::Usage(value.into())
}
impl DownloadCommand {
    pub fn discover(args: &[String]) -> bool {
        args.first().is_some_and(|arg| arg == "download")
    }
    fn parse_flags(args: &[String]) -> (DownloadOptions, Result<bool, CommandError>) {
        let mut options = DownloadOptions {
            sources: vec![],
            record_prefix: None,
            download_path: None,
            output: None,
            filename_template: None,
            pages: String::new(),
            quality: "original".into(),
            ugoira_mode: "gif".into(),
            on_error: "skip".into(),
            json: None,
            ndjson: None,
            proxy: None,
            no_proxy: None,
        };
        let result = (|| -> Result<bool, CommandError> {
            let mut index = usize::from(Self::discover(args));
            let mut flags = true;
            let mut help = false;
            while index < args.len() {
                let arg = &args[index];
                index += 1;
                if flags && arg == "--" {
                    flags = false;
                    continue;
                }
                if flags && arg.starts_with("--") {
                    let (name, raw) = arg
                        .split_once('=')
                        .map_or((arg.as_str(), None), |(name, value)| (name, Some(value)));
                    match name {
                        "--help" => {
                            help = crate::auth_accounts::boolean(raw.unwrap_or("true"), "--help")?
                        }
                        "--json" => {
                            options.json = Some(crate::auth_accounts::boolean(
                                raw.unwrap_or("true"),
                                "--json",
                            )?)
                        }
                        "--ndjson" => {
                            options.ndjson = Some(crate::auth_accounts::boolean(
                                raw.unwrap_or("true"),
                                "--ndjson",
                            )?)
                        }
                        "--no-proxy" => {
                            options.no_proxy = Some(crate::auth_accounts::boolean(
                                raw.unwrap_or("true"),
                                "--no-proxy",
                            )?)
                        }
                        "--download-path"
                        | "--output"
                        | "--filename-template"
                        | "--pages"
                        | "--quality"
                        | "--ugoira-mode"
                        | "--on-error"
                        | "--proxy" => {
                            let value = if let Some(raw) = raw {
                                raw.to_owned()
                            } else {
                                let value = args
                                    .get(index)
                                    .ok_or_else(|| {
                                        usage(format!("flag needs an argument: {name}"))
                                    })?
                                    .clone();
                                index += 1;
                                value
                            };
                            match name {
                                "--download-path" => options.download_path = Some(value),
                                "--output" => options.output = Some(value),
                                "--filename-template" => options.filename_template = Some(value),
                                "--pages" => options.pages = value,
                                "--quality" => options.quality = value,
                                "--ugoira-mode" => options.ugoira_mode = value,
                                "--on-error" => options.on_error = value,
                                "--proxy" => options.proxy = Some(value),
                                _ => unreachable!(),
                            }
                        }
                        _ => return Err(usage(format!("unknown flag: {name}"))),
                    }
                    continue;
                }
                if flags && arg.starts_with('-') && arg != "-" {
                    let mut chars = arg[1..].chars().peekable();
                    while let Some(ch) = chars.next() {
                        match ch {
                            'h' | 'j' => {
                                let value = if chars.peek() == Some(&'=') {
                                    chars.next();
                                    crate::auth_accounts::boolean(
                                        &chars.by_ref().collect::<String>(),
                                        if ch == 'h' {
                                            "-h, --help"
                                        } else {
                                            "-j, --json"
                                        },
                                    )?
                                } else {
                                    true
                                };
                                if ch == 'h' {
                                    help = value
                                } else {
                                    options.json = Some(value)
                                }
                            }
                            'o' => {
                                let mut value = chars.by_ref().collect::<String>();
                                if value.starts_with('=') {
                                    value.remove(0);
                                }
                                if value.is_empty() {
                                    value = args
                                        .get(index)
                                        .ok_or_else(|| usage("flag needs an argument: 'o' in -o"))?
                                        .clone();
                                    index += 1;
                                }
                                options.output = Some(value);
                                break;
                            }
                            _ => {
                                return Err(usage(format!(
                                    "unknown shorthand flag: {ch:?} in {arg}"
                                )));
                            }
                        }
                    }
                    continue;
                }
                options.sources.push(arg.clone());
            }
            Ok(help)
        })();
        (options, result)
    }
    pub fn output_policy_requested(args: &[String]) -> (bool, bool) {
        let (options, _) = Self::parse_flags(args);
        (
            options.ndjson == Some(true),
            options.json.is_some() || options.ndjson == Some(true),
        )
    }
    pub fn parse<R: Read + ?Sized>(
        args: &[String],
        input: &mut R,
        terminal: bool,
    ) -> Result<Self, CommandError> {
        let (mut options, parsed) = Self::parse_flags(args);
        let help = parsed?;
        if help {
            return Ok(Self::Help(help_text().into()));
        }
        if options.sources.is_empty() {
            if terminal {
                return Err(usage("usage: pixiv download [options] SRC..."));
            }
            let mut prefix = Vec::new();
            loop {
                let mut byte = [0u8; 1];
                let count = input
                    .read(&mut byte)
                    .map_err(|error| usage(format!("read stdin input: {error}")))?;
                if count == 0 {
                    break;
                }
                prefix.push(byte[0]);
                if !matches!(byte[0], b' ' | b'\t' | b'\r' | b'\n') {
                    if byte[0] == b'{' {
                        options.record_prefix = Some(prefix.clone());
                    }
                    break;
                }
            }
            if options.record_prefix.is_none() {
                input
                    .read_to_end(&mut prefix)
                    .map_err(|error| usage(format!("read stdin input: {error}")))?;
                if prefix.ends_with(b"\r\n") {
                    prefix.truncate(prefix.len() - 2);
                } else if prefix.ends_with(b"\n") {
                    prefix.pop();
                }
                if !prefix.is_empty() {
                    options
                        .sources
                        .push(String::from_utf8_lossy(&prefix).into_owned());
                }
            }
        }
        Ok(Self::Run(Box::new(options)))
    }
    pub fn parse_root<R: Read + ?Sized>(
        args: &[String],
        input: &mut R,
        terminal: bool,
    ) -> Result<Self, CommandError> {
        Self::parse(args, input, terminal).map_err(|error| match error {
            CommandError::Usage(message) => {
                if let Some(name) = message.strip_prefix("unknown flag: ") {
                    usage(format!("unknown option '{name}'"))
                } else if let Some(short) = message
                    .strip_prefix("unknown shorthand flag: ")
                    .and_then(|value| value.split('\'').nth(1))
                {
                    usage(format!("unknown option '-{short}'"))
                } else {
                    CommandError::Usage(message)
                }
            }
            other => other,
        })
    }
    pub fn render_help(&self, command_path: &str) -> Option<String> {
        let Self::Help(text) = self else { return None };
        Some(text.replace(
            "  download [SRC...] [flags]",
            &format!("  {command_path} [SRC...] [flags]"),
        ))
    }
    pub fn requires_config(&self) -> bool {
        matches!(self, Self::Run(_))
    }
    pub fn requires_startup(&self) -> bool {
        self.requires_config()
    }
    pub fn machine_output(&self) -> bool {
        matches!(self,Self::Run(options) if options.json.is_some()||options.ndjson==Some(true))
    }
    pub fn proxy_override(&self) -> Result<Option<&str>, CommandError> {
        let Self::Run(options) = self else {
            return Ok(None);
        };
        if options.proxy.is_some() && options.no_proxy.is_some() {
            return Err(CommandError::Message(
                "use either --proxy or --no-proxy, not both",
            ));
        }
        Ok(if options.no_proxy == Some(true) {
            Some("")
        } else {
            options.proxy.as_deref()
        })
    }
    fn prepare<F: FnOnce() -> Result<DownloadRuntime, CommandError>>(
        &self,
        load: F,
    ) -> Result<Prepared, CommandError> {
        let Self::Run(options) = self else {
            unreachable!()
        };
        let fail_fast = match options.on_error.as_str() {
            "skip" => false,
            "fail-fast" => true,
            _ => return Err(usage("on-error must be one of: skip, fail-fast")),
        };
        let pages = parse_pages(&options.pages).map_err(|error| usage(error.to_string()))?;
        let quality = if options.quality.is_empty() {
            "original"
        } else {
            &options.quality
        };
        validate_quality(quality).map_err(|error| usage(error.to_string()))?;
        let format = if options.ugoira_mode.is_empty() {
            "gif"
        } else {
            &options.ugoira_mode
        };
        validate_ugoira_format(format).map_err(|error| usage(error.to_string()))?;
        self.proxy_override()?;
        let mut runtime = load()?;
        if let (Some(path), Some(output)) = (&options.download_path, &options.output)
            && path != output
        {
            return Err(usage(
                "--output and --download-path must name the same directory when both are provided",
            ));
        }
        if let Some(path) = options.output.as_ref().or(options.download_path.as_ref()) {
            runtime.download_path = path.clone();
        }
        if let Some(template) = &options.filename_template {
            runtime.filename_template = template.clone();
        }
        if options.record_prefix.is_none()
            && !options.sources.is_empty()
            && options.json.is_some()
            && options.ndjson.is_some()
        {
            return Err(usage("--json and --ndjson cannot be used together"));
        }
        Ok(Prepared {
            sources: options.sources.clone(),
            request: DownloadRequest {
                download_path: runtime.download_path,
                filename_template: runtime.filename_template,
                directory_template: runtime.directory_template,
                pages,
                quality: quality.into(),
                ugoira_format: format.into(),
            },
            machine: options.ndjson == Some(true) || options.json.unwrap_or(runtime.output_json),
            ndjson: options.ndjson == Some(true),
            fail_fast,
        })
    }
    pub async fn execute_http<
        R: Read + ?Sized,
        W: Write + Send + 'static,
        E: Write + Send + 'static,
    >(
        &self,
        store: &Store,
        context: &Context,
        input: &mut R,
        out: Arc<Mutex<W>>,
        err: Arc<Mutex<E>>,
    ) -> Result<(), CommandError> {
        self.execute_http_with_post_success(
            store,
            context,
            input,
            DownloadSinks {
                output: out,
                error: err,
            },
            None,
        )
        .await
    }
    pub async fn execute_http_with_post_success<
        'a,
        R: Read + ?Sized,
        W: Write + Send + 'static,
        E: Write + Send + 'static,
    >(
        &self,
        store: &Store,
        context: &'a Context,
        input: &mut R,
        sinks: DownloadSinks<W, E>,
        post_success: crate::update::PostSuccess<'a>,
    ) -> Result<(), CommandError> {
        let mut retained_database = None;
        let result = self
            .execute_with_factory_and_post_success(
                DownloadLifecycle {
                    context,
                    post_success,
                },
                || {
                    let runtime = store
                        .current()
                        .and_then(|snapshot| snapshot.runtime())
                        .map_err(SchedulerError::from)?;
                    Ok(DownloadRuntime {
                        download_path: runtime.download_path,
                        filename_template: runtime.filename_template,
                        directory_template: runtime.directory_template,
                        output_json: runtime.output_json,
                    })
                },
                input,
                sinks,
                || {
                    let database = Database::open(store.path().parent().unwrap())
                        .map_err(|error| CommandError::State(Box::new(error)))?;
                    let database = Arc::new(Mutex::new(database));
                    retained_database = Some(database.clone());
                    Ok(Execution::http(store.clone(), database))
                },
                |client| Arc::new(NativeDownloadSaveClient::new(client)),
            )
            .await;
        let cleanup = retained_database
            .map(|database| {
                Arc::try_unwrap(database)
                    .map_err(|_| {
                        CommandError::Message(
                            "download service retained the database after execution",
                        )
                    })
                    .and_then(|database| {
                        database
                            .into_inner()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .close()
                            .map_err(|error| CommandError::State(Box::new(error)))
                    })
            })
            .unwrap_or(Ok(()));
        crate::finish_with_cleanup(result, cleanup)
    }
    pub async fn execute_with_factory<
        T: Transport + 'static,
        R: Read + ?Sized,
        W: Write + Send + 'static,
        E: Write + Send + 'static,
        L,
        X,
        F,
    >(
        &self,
        context: &Context,
        load: L,
        input: &mut R,
        sinks: DownloadSinks<W, E>,
        open: X,
        factory: F,
    ) -> Result<(), CommandError>
    where
        L: FnOnce() -> Result<DownloadRuntime, CommandError>,
        X: FnOnce() -> Result<Execution<T>, CommandError>,
        F: Fn(Arc<Client<T>>) -> Arc<dyn DownloadSaveClient> + Send + Sync + 'static,
    {
        self.execute_with_factory_and_post_success(
            DownloadLifecycle {
                context,
                post_success: None,
            },
            load,
            input,
            sinks,
            open,
            factory,
        )
        .await
    }
    pub async fn execute_with_factory_and_post_success<
        T: Transport + 'static,
        R: Read + ?Sized,
        W: Write + Send + 'static,
        E: Write + Send + 'static,
        L,
        X,
        F,
    >(
        &self,
        lifecycle: DownloadLifecycle<'_>,
        load: L,
        input: &mut R,
        sinks: DownloadSinks<W, E>,
        open: X,
        factory: F,
    ) -> Result<(), CommandError>
    where
        L: FnOnce() -> Result<DownloadRuntime, CommandError>,
        X: FnOnce() -> Result<Execution<T>, CommandError>,
        F: Fn(Arc<Client<T>>) -> Arc<dyn DownloadSaveClient> + Send + Sync + 'static,
    {
        let context = lifecycle.context;
        let DownloadSinks {
            output: out,
            error: err,
        } = sinks;
        if let Self::Help(text) = self {
            out.lock()
                .unwrap_or_else(|e| e.into_inner())
                .write_all(text.as_bytes())?;
            return Ok(());
        }
        let prepared = self.prepare(load)?;
        let Self::Run(options) = self else {
            unreachable!()
        };
        if options.sources.is_empty() {
            return records(
                self,
                lifecycle,
                input,
                prepared,
                DownloadSinks {
                    output: out,
                    error: err,
                },
                open,
                factory,
            )
            .await;
        }
        let execution = open()?;
        let result = self
            .execute_prepared(
                &execution,
                context,
                prepared,
                DownloadSinks {
                    output: out,
                    error: err,
                },
                factory,
                Presentation::Report,
            )
            .await;
        crate::update::notify_post_success(
            result,
            lifecycle.post_success,
            crate::update::PostSuccessPolicy::default(),
        )
        .await
    }
    async fn execute_prepared<
        T: Transport + 'static,
        W: Write + Send + 'static,
        E: Write + Send + 'static,
        F,
    >(
        &self,
        execution: &Execution<T>,
        context: &Context,
        prepared: Prepared,
        sinks: DownloadSinks<W, E>,
        factory: F,
        presentation: Presentation,
    ) -> Result<(), CommandError>
    where
        F: Fn(Arc<Client<T>>) -> Arc<dyn DownloadSaveClient> + Send + Sync + 'static,
    {
        let DownloadSinks {
            output: out,
            error: err,
        } = sinks;
        let sources = prepared.sources.clone();
        let result = Arc::new(Mutex::new(None));
        let captured = result.clone();
        let callback = Arc::new(
            move |context: Context, client: Arc<Client<T>>| -> pixiv_app::facade::UseFuture {
                let sources = sources.clone();
                let request = prepared.request.clone();
                let out = out.clone();
                let err = err.clone();
                let captured = captured.clone();
                let client = factory(client);
                let modes = Prepared {
                    sources: sources.clone(),
                    request: request.clone(),
                    machine: prepared.machine,
                    ndjson: prepared.ndjson,
                    fail_fast: prepared.fail_fast,
                };
                Box::pin(async move {
                    let attempt =
                        download_sources(&context, client.as_ref(), &sources, &request).await;
                    let committed = report_committed(&attempt.report);
                    let outcome = match presentation {
                        Presentation::Report => present(
                            attempt,
                            &modes,
                            &mut *out.lock().unwrap_or_else(|e| e.into_inner()),
                            &mut *err.lock().unwrap_or_else(|e| e.into_inner()),
                        ),
                        Presentation::Record => present_record(
                            attempt,
                            &mut *err.lock().unwrap_or_else(|e| e.into_inner()),
                        ),
                    };
                    let (original, error) = match outcome {
                        Ok(()) => (None, None),
                        Err(error) => {
                            let (original, error) = split_error(error);
                            (Some(original), Some(SchedulerError::Shared(error)))
                        }
                    };
                    *captured.lock().unwrap_or_else(|e| e.into_inner()) = Some(original);
                    UseOutcome { committed, error }
                })
            },
        );
        let operation = execution
            .use_client(Some(context), 0, self.proxy_override()?, Some(callback))
            .await;
        if let Some(Some(original)) = result.lock().unwrap_or_else(|e| e.into_inner()).take() {
            match original {
                CommandError::Output(_) | CommandError::Pipeline => return Err(original),
                _ => {}
            }
        }
        operation.map_err(Into::into)
    }
}
fn split_error(error: CommandError) -> (CommandError, Arc<SchedulerError>) {
    match error {
        CommandError::App(error) => {
            let error = Arc::new(error);
            (
                CommandError::App(SchedulerError::Shared(error.clone())),
                error,
            )
        }
        CommandError::Sdk(error) => {
            let error = Arc::new(SchedulerError::from(error));
            (
                CommandError::App(SchedulerError::Shared(error.clone())),
                error,
            )
        }
        other => {
            let scheduling = Arc::new(SchedulerError::Message(other.to_string()));
            (other, scheduling)
        }
    }
}
fn report_committed(report: &DownloadReport) -> bool {
    report.committed || report.items.iter().any(|item| !item.files.is_empty())
}
fn report_error(report: &DownloadReport) -> Option<SchedulerError> {
    let first = report.failures.first()?;
    Some(SchedulerError::Wrapped {
        message: format!("download completed with {} failures", report.failures.len()),
        source: Box::new(SchedulerError::Shared(first.cause.clone())),
    })
}
#[derive(Serialize)]
struct Artifact<'a> {
    #[serde(skip_serializing_if = "is_zero")]
    artwork_id: i64,
    kind: &'a str,
    #[serde(skip_serializing_if = "is_zero")]
    page: i64,
    path: String,
    bytes: i64,
    #[serde(skip_serializing_if = "str::is_empty")]
    quality: &'a str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    frames: Vec<UgoiraFrame>,
    #[serde(skip_serializing_if = "Option::is_none")]
    frame_report: Option<FrameReport<'a>>,
}
#[derive(Serialize)]
struct FrameReport<'a> {
    declared: usize,
    actual: usize,
    undeclared: Option<&'a [String]>,
    missing: Option<&'a [String]>,
}
fn is_zero(n: &i64) -> bool {
    *n == 0
}
#[derive(Serialize)]
struct Failure {
    #[serde(skip_serializing_if = "is_zero")]
    artwork_id: i64,
    error: FailureDetail,
}
#[derive(Serialize)]
struct FailureDetail {
    code: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    path: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    missing: Vec<String>,
}
#[derive(Serialize)]
#[serde(untagged)]
enum ReportRecord<'a> {
    Artifact(Artifact<'a>),
    Failure(Failure),
}
fn write_warnings<E: Write + ?Sized>(report: &DownloadReport, err: &mut E) -> io::Result<()> {
    for warning in &report.warnings {
        let mut label = if warning.illust_id > 0 {
            format!("artwork {}", warning.illust_id)
        } else {
            "download".into()
        };
        if !warning.kind.is_empty() {
            label.push_str(&format!(" ({})", warning.kind));
        }
        writeln!(err, "warning: {label}: {}", warning.message)?;
    }
    Ok(())
}
fn present_record<E: Write + ?Sized>(
    attempt: DownloadAttempt,
    err: &mut E,
) -> Result<(), CommandError> {
    let business = attempt.error.or_else(|| report_error(&attempt.report));
    let warning = write_warnings(&attempt.report, err).err();
    match (business, warning) {
        (None, None) => Ok(()),
        (Some(error), None) => Err(error.into()),
        (None, Some(error)) => Err(SchedulerError::Message(error.to_string()).into()),
        (Some(error), Some(warning)) => {
            Err(
                SchedulerError::Joined(vec![error, SchedulerError::Message(warning.to_string())])
                    .into(),
            )
        }
    }
}
fn present<W: Write + ?Sized, E: Write + ?Sized>(
    attempt: DownloadAttempt,
    prepared: &Prepared,
    out: &mut W,
    err: &mut E,
) -> Result<(), CommandError> {
    let report = attempt.report;
    write_warnings(&report, err)?;
    if let Some(error) = attempt.error {
        return Err(error.into());
    }
    if prepared.machine
        && !prepared.fail_fast
        && (!report.items.is_empty() || !report.failures.is_empty())
    {
        let mut records = Vec::new();
        for item in &report.items {
            for file in &item.files {
                records.push(ReportRecord::Artifact(Artifact {
                    artwork_id: item.illust_id,
                    kind: &item.kind,
                    page: if matches!(item.kind.as_str(), "resource" | "ugoira") {
                        0
                    } else {
                        file.page
                    },
                    path: file.path.to_string_lossy().into_owned(),
                    bytes: file.bytes,
                    quality: if item.kind == "ugoira" {
                        &item.quality
                    } else {
                        ""
                    },
                    frames: if item.kind == "ugoira" {
                        item.frames.clone()
                    } else {
                        Vec::new()
                    },
                    frame_report: if item.kind == "ugoira" {
                        item.frame_report.as_ref().map(|report| FrameReport {
                            declared: report.declared,
                            actual: report.actual,
                            undeclared: report.undeclared.as_deref(),
                            missing: report.missing.as_deref(),
                        })
                    } else {
                        None
                    },
                }));
            }
        }
        for failure in &report.failures {
            let code = if !failure.code.is_empty() {
                failure.code.clone()
            } else {
                failure
                    .cause
                    .classified()
                    .map(|e| e.code.as_str().to_owned())
                    .unwrap_or("command_failed".into())
            };
            records.push(ReportRecord::Failure(Failure {
                artwork_id: failure.illust_id,
                error: FailureDetail {
                    code,
                    path: failure.path.to_string_lossy().into_owned(),
                    missing: failure.missing.clone(),
                },
            }));
        }
        if prepared.ndjson {
            for record in records {
                writeln!(
                    out,
                    "{}",
                    crate::go_json_escape(
                        serde_json::to_string(&record)
                            .map_err(|_| CommandError::Message("cannot encode download report"))?
                    )
                )?;
            }
        } else {
            writeln!(
                out,
                "{}",
                crate::go_json_escape(
                    serde_json::to_string_pretty(&records)
                        .map_err(|_| CommandError::Message("cannot encode download report"))?
                )
            )?;
        }
    }
    if report.failures.is_empty() {
        Ok(())
    } else if prepared.machine && !prepared.fail_fast {
        Err(CommandError::Pipeline)
    } else {
        Err(report_error(&report).unwrap().into())
    }
}
#[derive(Serialize)]
struct Diagnostic<'a> {
    kind: &'a str,
    operation: &'a str,
    line: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    r#type: String,
    code: &'a str,
    message: String,
}
async fn records<
    T: Transport + 'static,
    R: Read + ?Sized,
    W: Write + Send + 'static,
    E: Write + Send + 'static,
    X: FnOnce() -> Result<Execution<T>, CommandError>,
    F: Fn(Arc<Client<T>>) -> Arc<dyn DownloadSaveClient> + Send + Sync + 'static,
>(
    command: &DownloadCommand,
    lifecycle: DownloadLifecycle<'_>,
    input: &mut R,
    prepared: Prepared,
    sinks: DownloadSinks<W, E>,
    open: X,
    factory: F,
) -> Result<(), CommandError> {
    let context = lifecycle.context;
    let DownloadCommand::Run(options) = command else {
        unreachable!()
    };
    let mut execution = None;
    let mut open = Some(open);
    let factory = Arc::new(factory);
    let prefix = options.record_prefix.clone().unwrap_or_default();
    let mut reader = io::BufReader::new(io::Cursor::new(prefix).chain(input));
    let mut number = 0;
    let mut failed = false;
    loop {
        if let Some(error) = context.error() {
            return Err(SchedulerError::from(error).into());
        }
        let mut line = Vec::new();
        let count = reader
            .read_until(b'\n', &mut line)
            .map_err(|error| CommandError::MessageText(format!("read NDJSON input: {error}")))?;
        if count == 0 {
            break;
        }
        number += 1;
        let at_eof = !line.ends_with(b"\n");
        let parsed = crate::record_input::parse_go_line(&line);
        let (code, message) = match parsed {
            Err(message) => ("invalid_record", message),
            Ok((_, typ, _))
                if !matches!(typ.as_str(), "artwork" | "illust" | "manga" | "ugoira") =>
            {
                if let Some(error) = context.error() {
                    return Err(SchedulerError::from(error).into());
                }
                (
                    "unsupported_type",
                    format!(
                        "record type {} is not supported by download",
                        crate::search::quote(&typ)
                    ),
                )
            }
            Ok((id, _, _)) => {
                let id = id.parse::<i64>().ok().filter(|id| *id > 0);
                let Some(id) = id else {
                    if let Some(error) = context.error() {
                        return Err(SchedulerError::from(error).into());
                    }
                    write_record_diagnostic(
                        &sinks.error,
                        &line,
                        number,
                        "invalid_id",
                        "record id must be a positive integer: record id must be a positive integer".into(),
                    )?;
                    failed = true;
                    if prepared.fail_fast || at_eof {
                        break;
                    }
                    continue;
                };
                let executor = execution.get_or_insert_with(|| {
                    open.take().unwrap()().map_err(|error| split_error(error).1)
                });
                let outcome = match executor {
                    Ok(execution) => {
                        let mut request = prepared.clone();
                        request.sources = vec![id.to_string()];
                        request.machine = false;
                        request.ndjson = false;
                        let factory = factory.clone();
                        command
                            .execute_prepared(
                                execution,
                                context,
                                request,
                                DownloadSinks {
                                    output: sinks.output.clone(),
                                    error: sinks.error.clone(),
                                },
                                move |client| factory(client),
                                Presentation::Record,
                            )
                            .await
                    }
                    Err(error) => Err(SchedulerError::Shared(error.clone()).into()),
                };
                match outcome {
                    Ok(()) if at_eof => break,
                    Ok(()) => continue,
                    Err(error) => {
                        if let Some(error) = context.error() {
                            return Err(SchedulerError::from(error).into());
                        }
                        if let CommandError::Output(error) = error {
                            return Err(CommandError::Output(error));
                        }
                        ("action_failed", error.to_string())
                    }
                }
            }
        };
        write_record_diagnostic(&sinks.error, &line, number, code, message)?;
        failed = true;
        if prepared.fail_fast || at_eof {
            break;
        }
    }
    let result = if failed {
        Err(CommandError::Pipeline)
    } else {
        Ok(())
    };
    let result = crate::update::notify_post_success(
        result,
        lifecycle.post_success,
        crate::update::PostSuccessPolicy::default(),
    )
    .await;
    drop(execution);
    result
}
fn write_record_diagnostic<E: Write + ?Sized>(
    err: &Arc<Mutex<E>>,
    line: &[u8],
    number: i64,
    code: &str,
    message: String,
) -> Result<(), CommandError> {
    let (id, typ) = crate::record_input::diagnostic_identity(line);
    let diagnostic = Diagnostic {
        kind: "record_error",
        operation: "download",
        line: number,
        id,
        r#type: typ,
        code,
        message,
    };
    writeln!(
        &mut *err.lock().unwrap_or_else(|error| error.into_inner()),
        "{}",
        crate::go_json_escape(
            serde_json::to_string(&diagnostic)
                .map_err(|_| CommandError::Message("cannot encode record diagnostic"))?
        )
    )?;
    Ok(())
}
fn help_text() -> &'static str {
    "Download illustrations\n\nUsage:\n  download [SRC...] [flags]\n\nFlags:\n      --download-path string       download directory\n      --filename-template string   filename template placeholders: {id}, {title}, {author}, {author_id}, {date}, {tags}, {num}\n  -h, --help                       help for download\n  -j, --json                       print a JSON artifact report\n      --ndjson                     print one JSON artifact record per line\n      --no-proxy                   clear the configured proxy for this command\n      --on-error string            record failure strategy: skip or fail-fast (default \"skip\")\n  -o, --output string              download directory (alias for --download-path)\n      --pages string               1-based page selection, e.g. 1,3-5; default all pages\n      --proxy string               proxy URL (http, https, socks5, or socks5h) for this command\n      --quality string             static image quality: original, regular, small, thumb, mini (default \"original\")\n      --ugoira-mode string         ugoira output mode: gif, apng (default \"gif\")\n"
}
