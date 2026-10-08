use clap::{Args, Parser, Subcommand};
use pixiv_cli_rs::search::SearchOptions;
use pixiv_cli_rs::{
    CommandError, DetailOutput, detail_artwork_id, finish_command, saved_artwork_detail,
};
use pixiv_sdk::{Client, Error, Reason, reference::artwork_id};
use std::io::{self, IsTerminal, Write};

#[derive(Parser)]
#[command(name = "pixiv", version, about = "Rust migration of pixiv-cli")]
struct Arguments {
    #[command(subcommand)]
    command: Command,
}

#[derive(Args)]
struct ProxyOptions {
    #[arg(long)]
    proxy: Option<String>,
    #[arg(long, num_args = 0..=1, require_equals = true, default_missing_value = "true")]
    no_proxy: Option<bool>,
}

impl ProxyOptions {
    fn override_value(&self) -> Result<Option<&str>, CommandError> {
        if self.proxy.is_some() && self.no_proxy.is_some() {
            return Err(CommandError::Message(
                "use either --proxy or --no-proxy, not both",
            ));
        }
        if self.no_proxy == Some(true) {
            Ok(Some(""))
        } else {
            Ok(self.proxy.as_deref())
        }
    }
}

#[derive(Subcommand)]
enum Command {
    Mcp,
    #[command(args_override_self = true)]
    Detail {
        source: String,
        #[command(flatten)]
        connection: ProxyOptions,
        #[arg(long, short = 'j', conflicts_with = "ndjson")]
        json: bool,
        #[arg(long)]
        ndjson: bool,
    },
    Search {
        query: String,
        #[command(flatten)]
        options: Box<SearchOptions>,
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
        Command::Mcp => false,
        Command::Detail { json, ndjson, .. } => *json || *ndjson,
        Command::Ugoira { json, .. } => *json,
        Command::Search { json, ndjson, .. } => *json || *ndjson,
    };
    let ndjson_output = match &args.command {
        Command::Mcp => false,
        Command::Detail { ndjson, .. } => *ndjson,
        Command::Search { json, ndjson, .. } => *ndjson || (!*json && !io::stdout().is_terminal()),
        Command::Ugoira { .. } => false,
    };
    let exit = finish_command(
        execute(args).await,
        ndjson_output,
        machine_output,
        &mut io::stderr().lock(),
    );
    if exit != 0 {
        std::process::exit(exit);
    }
}

async fn execute(args: Arguments) -> Result<(), CommandError> {
    let account_config = if matches!(&args.command, Command::Detail { .. } | Command::Mcp) {
        let home_name = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
        let home = std::env::var_os(home_name)
            .filter(|home| !home.is_empty())
            .ok_or_else(|| {
                let variable = if cfg!(windows) {
                    "%USERPROFILE%"
                } else {
                    "$HOME"
                };
                CommandError::MessageText(format!("{variable} is not defined"))
            })?;
        let directory = std::path::PathBuf::from(home).join(".pixiv-cli");
        let config = pixiv_app::config::Store::new(directory.join("config.toml"));
        config
            .ensure_defaults()
            .map_err(pixiv_app::scheduler::SchedulerError::from)?;
        config
            .current()
            .and_then(|snapshot| snapshot.runtime())
            .map_err(pixiv_app::scheduler::SchedulerError::from)?;
        Some((directory, config))
    } else {
        None
    };
    let detail_id = match &args.command {
        Command::Detail { source, .. } => Some(detail_artwork_id(source)?),
        _ => None,
    };
    let search_request = match &args.command {
        Command::Search { query, options, .. } => {
            Some(options.request(query, chrono::Utc::now().fixed_offset())?)
        }
        _ => None,
    };
    if let Command::Detail {
        json,
        ndjson,
        connection,
        ..
    } = &args.command
    {
        let (directory, config) = account_config.expect("detail startup was resolved");
        let json_output = if *json {
            true
        } else {
            config
                .current()
                .and_then(|snapshot| snapshot.runtime())
                .map_err(pixiv_app::scheduler::SchedulerError::from)?
                .output_json
        };
        let database = pixiv_app::database::Database::open(&directory)
            .map_err(|error| CommandError::State(Box::new(error)))?;
        let execution = pixiv_app::execution::Execution::http(
            config,
            std::sync::Arc::new(std::sync::Mutex::new(database)),
        );
        let mode = if *ndjson {
            DetailOutput::Ndjson
        } else if json_output {
            DetailOutput::Json
        } else {
            DetailOutput::Human
        };
        return saved_artwork_detail(
            &execution,
            &pixiv_app::lifecycle::Context::new(),
            detail_id.expect("detail input was resolved"),
            0,
            connection.override_value()?,
            mode,
            &mut io::stdout().lock(),
        )
        .await;
    }
    if matches!(&args.command, Command::Mcp) {
        let (directory, config) = account_config.expect("MCP startup was resolved");
        let database = pixiv_app::database::Database::open(&directory)
            .map_err(|error| CommandError::State(Box::new(error)))?;
        let execution = pixiv_app::execution::Execution::http(
            config,
            std::sync::Arc::new(std::sync::Mutex::new(database)),
        );
        pixiv_mcp::stdio::serve_saved(&execution, tokio::io::stdin(), &mut tokio::io::stdout())
            .await?;
        return Ok(());
    }
    let token = std::env::var("PIXIV_ACCESS_TOKEN").unwrap_or_default();
    let proxy = std::env::var("https_proxy")
        .or_else(|_| std::env::var("HTTPS_PROXY"))
        .ok();
    let client = Client::new(&token, proxy.as_deref())?;
    match args.command {
        Command::Mcp => unreachable!("MCP uses saved account execution"),
        Command::Detail { .. } => unreachable!("detail uses saved account execution"),
        Command::Search {
            options,
            json,
            ndjson,
            ..
        } => {
            let mode = if json {
                DetailOutput::Json
            } else if ndjson || !io::stdout().is_terminal() {
                DetailOutput::Ndjson
            } else {
                DetailOutput::Human
            };
            pixiv_cli_rs::search::artwork_search(
                &client,
                search_request.expect("search options were resolved"),
                &options,
                mode,
                &mut io::stdout().lock(),
            )
            .await?;
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

fn local() -> Error {
    Error::new(Reason::LocalStateError, "output")
}

fn output(value: &str) -> Result<(), CommandError> {
    writeln!(io::stdout().lock(), "{value}").map_err(CommandError::from)
}
