use clap::{Parser, Subcommand};
use pixiv_cli_rs::{CommandError, DetailOutput, artwork_detail};
use pixiv_sdk::{Client, Error, Reason, reference::artwork_id};
use serde_json::json;
use std::io::{self, IsTerminal, Write};

#[derive(Parser)]
#[command(name = "pixiv", version, about = "Rust migration of pixiv-cli")]
struct Arguments {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Detail {
        source: String,
        #[arg(long, short = 'j', conflicts_with = "ndjson")]
        json: bool,
        #[arg(long)]
        ndjson: bool,
    },
    Search {
        query: String,
        #[arg(long, conflicts_with = "ndjson")]
        json: bool,
        #[arg(long)]
        ndjson: bool,
    },
    Ugoira {
        source: String,
        #[arg(long)]
        json: bool,
    },
}

#[tokio::main]
async fn main() {
    let args = Arguments::parse();
    let machine_output = match &args.command {
        Command::Detail { json, ndjson, .. } => *json || *ndjson,
        Command::Ugoira { json, .. } => *json,
        Command::Search { json, ndjson, .. } => *json || *ndjson,
    };
    match execute(args).await {
        Ok(()) => (),
        Err(error) => {
            if machine_output {
                let mut body = json!({"code": error.code(), "message": error.to_string()});
                if let Some(seconds) = error
                    .sdk_error()
                    .and_then(|error| {
                        error.retry_after_seconds_at(std::time::SystemTime::now().into())
                    })
                    .filter(|seconds| *seconds > 0)
                {
                    body["retry_after_seconds"] = json!(seconds);
                }
                eprintln!("{}", json!({"error": body}));
            } else {
                eprintln!("error: {error}");
            }
            std::process::exit(1);
        }
    }
}

async fn execute(args: Arguments) -> Result<(), CommandError> {
    let detail_id = match &args.command {
        Command::Detail { source, .. } => Some(detail_artwork_id(source)?),
        _ => None,
    };
    let token = std::env::var("PIXIV_ACCESS_TOKEN").unwrap_or_default();
    let proxy = std::env::var("https_proxy")
        .or_else(|_| std::env::var("HTTPS_PROXY"))
        .ok();
    let client = Client::new(&token, proxy.as_deref())?;
    match args.command {
        Command::Detail { json, ndjson, .. } => {
            let mode = if ndjson {
                DetailOutput::Ndjson
            } else if json {
                DetailOutput::Json
            } else {
                DetailOutput::Human
            };
            artwork_detail(
                &client,
                detail_id.expect("detail input was resolved"),
                mode,
                &mut io::stdout().lock(),
            )
            .await?;
        }
        Command::Search {
            query,
            json,
            ndjson,
        } => {
            let artworks = client.search_artworks(&query).await?;
            if json {
                let dtos: Vec<_> = artworks
                    .iter()
                    .map(pixiv_sdk::dto::ArtworkDto::from)
                    .collect();
                output(&serde_json::to_string_pretty(&dtos).map_err(|_| local())?)?;
            } else {
                for artwork in artworks {
                    if ndjson || !io::stdout().is_terminal() {
                        output(
                            &serde_json::to_string(&pixiv_sdk::dto::ArtworkDto::from(&artwork))
                                .map_err(|_| local())?,
                        )?;
                    } else {
                        output(&format!(
                            "{} {} — {}",
                            artwork.id, artwork.title, artwork.user.name
                        ))?;
                    }
                }
            }
        }
        Command::Ugoira { source, json } => {
            let metadata = client.ugoira_metadata(artwork_id(&source)?).await?;
            if json {
                output(
                    &serde_json::to_string_pretty(&pixiv_sdk::dto::UgoiraMetadataDto::from(
                        &metadata,
                    ))
                    .map_err(|_| local())?,
                )?;
            } else {
                output(&format!("frames: {}", metadata.frames.len()))?;
                for frame in metadata.frames {
                    output(&format!(
                        "{} {}ms",
                        frame.filename, frame.delay_milliseconds
                    ))?;
                }
            }
        }
    }
    Ok(())
}

fn detail_artwork_id(source: &str) -> pixiv_sdk::Result<i64> {
    if let Ok(id) = source.trim().parse::<i64>()
        && id > 0
    {
        return Ok(id);
    }
    let reference = pixiv_sdk::reference::parse_url(source).map_err(|_| {
        Error::new(Reason::InvalidArgument, "detail")
            .with_detail("argument must be an entity ID or a supported Pixiv URL")
    })?;
    if reference.kind != pixiv_sdk::reference::REFERENCE_KIND_ARTWORK {
        return Err(Error::new(Reason::InvalidArgument, "detail")
            .with_detail("URL does not name a supported Pixiv artwork"));
    }
    Ok(reference.id)
}

fn local() -> Error {
    Error::new(Reason::LocalStateError, "output")
}

fn output(value: &str) -> pixiv_sdk::Result<()> {
    writeln!(io::stdout().lock(), "{value}").map_err(|_| local())
}
