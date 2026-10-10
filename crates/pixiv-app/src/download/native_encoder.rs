use super::{AnimationEncoder, EncoderFuture, EncoderInput, file_replace::replace_file};
use crate::{
    auth_bundle::go_quote,
    lifecycle::{Context, ContextError},
    scheduler::SchedulerError,
};
use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock},
};
use tokio::sync::{Semaphore, oneshot};

static ENCODE_GATE: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(1)));

#[derive(Clone, Copy, Debug, Default)]
pub struct NativeAnimationEncoder;

impl AnimationEncoder for NativeAnimationEncoder {
    fn encode(&self, context: Context, input: EncoderInput) -> EncoderFuture<'_> {
        Box::pin(async move {
            let child = Context::new();
            let mut cancellation = CancelOnDrop(Some(child.clone()));
            let worker = tokio::spawn(encode_owned(
                EncodingContext {
                    parent: context,
                    child,
                },
                input,
            ));
            let result = worker.await.map_err(|error| {
                SchedulerError::Message(format!("ugoira encoder worker failed: {error}"))
            });
            cancellation.0.take();
            result?
        })
    }
}

struct CancelOnDrop(Option<Context>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(context) = &self.0 {
            context.cancel();
        }
    }
}

#[derive(Clone)]
struct EncodingContext {
    parent: Context,
    child: Context,
}

impl EncodingContext {
    fn error(&self) -> Option<ContextError> {
        self.parent.error().or_else(|| self.child.error())
    }

    async fn cancelled(&self) -> ContextError {
        tokio::select! {
            _ = self.parent.cancelled() => {},
            _ = self.child.cancelled() => {},
        }
        self.error().unwrap_or(ContextError::Canceled)
    }
}

#[derive(Serialize)]
struct FrameWire<'a> {
    file: &'a str,
    delay: i64,
}

async fn encode_owned(context: EncodingContext, input: EncoderInput) -> Result<(), SchedulerError> {
    let format = match input.format.as_str() {
        "" | "gif" => "gif",
        "apng" => "apng",
        other => {
            return Err(SchedulerError::Message(format!(
                "invalid ugoira animation format {}; expected gif or apng",
                go_quote(other)
            )));
        }
    };
    if let Some(error) = context.error() {
        return Err(error.into());
    }
    let frames = input.frames.as_ref().map(|frames| {
        frames
            .iter()
            .map(|frame| FrameWire {
                file: &frame.filename,
                delay: frame.delay_milliseconds,
            })
            .collect::<Vec<_>>()
    });
    let frames_json = serde_json::to_string(&frames)
        .map_err(|error| SchedulerError::Message(format!("marshal ugoira frames: {error}")))?;
    let mut temporary = TemporaryAnimation::create(&input.output_path)?;
    let token = Arc::new(ugoira_rs::CancellationToken::default());
    let watcher_token = Arc::clone(&token);
    let watcher_context = context.clone();
    let (stop_sender, stop_receiver) = oneshot::channel();
    let watcher = tokio::spawn(async move {
        tokio::select! {
            _ = watcher_context.cancelled() => watcher_token.cancel(),
            _ = stop_receiver => {},
        }
    });
    let permit = tokio::select! {
        permit = Arc::clone(&ENCODE_GATE).acquire_owned() => {
            permit.map_err(|error| SchedulerError::Message(format!("ugoira encoding gate failed: {error}")))
        },
        error = context.cancelled() => Err(error.into()),
    };
    let result = match permit {
        Ok(permit) => {
            let output_path = temporary.path.clone();
            let native = tokio::task::spawn_blocking(move || {
                let frames = ugoira_rs::parse_frames_json(&frames_json)?;
                match format {
                    "gif" => ugoira_rs::encode_gif(
                        &input.zip_path,
                        &frames,
                        &output_path,
                        input.max_edge,
                        &token,
                    ),
                    _ => ugoira_rs::encode_apng(
                        &input.zip_path,
                        &frames,
                        &output_path,
                        input.max_edge,
                        &token,
                    ),
                }
            })
            .await;
            drop(permit);
            match native {
                Ok(result) => result.map_err(|error| {
                    SchedulerError::Message(format!(
                        "rust ugoira encoder failed: {}",
                        error.to_string().replace('\0', " ")
                    ))
                }),
                Err(error) if error.is_panic() => {
                    let payload = error.into_panic();
                    let message = if let Some(message) = payload.downcast_ref::<&str>() {
                        *message
                    } else if let Some(message) = payload.downcast_ref::<String>() {
                        message.as_str()
                    } else {
                        "unknown panic payload"
                    };
                    Err(SchedulerError::Message(format!(
                        "rust ugoira encoder failed: panic in ugoira_encode: {}",
                        message.replace('\0', " ")
                    )))
                }
                Err(error) => Err(SchedulerError::Message(format!(
                    "ugoira encoder blocking worker failed: {error}"
                ))),
            }
        }
        Err(error) => Err(error),
    };
    let _ = stop_sender.send(());
    let watcher_result = watcher.await.map_err(|error| {
        SchedulerError::Message(format!("ugoira cancellation watcher failed: {error}"))
    });
    let mut errors = Vec::new();
    if let Err(error) = result {
        errors.push(error);
    }
    if let Err(error) = watcher_result {
        errors.push(error);
    }
    if let Some(error) = context.error()
        && !errors.iter().any(|existing| {
            matches!(
                (existing, error),
                (
                    SchedulerError::Canceled,
                    crate::lifecycle::ContextError::Canceled
                ) | (
                    SchedulerError::DeadlineExceeded,
                    crate::lifecycle::ContextError::DeadlineExceeded
                )
            )
        })
    {
        errors.push(error.into());
    }
    match errors.len() {
        0 => {}
        1 => return Err(errors.remove(0)),
        _ => return Err(SchedulerError::Joined(errors)),
    }
    if let Some(error) = context.error() {
        return Err(error.into());
    }
    match replace_file(&temporary.path, &input.output_path) {
        Ok(()) => {
            temporary.cleanup = false;
            Ok(())
        }
        Err(error) => {
            temporary.cleanup = !error.preserve_source;
            Err(error.error)
        }
    }
}

struct TemporaryAnimation {
    path: PathBuf,
    cleanup: bool,
}
impl TemporaryAnimation {
    fn create(output: &Path) -> Result<Self, SchedulerError> {
        let basename = output.file_name().unwrap_or_default().to_string_lossy();
        let extension = basename
            .rfind('.')
            .map(|index| &basename[index..])
            .filter(|extension| !extension.is_empty())
            .ok_or_else(|| {
                SchedulerError::Message(format!(
                    "ugoira output path {} has no file extension",
                    go_quote(&output.to_string_lossy())
                ))
            })?;
        let directory = crate::config::private_file::directory(output);
        for _ in 0..128 {
            let mut random = [0_u8; 16];
            getrandom::fill(&mut random)
                .map_err(|error| SchedulerError::Message(error.to_string()))?;
            let suffix = random
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            let path = directory.join(format!(".ugoira-{suffix}{extension}"));
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&path) {
                Ok(file) => {
                    let temporary = Self {
                        path,
                        cleanup: true,
                    };
                    crate::config::private_file::close(file)
                        .map_err(|error| SchedulerError::Message(error.to_string()))?;
                    return Ok(temporary);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(SchedulerError::Message(error.to_string())),
            }
        }
        Err(SchedulerError::Message(
            "could not create ugoira staging file".into(),
        ))
    }
}
impl Drop for TemporaryAnimation {
    fn drop(&mut self) {
        if self.cleanup {
            let _ = fs::remove_file(&self.path);
        }
    }
}
