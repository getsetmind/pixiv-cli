use super::{
    CallerContext, Command, CommandRunner, ExternalError, UpdateFuture,
    coordinator::{AutomaticChecker, AutomaticCheckerOptions, Coordinator, CoordinatorOptions},
    go_quote,
    http::ordinary_transport,
    installer::{
        FileReleaseCache, NativeSourceDetector, ProcessExitError, ReleaseInstallerOptions,
        SignedReleaseInstaller, production_trusted_keys,
    },
    message,
    release::{CACHE_FILENAME, GitHubReleaseClient, ReleaseClientOptions},
    wrap,
};
use crate::{
    callback_handler::app_data_directory,
    host_process::{HostProcessError, ProcessStdio, SystemHostProcess, prepare_command},
    reverse_search::http::HttpTransport,
};
use futures_util::{StreamExt, stream::FuturesUnordered};
use std::{
    ffi::{OsStr, OsString},
    io::{self, Read, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_util::sync::CancellationToken;

pub type SharedUpdateWriter = Arc<Mutex<Box<dyn Write + Send>>>;

pub fn shared_writer<W: Write + Send + 'static>(writer: W) -> SharedUpdateWriter {
    Arc::new(Mutex::new(Box::new(writer)))
}

#[derive(Default)]
pub struct ProductionUpdateOptions {
    pub proxy: String,
    pub cache_directory: Option<PathBuf>,
    pub http_transport: Option<Arc<dyn HttpTransport>>,
}

type ReleasePorts = (Arc<GitHubReleaseClient>, Arc<dyn HttpTransport>, bool);

fn release_dependencies(options: ProductionUpdateOptions) -> Result<ReleasePorts, ExternalError> {
    let transport = ordinary_transport(&options.proxy)
        .map_err(|error| wrap("parse update proxy URL", error))?;
    let transport = options.http_transport.unwrap_or(transport);
    let enable_public_release_sources = options.proxy.is_empty();
    let directory = match options.cache_directory {
        Some(directory) => directory,
        None => app_data_directory()
            .map_err(|error| wrap("determine application data directory", Box::new(error)))?
            .join("cache"),
    };
    let cache = Arc::new(FileReleaseCache::new(
        directory.clone(),
        directory.join(CACHE_FILENAME),
    ));
    let checker = GitHubReleaseClient::new(ReleaseClientOptions {
        transport: Some(transport.clone()),
        cache: Some(cache),
        enable_public_release_sources,
        ..ReleaseClientOptions::default()
    })
    .map_err(|error| wrap("create GitHub release client", error))?;
    Ok((Arc::new(checker), transport, enable_public_release_sources))
}

pub fn new_coordinator(
    proxy: &str,
    out: SharedUpdateWriter,
    err_out: SharedUpdateWriter,
) -> Result<Coordinator, ExternalError> {
    new_coordinator_with_options(
        ProductionUpdateOptions {
            proxy: proxy.into(),
            ..ProductionUpdateOptions::default()
        },
        out,
        err_out,
    )
}

pub fn new_coordinator_with_options(
    options: ProductionUpdateOptions,
    out: SharedUpdateWriter,
    err_out: SharedUpdateWriter,
) -> Result<Coordinator, ExternalError> {
    let (checker, transport, enable_public_release_sources) = release_dependencies(options)?;
    Coordinator::new(CoordinatorOptions {
        source_detector: Some(Arc::new(NativeSourceDetector::default())),
        release_checker: Some(checker),
        command_runner: Some(Arc::new(ProcessCommandRunner::new(
            Some(out),
            Some(err_out),
        ))),
        release_installer: Some(Arc::new(SignedReleaseInstaller::new(
            ReleaseInstallerOptions {
                http_transport: Some(transport),
                trusted_keys: production_trusted_keys(),
                enable_public_release_sources,
                ..ReleaseInstallerOptions::default()
            },
        ))),
    })
}

pub fn new_automatic_checker(proxy: &str) -> Result<AutomaticChecker, ExternalError> {
    new_automatic_checker_with_options(ProductionUpdateOptions {
        proxy: proxy.into(),
        ..ProductionUpdateOptions::default()
    })
}

pub fn new_automatic_checker_with_options(
    options: ProductionUpdateOptions,
) -> Result<AutomaticChecker, ExternalError> {
    let (checker, _, _) = release_dependencies(options)?;
    AutomaticChecker::new(AutomaticCheckerOptions {
        source_detector: Some(Arc::new(NativeSourceDetector::default())),
        release_checker: Some(checker),
    })
}

pub struct ProcessCommandRunner {
    out: Option<SharedUpdateWriter>,
    err_out: Option<SharedUpdateWriter>,
}

impl ProcessCommandRunner {
    pub fn new(out: Option<SharedUpdateWriter>, err_out: Option<SharedUpdateWriter>) -> Self {
        Self { out, err_out }
    }
}

struct StopOnDrop(CancellationToken);
impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

fn copy_bytes(writer: &SharedUpdateWriter, bytes: &[u8]) -> io::Result<()> {
    let count = writer
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .write(bytes)?;
    if count > bytes.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid write result",
        ));
    }
    if count < bytes.len() {
        return Err(io::Error::new(io::ErrorKind::WriteZero, "short write"));
    }
    Ok(())
}

async fn copy_output<R: AsyncRead + Unpin>(
    mut reader: R,
    writer: Option<SharedUpdateWriter>,
) -> Result<(), ExternalError> {
    let mut buffer = [0u8; 32768];
    loop {
        let count = reader.read(&mut buffer).await?;
        if count == 0 {
            return Ok(());
        }
        if let Some(writer) = &writer {
            copy_bytes(writer, &buffer[..count])?;
        }
    }
}

async fn copy_combined(
    mut reader: io::PipeReader,
    writer: SharedUpdateWriter,
) -> Result<(), ExternalError> {
    tokio::task::spawn_blocking(move || {
        let mut buffer = [0u8; 32768];
        loop {
            let count = reader.read(&mut buffer)?;
            if count == 0 {
                return Ok::<(), io::Error>(());
            }
            copy_bytes(&writer, &buffer[..count])?;
        }
    })
    .await
    .map_err(|error| message(format!("wait for command output: {error}")))?
    .map_err(|error| Box::new(error) as ExternalError)
}

async fn wait_for_command(
    child: &mut tokio::process::Child,
    context: CallerContext,
    stop: CancellationToken,
) -> Result<(std::process::ExitStatus, Option<ExternalError>), ExternalError> {
    let cancellation = tokio::select! {
        biased;
        status = child.wait() => return Ok((status?, None)),
        error = context.cancelled() => Some(Box::new(error) as ExternalError),
        _ = stop.cancelled() => None,
    };
    if let Some(status) = child.try_wait()? {
        return Ok((status, None));
    }
    let cancellation = match child.start_kill() {
        Ok(()) => cancellation,
        Err(error) => Some(wrap("exec: canceling Cmd", Box::new(error))),
    };
    Ok((child.wait().await?, cancellation))
}

impl CommandRunner for ProcessCommandRunner {
    fn run(
        &self,
        context: CallerContext,
        command: Command,
    ) -> UpdateFuture<'_, Result<(), ExternalError>> {
        let out = self.out.clone();
        let err_out = self.err_out.clone();
        Box::pin(async move {
            if command.name.is_empty() {
                return Err(message("command name is empty"));
            }
            let name = command.name;
            let label = format!("run command {}", go_quote(&name));
            let same_writer = match (&out, &err_out) {
                (Some(out), Some(err)) => Arc::ptr_eq(out, err),
                _ => false,
            };
            let stdio = if same_writer {
                ProcessStdio::Combined
            } else {
                ProcessStdio::Capture
            };
            let args: Vec<OsString> = command.args.into_iter().map(OsString::from).collect();
            let (executable, mut std_command, combined) =
                prepare_command(&SystemHostProcess, OsStr::new(&name), &args, stdio)
                    .map_err(|error| wrap(&label, Box::new(error)))?;
            if out.is_none() {
                std_command.stdout(std::process::Stdio::null());
            }
            if err_out.is_none() {
                std_command.stderr(std::process::Stdio::null());
            }
            if let Some(error) = context.error() {
                return Err(wrap(label, Box::new(error)));
            }
            let stop = CancellationToken::new();
            let guard = StopOnDrop(stop.clone());
            let worker = tokio::spawn(async move {
                let mut child = tokio::process::Command::from(std_command)
                    .kill_on_drop(true)
                    .spawn()
                    .map_err(|source| {
                        Box::new(HostProcessError::Spawn {
                            program: executable.into_os_string(),
                            source,
                        }) as ExternalError
                    })?;
                let mut copies =
                    FuturesUnordered::<UpdateFuture<'static, Result<(), ExternalError>>>::new();
                if let Some(reader) = combined {
                    copies.push(Box::pin(copy_combined(
                        reader,
                        out.expect("combined writer"),
                    )));
                } else {
                    if let Some(reader) = child.stdout.take() {
                        copies.push(Box::pin(copy_output(reader, out)));
                    }
                    if let Some(reader) = child.stderr.take() {
                        copies.push(Box::pin(copy_output(reader, err_out)));
                    }
                }
                let copy = async move {
                    let mut first_error = None;
                    while let Some(result) = copies.next().await {
                        if let Err(error) = result
                            && first_error.is_none()
                        {
                            first_error = Some(error);
                        }
                    }
                    first_error.map_or(Ok(()), Err)
                };
                let (wait, copied) =
                    tokio::join!(wait_for_command(&mut child, context, stop), copy);
                let (status, cancellation) = wait?;
                if !status.success() {
                    return Err(Box::new(ProcessExitError {
                        status,
                        stderr: Vec::new(),
                    }) as ExternalError);
                }
                if let Some(error) = cancellation {
                    return Err(error);
                }
                copied
            });
            let result = worker
                .await
                .map_err(|error| message(format!("wait for command: {error}")))?
                .map_err(|error| wrap(label, error));
            drop(guard);
            result
        })
    }
}
