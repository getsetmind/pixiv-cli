use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand};
use pixiv_cli_rs::search::{SearchInput, SearchOptions};
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
        proxy_override(self.proxy.as_deref(), self.no_proxy)
    }
}
fn proxy_override(
    proxy: Option<&str>,
    no_proxy: Option<bool>,
) -> Result<Option<&str>, CommandError> {
    if proxy.is_some() && no_proxy.is_some() {
        return Err(CommandError::Message(
            "use either --proxy or --no-proxy, not both",
        ));
    }
    if no_proxy == Some(true) {
        Ok(Some(""))
    } else {
        Ok(proxy)
    }
}

#[derive(Subcommand)]
enum NovelCommand {
    #[command(args_override_self = true)]
    Search {
        #[command(flatten)]
        options: Box<pixiv_cli_rs::novel_search::NovelSearchOptions>,
        #[command(flatten)]
        connection: ProxyOptions,
    },
}

#[derive(Subcommand)]
enum UserCommand {
    #[command(args_override_self = true)]
    Artworks {
        #[command(flatten)]
        options: Box<pixiv_cli_rs::user_works::UserArtworksOptions>,
        #[command(flatten)]
        connection: ProxyOptions,
    },
    #[command(args_override_self = true)]
    Novels {
        #[command(flatten)]
        options: Box<pixiv_cli_rs::user_works::UserWorksOptions>,
        #[command(flatten)]
        connection: ProxyOptions,
    },
    #[command(args_override_self = true)]
    Following {
        #[command(flatten)]
        options: Box<pixiv_cli_rs::user_relationships::UserFollowingOptions>,
        #[command(flatten)]
        connection: ProxyOptions,
    },
    #[command(args_override_self = true)]
    Followers {
        #[command(flatten)]
        options: Box<pixiv_cli_rs::user_relationships::UserFollowingOptions>,
        #[command(flatten)]
        connection: ProxyOptions,
    },
    #[command(args_override_self = true)]
    Related {
        #[command(flatten)]
        options: Box<pixiv_cli_rs::user_works::UserWorksOptions>,
        #[command(flatten)]
        connection: ProxyOptions,
    },
    #[command(args_override_self = true)]
    Blocked {
        #[command(flatten)]
        options: Box<pixiv_cli_rs::user_works::UserWorksOptions>,
        #[command(flatten)]
        connection: ProxyOptions,
    },
    Follow {
        #[command(subcommand)]
        command: pixiv_cli_rs::mutation::FollowCommand,
    },
}

#[derive(Subcommand)]
enum Command {
    Bookmark {
        #[command(subcommand)]
        command: pixiv_cli_rs::mutation::BookmarkCommand,
    },
    Follow {
        #[command(subcommand)]
        command: pixiv_cli_rs::mutation::FollowCommand,
    },
    User {
        #[command(subcommand)]
        command: UserCommand,
    },
    Novel {
        #[command(subcommand)]
        command: NovelCommand,
    },
    #[command(args_override_self = true)]
    Mcp {
        #[command(flatten)]
        connection: ProxyOptions,
    },
    #[command(args_override_self = true)]
    Detail {
        source: String,
        #[arg(long = "type", short = 't', default_value = "artwork")]
        entity: String,
        #[arg(long,action=clap::ArgAction::Set,num_args=0..=1,require_equals=true,default_missing_value="true",default_value="false")]
        content: bool,
        #[command(flatten)]
        connection: ProxyOptions,
        #[arg(long, short = 'j', conflicts_with = "ndjson")]
        json: bool,
        #[arg(long)]
        ndjson: bool,
    },
    #[command(args_override_self = true)]
    Search {
        #[command(flatten)]
        input: SearchInput,
        #[command(flatten)]
        connection: ProxyOptions,
        #[command(flatten)]
        options: Box<SearchOptions>,
    },
    #[command(args_override_self = true)]
    Ranking {
        #[command(flatten)]
        options: Box<pixiv_cli_rs::ranking::RankingOptions>,
        #[command(flatten)]
        connection: ProxyOptions,
    },
    #[command(args_override_self = true)]
    Series {
        #[command(flatten)]
        options: Box<pixiv_cli_rs::novel_series::NovelSeriesOptions>,
        #[command(flatten)]
        connection: ProxyOptions,
    },
    #[command(args_override_self = true)]
    Recommended {
        #[command(flatten)]
        options: Box<pixiv_cli_rs::recommended::RecommendedOptions>,
        #[command(flatten)]
        connection: ProxyOptions,
    },
    Ugoira {
        source: String,
        #[arg(long)]
        json: bool,
    },
}

#[tokio::main]
async fn main() {
    let matches = Arguments::command()
        .try_get_matches()
        .unwrap_or_else(|error| {
            let mutation_route = matches!(
                std::env::args().nth(1).as_deref(),
                Some("bookmark" | "follow" | "user")
            );
            let command_error = if mutation_route {
                pixiv_cli_rs::mutation::argument_error(&error)
            } else {
                pixiv_cli_rs::argument_error(&error)
            };
            if let Some(error) = command_error {
                std::process::exit(finish_command(
                    Err(error),
                    false,
                    false,
                    &mut io::stderr().lock(),
                ));
            }
            error.exit()
        });
    let mut args = Arguments::from_arg_matches(&matches).unwrap_or_else(|error| error.exit());
    if let Command::Search { input, .. } = &mut args.command {
        input.record_flag_presence(
            matches
                .subcommand_matches("search")
                .expect("search was parsed"),
        );
    }
    args.command = match args.command {
        Command::Novel {
            command:
                NovelCommand::Search {
                    options,
                    connection,
                },
        } => {
            let (input, options) = options.into_search();
            Command::Search {
                input,
                options: Box::new(options),
                connection,
            }
        }
        command => command,
    };
    let machine_output = match &args.command {
        Command::User {
            command: UserCommand::Artworks { options, .. },
        } => options.listing.json.is_some() || options.listing.ndjson,
        Command::User {
            command: UserCommand::Novels { options, .. },
        } => options.json.is_some() || options.ndjson,
        Command::User {
            command: UserCommand::Following { options, .. } | UserCommand::Followers { options, .. },
        } => options.listing.json.is_some() || options.listing.ndjson,
        Command::User {
            command: UserCommand::Related { options, .. } | UserCommand::Blocked { options, .. },
        } => options.json.is_some() || options.ndjson,
        Command::Novel { .. } => unreachable!("novel commands were resolved"),
        Command::Mcp { .. }
        | Command::Bookmark { .. }
        | Command::Follow { .. }
        | Command::User { .. } => false,
        Command::Detail { json, ndjson, .. } => *json || *ndjson,
        Command::Ugoira { json, .. } => *json,
        Command::Search { input, .. } => input.machine_output(),
        Command::Recommended { options, .. } => options.json.is_some() || options.ndjson,
        Command::Ranking { options, .. } => options.json.is_some() || options.ndjson,
        Command::Series { options, .. } => options.json.is_some() || options.ndjson,
    };
    let mut ndjson_output = match &args.command {
        Command::User {
            command: UserCommand::Artworks { options, .. },
        } => {
            options.listing.ndjson
                || (options.listing.json.is_none() && !io::stdout().is_terminal())
        }
        Command::User {
            command: UserCommand::Novels { options, .. },
        } => options.ndjson || (options.json.is_none() && !io::stdout().is_terminal()),
        Command::User {
            command: UserCommand::Following { options, .. },
        } => options.listing.ndjson,
        Command::User {
            command: UserCommand::Followers { options, .. },
        } => {
            options.listing.ndjson
                || (options.listing.json.is_none() && !io::stdout().is_terminal())
        }
        Command::User {
            command: UserCommand::Related { options, .. } | UserCommand::Blocked { options, .. },
        } => options.ndjson || (options.json.is_none() && !io::stdout().is_terminal()),
        Command::Novel { .. } => unreachable!("novel commands were resolved"),
        Command::Mcp { .. }
        | Command::Bookmark { .. }
        | Command::Follow { .. }
        | Command::User { .. } => false,
        Command::Detail { ndjson, .. } => *ndjson,
        Command::Search { input, .. } => {
            input.ndjson || (input.json.is_none() && !io::stdout().is_terminal())
        }
        Command::Ugoira { .. } => false,
        Command::Recommended { options, .. } => {
            options.ndjson || (options.json.is_none() && !io::stdout().is_terminal())
        }
        Command::Ranking { options, .. } => {
            options.ndjson || (options.json.is_none() && !io::stdout().is_terminal())
        }
        Command::Series { options, .. } => {
            options.ndjson || (options.json.is_none() && !io::stdout().is_terminal())
        }
    };
    let exit = finish_command(
        execute(args, &mut ndjson_output).await,
        ndjson_output,
        machine_output,
        &mut io::stderr().lock(),
    );
    if exit != 0 {
        std::process::exit(exit);
    }
}

async fn execute(mut args: Arguments, ndjson_output: &mut bool) -> Result<(), CommandError> {
    let mut user_works = match &args.command {
        Command::User {
            command: UserCommand::Artworks { options, .. },
        } => Some(pixiv_cli_rs::user_works::UserWorks::Artworks(
            options.as_ref().clone(),
        )),
        Command::User {
            command: UserCommand::Novels { options, .. },
        } => Some(pixiv_cli_rs::user_works::UserWorks::Novels(
            options.as_ref().clone(),
        )),
        _ => None,
    };
    if let Some(options) = &mut user_works {
        options.resolve_source(&mut io::stdin().lock(), io::stdin().is_terminal())?;
    }
    let mut user_relationships = match &args.command {
        Command::User {
            command: UserCommand::Following { options, .. },
        } => Some(
            pixiv_cli_rs::user_relationships::UserRelationships::Following(
                options.as_ref().clone(),
            ),
        ),
        Command::User {
            command: UserCommand::Followers { options, .. },
        } => Some(
            pixiv_cli_rs::user_relationships::UserRelationships::Followers(
                options.as_ref().clone(),
            ),
        ),
        Command::User {
            command: UserCommand::Related { options, .. },
        } => Some(
            pixiv_cli_rs::user_relationships::UserRelationships::Related(options.as_ref().clone()),
        ),
        Command::User {
            command: UserCommand::Blocked { options, .. },
        } => Some(
            pixiv_cli_rs::user_relationships::UserRelationships::Blocked(options.as_ref().clone()),
        ),
        _ => None,
    };
    if let Some(options) = &mut user_relationships {
        options.resolve_source(&mut io::stdin().lock(), io::stdin().is_terminal())?;
    }
    if let Command::Recommended { options, .. } = &mut args.command {
        options.resolve_source(&mut io::stdin().lock(), io::stdin().is_terminal())?;
    }
    if let Command::Ranking { options, .. } = &args.command {
        options.validate_arguments()?;
    }
    if let Command::Series { options, .. } = &mut args.command {
        options.resolve_source(&mut io::stdin().lock(), io::stdin().is_terminal())?;
    }
    let search_word = match &args.command {
        Command::Search { input, .. } => {
            if input.trending_tags {
                input.validate_trending_arguments()?;
                None
            } else {
                Some(input.resolve_word(&mut io::stdin().lock(), io::stdin().is_terminal())?)
            }
        }
        _ => None,
    };
    let account_config = if matches!(
        &args.command,
        Command::Detail { .. }
            | Command::Mcp { .. }
            | Command::Search { .. }
            | Command::Recommended { .. }
            | Command::Ranking { .. }
            | Command::Series { .. }
            | Command::Bookmark { .. }
            | Command::Follow { .. }
            | Command::User { .. }
    ) {
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
    if let Some(options) = user_works {
        let (directory, config) = account_config.expect("user works startup was resolved");
        options.validate()?;
        let connection = match &args.command {
            Command::User {
                command:
                    UserCommand::Artworks { connection, .. } | UserCommand::Novels { connection, .. },
            } => connection,
            _ => unreachable!("user works route was resolved"),
        };
        let proxy = connection.override_value()?;
        let configured_json = if options.options().ndjson {
            options.output_mode(false, true)?;
            false
        } else {
            config
                .current()
                .and_then(|snapshot| snapshot.runtime())
                .map_err(pixiv_app::scheduler::SchedulerError::from)?
                .output_json
        };
        let mode = options.output_mode(configured_json, io::stdout().is_terminal())?;
        *ndjson_output = mode == DetailOutput::Ndjson;
        let database = pixiv_app::database::Database::open(&directory)
            .map_err(|error| CommandError::State(Box::new(error)))?;
        let execution = pixiv_app::execution::Execution::http(
            config,
            std::sync::Arc::new(std::sync::Mutex::new(database)),
        );
        return pixiv_cli_rs::user_works::saved_user_works(
            &execution,
            &pixiv_app::lifecycle::Context::new(),
            options,
            proxy,
            mode,
            io::stdout(),
        )
        .await;
    }
    if let Some(options) = user_relationships {
        let (directory, config) = account_config.expect("user relationships startup was resolved");
        options.validate()?;
        let connection = match &args.command {
            Command::User {
                command:
                    UserCommand::Following { connection, .. }
                    | UserCommand::Followers { connection, .. }
                    | UserCommand::Related { connection, .. }
                    | UserCommand::Blocked { connection, .. },
            } => connection,
            _ => unreachable!("user relationships route was resolved"),
        };
        let proxy = connection.override_value()?;
        let configured_json = if options.options().ndjson {
            options.output_mode(false, true)?;
            false
        } else {
            config
                .current()
                .and_then(|snapshot| snapshot.runtime())
                .map_err(pixiv_app::scheduler::SchedulerError::from)?
                .output_json
        };
        let mode = options.output_mode(configured_json, io::stdout().is_terminal())?;
        *ndjson_output = mode == DetailOutput::Ndjson;
        let database = pixiv_app::database::Database::open(&directory)
            .map_err(|error| CommandError::State(Box::new(error)))?;
        let execution = pixiv_app::execution::Execution::http(
            config,
            std::sync::Arc::new(std::sync::Mutex::new(database)),
        );
        return pixiv_cli_rs::user_relationships::saved_user_relationships(
            &execution,
            &pixiv_app::lifecycle::Context::new(),
            options,
            proxy,
            mode,
            io::stdout(),
        )
        .await;
    }
    let mutation = match &args.command {
        Command::Bookmark { command } => {
            Some(pixiv_cli_rs::mutation::Mutation::Bookmark(command.clone()))
        }
        Command::Follow { command }
        | Command::User {
            command: UserCommand::Follow { command },
        } => Some(pixiv_cli_rs::mutation::Mutation::Follow(command.clone())),
        _ => None,
    };
    if let Some(action) = mutation {
        action.validate(io::stdin().is_terminal())?;
        let (directory, config) = account_config.expect("mutation startup was resolved");
        let mut execution = None;
        return pixiv_cli_rs::mutation::saved_mutation_with_factory(
            &pixiv_app::lifecycle::Context::new(),
            action,
            &mut io::stdin().lock(),
            io::stdin().is_terminal(),
            &mut io::stderr().lock(),
            || {
                if let Some(execution) = &execution {
                    return Ok(std::sync::Arc::clone(execution));
                }
                let database = pixiv_app::database::Database::open(&directory)
                    .map_err(|error| CommandError::State(Box::new(error)))?;
                let opened = std::sync::Arc::new(pixiv_app::execution::Execution::http(
                    config.clone(),
                    std::sync::Arc::new(std::sync::Mutex::new(database)),
                ));
                execution = Some(opened.clone());
                Ok(opened)
            },
        )
        .await;
    }

    let detail_id = match &args.command {
        Command::Detail {
            source,
            entity,
            content,
            ..
        } => {
            let entity = match entity.as_str() {
                "artwork" | "illust" | "manga" | "ugoira" => "artwork",
                "novel" => "novel",
                "user" => "user",
                _ => {
                    return Err(CommandError::Message(
                        "type must be one of artwork, novel, user",
                    ));
                }
            };
            if *content {
                if entity != "novel" {
                    return Err(CommandError::Message(
                        "--content is only supported when --type novel",
                    ));
                }
                return Err(Error::new(Reason::ContentUnavailable, "NovelContent")
                    .with_detail("novel content is unsupported by the v1 App API")
                    .into());
            }
            Some(if entity == "user" {
                pixiv_cli_rs::detail_user_id(source)?
            } else if entity == "novel" {
                pixiv_cli_rs::detail_novel_id(source)?
            } else {
                detail_artwork_id(source)?
            })
        }
        _ => None,
    };
    let novel_search_request = match &args.command {
        Command::Search { input, options, .. }
            if !input.trending_tags && input.entity.as_deref() == Some("novel") =>
        {
            Some(input.novel_request(
                options,
                search_word.as_deref().expect("search input was resolved"),
            )?)
        }
        _ => None,
    };
    let user_search_request = match &args.command {
        Command::Search { input, options, .. }
            if !input.trending_tags && input.entity.as_deref() == Some("user") =>
        {
            Some(input.user_request(
                options,
                search_word.as_deref().expect("search input was resolved"),
            )?)
        }
        _ => None,
    };
    let search_request = match &args.command {
        Command::Search { input, options, .. }
            if !input.trending_tags
                && novel_search_request.is_none()
                && user_search_request.is_none() =>
        {
            if input
                .entity
                .as_deref()
                .is_some_and(|entity| entity != "artwork")
            {
                return Err(CommandError::Message(
                    "type must be one of artwork, novel, user",
                ));
            }
            Some(options.request(
                search_word.as_deref().expect("search input was resolved"),
                chrono::Utc::now().fixed_offset(),
            )?)
        }
        _ => None,
    };
    if let Command::Search {
        options,
        connection,
        input,
        ..
    } = &args.command
    {
        let (directory, config) = account_config.expect("search startup was resolved");
        if input.trending_tags {
            input.validate_trending_flags()?;
        }
        let proxy = connection.override_value()?;
        let configured_json = config
            .current()
            .and_then(|snapshot| snapshot.runtime())
            .map_err(pixiv_app::scheduler::SchedulerError::from)?
            .output_json;
        let mode = if input.trending_tags {
            if input.json.unwrap_or(configured_json) {
                DetailOutput::Json
            } else {
                DetailOutput::Human
            }
        } else {
            input.output_mode(configured_json, io::stdout().is_terminal())?
        };
        *ndjson_output = mode == DetailOutput::Ndjson;
        if !input.trending_tags {
            options.validate_bookmark_strategy()?;
        }
        let database = pixiv_app::database::Database::open(&directory)
            .map_err(|error| CommandError::State(Box::new(error)))?;
        let execution = pixiv_app::execution::Execution::http(
            config,
            std::sync::Arc::new(std::sync::Mutex::new(database)),
        );
        if input.trending_tags {
            return pixiv_cli_rs::trending::saved_trending_tags(
                &execution,
                &pixiv_app::lifecycle::Context::new(),
                proxy,
                mode == DetailOutput::Json,
                &mut io::stdout().lock(),
            )
            .await;
        }
        if novel_search_request.is_some() {
            return pixiv_cli_rs::novel_search::saved_novel_search(
                &execution,
                &pixiv_app::lifecycle::Context::new(),
                (
                    input,
                    options.as_ref(),
                    search_word.as_deref().expect("search input was resolved"),
                ),
                proxy,
                mode,
                io::stdout(),
            )
            .await;
        }
        if user_search_request.is_some() {
            return pixiv_cli_rs::user_search::saved_user_search(
                &execution,
                &pixiv_app::lifecycle::Context::new(),
                (
                    input,
                    options.as_ref(),
                    search_word.as_deref().expect("search input was resolved"),
                ),
                proxy,
                mode,
                io::stdout(),
            )
            .await;
        }
        return pixiv_cli_rs::search::saved_artwork_search(
            &execution,
            &pixiv_app::lifecycle::Context::new(),
            search_request.expect("search options were resolved"),
            options.as_ref().clone(),
            proxy,
            mode,
            io::stdout(),
        )
        .await;
    }
    if let Command::Recommended {
        options,
        connection,
    } = &args.command
    {
        let (directory, config) = account_config.expect("recommended startup was resolved");
        options.validate()?;
        let proxy = connection.override_value()?;
        let configured_json = if options.ndjson {
            options.output_mode(false, true)?;
            false
        } else {
            config
                .current()
                .and_then(|snapshot| snapshot.runtime())
                .map_err(pixiv_app::scheduler::SchedulerError::from)?
                .output_json
        };
        let mode = options.output_mode(configured_json, io::stdout().is_terminal())?;
        *ndjson_output = mode == DetailOutput::Ndjson;
        let database = pixiv_app::database::Database::open(&directory)
            .map_err(|error| CommandError::State(Box::new(error)))?;
        let execution = pixiv_app::execution::Execution::http(
            config,
            std::sync::Arc::new(std::sync::Mutex::new(database)),
        );
        return pixiv_cli_rs::recommended::saved_recommended(
            &execution,
            &pixiv_app::lifecycle::Context::new(),
            options.as_ref().clone(),
            proxy,
            mode,
            io::stdout(),
        )
        .await;
    }
    if let Command::Ranking {
        options,
        connection,
    } = &args.command
    {
        let (directory, config) = account_config.expect("ranking startup was resolved");
        options.validate()?;
        let proxy = connection.override_value()?;
        let configured_json = config
            .current()
            .and_then(|snapshot| snapshot.runtime())
            .map_err(pixiv_app::scheduler::SchedulerError::from)?
            .output_json;
        let mode = options.output_mode(configured_json, io::stdout().is_terminal())?;
        *ndjson_output = mode == DetailOutput::Ndjson;
        let database = pixiv_app::database::Database::open(&directory)
            .map_err(|error| CommandError::State(Box::new(error)))?;
        let execution = pixiv_app::execution::Execution::http(
            config,
            std::sync::Arc::new(std::sync::Mutex::new(database)),
        );
        return pixiv_cli_rs::ranking::saved_ranking(
            &execution,
            &pixiv_app::lifecycle::Context::new(),
            options.as_ref().clone(),
            proxy,
            mode,
            io::stdout(),
        )
        .await;
    }
    if let Command::Series {
        options,
        connection,
    } = &args.command
    {
        let (directory, config) = account_config.expect("series startup was resolved");
        options.validate()?;
        let proxy = connection.override_value()?;
        let configured_json = config
            .current()
            .and_then(|snapshot| snapshot.runtime())
            .map_err(pixiv_app::scheduler::SchedulerError::from)?
            .output_json;
        let mode = options.output_mode(configured_json, io::stdout().is_terminal())?;
        *ndjson_output = mode == DetailOutput::Ndjson;
        let database = pixiv_app::database::Database::open(&directory)
            .map_err(|error| CommandError::State(Box::new(error)))?;
        let execution = pixiv_app::execution::Execution::http(
            config,
            std::sync::Arc::new(std::sync::Mutex::new(database)),
        );
        return pixiv_cli_rs::novel_series::saved_novel_series(
            &execution,
            &pixiv_app::lifecycle::Context::new(),
            options.as_ref().clone(),
            proxy,
            mode,
            io::stdout(),
        )
        .await;
    }
    if let Command::Detail {
        json,
        ndjson,
        connection,
        entity,
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
        if entity == "user" {
            return pixiv_cli_rs::saved_user_detail(
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
        if entity == "novel" {
            return pixiv_cli_rs::saved_novel_detail(
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
    if let Command::Mcp { connection } = &args.command {
        let (directory, config) = account_config.expect("MCP startup was resolved");
        let proxy = connection.override_value()?;
        if proxy.is_some() {
            let runtime = config
                .current()
                .and_then(|snapshot| snapshot.runtime())
                .map_err(pixiv_app::scheduler::SchedulerError::from)?;
            pixiv_app::connection::CommandConnection::resolve(&runtime, proxy)
                .map_err(pixiv_app::scheduler::SchedulerError::Proxy)?;
        }
        let database = pixiv_app::database::Database::open(&directory)
            .map_err(|error| CommandError::State(Box::new(error)))?;
        let execution = pixiv_app::execution::Execution::http(
            config,
            std::sync::Arc::new(std::sync::Mutex::new(database)),
        );
        pixiv_mcp::stdio::serve_saved_with_proxy(
            &execution,
            proxy,
            tokio::io::stdin(),
            &mut tokio::io::stdout(),
        )
        .await?;
        return Ok(());
    }
    let token = std::env::var("PIXIV_ACCESS_TOKEN").unwrap_or_default();
    let proxy = std::env::var("https_proxy")
        .or_else(|_| std::env::var("HTTPS_PROXY"))
        .ok();
    let client = Client::new(&token, proxy.as_deref())?;
    match args.command {
        Command::Novel { .. } => unreachable!("novel commands were resolved"),
        Command::Mcp { .. } => unreachable!("MCP uses saved account execution"),
        Command::Bookmark { .. } | Command::Follow { .. } | Command::User { .. } => {
            unreachable!("mutations use saved account execution")
        }
        Command::Detail { .. } => unreachable!("detail uses saved account execution"),
        Command::Search { .. } => unreachable!("search uses saved account execution"),
        Command::Recommended { .. } => unreachable!("recommended uses saved account execution"),
        Command::Ranking { .. } => unreachable!("ranking uses saved account execution"),
        Command::Series { .. } => unreachable!("series uses saved account execution"),
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
