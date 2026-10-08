use clap::{Parser, Subcommand};
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
        #[arg(long)]
        json: bool,
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
        Command::Detail { json, .. } | Command::Ugoira { json, .. } => *json,
        Command::Search { json, ndjson, .. } => *json || *ndjson,
    };
    match execute(args).await {
        Ok(()) => (),
        Err(error) => {
            if machine_output {
                eprintln!("{}", json!({"error": error}));
            } else {
                eprintln!("error: {error}");
            }
            std::process::exit(if error.code == Reason::InvalidArgument {
                2
            } else {
                1
            });
        }
    }
}

async fn execute(args: Arguments) -> pixiv_sdk::Result<()> {
    let token = std::env::var("PIXIV_ACCESS_TOKEN").unwrap_or_default();
    let proxy = std::env::var("https_proxy")
        .or_else(|_| std::env::var("HTTPS_PROXY"))
        .ok();
    let client = Client::new(&token, proxy.as_deref())?;
    match args.command {
        Command::Detail { source, json } => {
            let artwork = client.artwork(artwork_id(&source)?).await?;
            if json {
                output(&serde_json::to_string_pretty(&artwork).map_err(|_| local())?)?;
            } else {
                output(&format!(
                    "{} {} — {}",
                    artwork.id, artwork.title, artwork.user.name
                ))?;
            }
        }
        Command::Search {
            query,
            json,
            ndjson,
        } => {
            let artworks = client.search_artworks(&query).await?;
            if json {
                output(&serde_json::to_string_pretty(&artworks).map_err(|_| local())?)?;
            } else {
                for artwork in artworks {
                    if ndjson || !io::stdout().is_terminal() {
                        output(&serde_json::to_string(&artwork).map_err(|_| local())?)?;
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
                output(&serde_json::to_string_pretty(&metadata).map_err(|_| local())?)?;
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

fn local() -> Error {
    Error::new(Reason::LocalStateError, "output")
}

fn output(value: &str) -> pixiv_sdk::Result<()> {
    writeln!(io::stdout().lock(), "{value}").map_err(|_| local())
}
