use crate::CommandError;
use pixiv_app::{
    execution::Execution,
    facade::UseOutcome,
    lifecycle::{Context, ContextError},
    scheduler::SchedulerError,
};
use pixiv_sdk::{
    Client, Error, Reason,
    dto::UgoiraMetadataDto,
    error::{Cause, TransportKind},
    models::{ArtworkKind, UgoiraMetadata},
    reference::{REFERENCE_KIND_ARTWORK, parse_url},
    transport::Transport,
};
use std::{
    error::Error as StdError,
    io::{self, Write},
    sync::{Arc, Mutex},
};

#[derive(clap::Args, Clone, Debug)]
pub struct UgoiraOptions {
    #[arg(value_name = "ID_OR_URL", num_args = 0..)]
    pub sources: Vec<String>,
    #[arg(long, short = 'j', num_args = 0..=1, require_equals = true, default_missing_value = "true", value_parser = crate::timeline::timeline_boolean)]
    pub json: Option<bool>,
}

impl UgoiraOptions {
    pub fn validate_arguments(&self) -> Result<(), CommandError> {
        if self.sources.len() != 1 {
            return Err(CommandError::Usage(
                "ugoira requires one artwork ID or URL".into(),
            ));
        }
        Ok(())
    }

    pub fn artwork_id(&self) -> pixiv_sdk::Result<i64> {
        let invalid = || {
            Error::new(Reason::InvalidArgument, "ugoira")
                .with_detail("argument must be an artwork ID or a Pixiv artwork URL")
        };
        let source = self.sources.first().ok_or_else(invalid)?.trim();
        if let Ok(id) = source.parse::<i64>()
            && id > 0
        {
            return Ok(id);
        }
        let reference = parse_url(source).map_err(|_| invalid())?;
        if reference.kind != REFERENCE_KIND_ARTWORK {
            return Err(invalid());
        }
        Ok(reference.id)
    }

    pub fn output_json(&self, configured_json: bool) -> bool {
        self.json.unwrap_or(configured_json)
    }
}

pub fn configure_command(command: clap::Command) -> clap::Command {
    command.mut_arg("no_proxy", |argument| {
        argument.value_parser(clap::builder::ValueParser::new(
            crate::timeline::timeline_boolean,
        ))
    })
}

pub fn argument_error(error: &clap::Error) -> Option<CommandError> {
    if error.kind() == clap::error::ErrorKind::UnknownArgument {
        let invalid = error.get(clap::error::ContextKind::InvalidArg)?.to_string();
        let name = invalid.split('=').next()?;
        if name.starts_with('-') {
            return Some(CommandError::Usage(format!("unknown option '{name}'")));
        }
    }
    crate::timeline::argument_error(error)
}

pub async fn ugoira<T: Transport, W: Write>(
    client: &Client<T>,
    id: i64,
    json: bool,
    output: &mut W,
) -> Result<(), CommandError> {
    let metadata = metadata(client, None, id).await?;
    present(&metadata, json, output)
}

pub async fn saved_ugoira<T: Transport + 'static, W: Write + Send + 'static>(
    execution: &Execution<T>,
    context: &Context,
    id: i64,
    proxy: Option<&str>,
    json: bool,
    output: W,
) -> Result<(), CommandError> {
    let output = Arc::new(Mutex::new(output));
    let original = Arc::new(Mutex::new(None));
    let retained = original.clone();
    let result = execution
        .use_client(
            Some(context),
            0,
            proxy,
            Some(Arc::new(move |context, client| {
                let output = output.clone();
                let retained = retained.clone();
                Box::pin(async move {
                    let result = match metadata(&client, Some(&context), id).await {
                        Ok(metadata) => present(
                            &metadata,
                            json,
                            &mut *output.lock().unwrap_or_else(|error| error.into_inner()),
                        ),
                        Err(error) => Err(error.into()),
                    };
                    let error = match result {
                        Ok(()) => {
                            *retained.lock().unwrap_or_else(|error| error.into_inner()) = None;
                            None
                        }
                        Err(CommandError::Sdk(error)) => Some(SchedulerError::from(error)),
                        Err(CommandError::App(error)) => Some(error),
                        Err(CommandError::Output(error)) if writer_sdk_error(&error).is_some() => {
                            Some(SchedulerError::from(
                                writer_sdk_error(&error).unwrap().clone(),
                            ))
                        }
                        Err(error) => {
                            let message = error.to_string();
                            *retained.lock().unwrap_or_else(|error| error.into_inner()) =
                                Some(error);
                            Some(SchedulerError::Message(message))
                        }
                    };
                    UseOutcome {
                        committed: false,
                        error,
                    }
                })
            })),
        )
        .await;
    if let Some(error) = original
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take()
    {
        return Err(error);
    }
    result.map_err(Into::into)
}

fn writer_sdk_error(error: &io::Error) -> Option<&Error> {
    let mut current = error.get_ref()? as &(dyn StdError + 'static);
    loop {
        if let Some(error) = current.downcast_ref::<Error>() {
            return Some(error);
        }
        current = if let Some(error) = current.downcast_ref::<io::Error>() {
            error.get_ref()? as &(dyn StdError + 'static)
        } else {
            current.source()?
        };
    }
}

async fn metadata<T: Transport>(
    client: &Client<T>,
    context: Option<&Context>,
    id: i64,
) -> pixiv_sdk::Result<UgoiraMetadata> {
    let artwork = if let Some(context) = context {
        tokio::select! {
            biased;
            result = client.artwork(id) => result,
            error = context.cancelled() => Err(context_error("Artwork", error)),
        }
    } else {
        client.artwork(id).await
    }?;
    if artwork.kind != ArtworkKind::Ugoira {
        return Err(Error::new(Reason::NotUgoira, "ugoira").with_detail("artwork is not a ugoira"));
    }
    let mut metadata = if let Some(context) = context {
        tokio::select! {
            biased;
            result = client.ugoira_metadata(id) => result,
            error = context.cancelled() => Err(context_error("UgoiraMetadata", error)),
        }
    } else {
        client.ugoira_metadata(id).await
    }?;
    metadata
        .archives
        .sort_by_key(|archive| archive.quality != "original");
    Ok(metadata)
}

fn context_error(operation: &str, error: ContextError) -> Error {
    Error::new(Reason::UpstreamUnavailable, operation)
        .with_transport(TransportKind::Http)
        .with_cause(Cause::TransportFailure(Box::new(match error {
            ContextError::Canceled => Cause::Canceled,
            ContextError::DeadlineExceeded => Cause::DeadlineExceeded,
        })))
}

fn present<W: Write>(
    metadata: &UgoiraMetadata,
    json: bool,
    output: &mut W,
) -> Result<(), CommandError> {
    if json {
        let body = serde_json::to_string_pretty(&UgoiraMetadataDto::from(metadata))
            .map_err(|_| Error::new(Reason::LocalStateError, "output"))?;
        let body = format!("{}\n", crate::go_json_escape(body));
        let _ = output.write(body.as_bytes())?;
        return Ok(());
    }
    let _ = output.write(format!("artwork: {}\n", metadata.artwork_id).as_bytes())?;
    for archive in &metadata.archives {
        let _ = output.write(format!("archive: {}\n", archive.quality).as_bytes())?;
    }
    let _ = output.write(format!("frames: {}\n", metadata.frames.len()).as_bytes())?;
    for frame in &metadata.frames {
        let _ = output
            .write(format!("{} {}ms\n", frame.filename, frame.delay_milliseconds).as_bytes())?;
    }
    Ok(())
}
