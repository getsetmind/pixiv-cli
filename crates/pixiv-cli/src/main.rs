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
enum BookmarkGroupCommand {
    #[command(args_override_self = true)]
    Detail {
        #[command(flatten)]
        options: Box<pixiv_cli_rs::bookmark_reads::BookmarkDetailOptions>,
        #[command(flatten)]
        connection: ProxyOptions,
    },
    #[command(args_override_self = true)]
    Tags {
        #[command(flatten)]
        options: Box<pixiv_cli_rs::bookmark_reads::BookmarkTagsOptions>,
        #[command(flatten)]
        connection: ProxyOptions,
    },
    #[command(args_override_self = true)]
    List {
        #[command(flatten)]
        options: Box<pixiv_cli_rs::bookmark_lists::BookmarkListOptions>,
        #[command(flatten)]
        connection: ProxyOptions,
    },
    #[command(flatten)]
    Mutation(pixiv_cli_rs::mutation::BookmarkCommand),
}

#[derive(Subcommand)]
enum UserCommand {
    #[command(args_override_self = true)]
    Detail {
        #[command(flatten)]
        options: pixiv_cli_rs::user_detail::UserDetailOptions,
        #[command(flatten)]
        connection: ProxyOptions,
    },
    #[command(args_override_self = true)]
    Search {
        #[command(flatten)]
        options: Box<pixiv_cli_rs::user_search::UserSearchOptions>,
        #[command(flatten)]
        connection: ProxyOptions,
    },
    #[command(args_override_self = true)]
    Bookmarks {
        #[command(flatten)]
        options: Box<pixiv_cli_rs::bookmark_lists::UserBookmarksOptions>,
        #[command(flatten)]
        connection: ProxyOptions,
    },
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
enum MyPixivCommand {
    #[command(args_override_self = true)]
    Users {
        #[command(flatten)]
        options: Box<pixiv_cli_rs::user_works::UserWorksOptions>,
        #[command(flatten)]
        connection: ProxyOptions,
    },
    #[command(args_override_self = true)]
    Works {
        #[command(flatten)]
        options: Box<pixiv_cli_rs::mypixiv::MyPixivWorksOptions>,
        #[command(flatten)]
        connection: ProxyOptions,
    },
}

#[derive(Subcommand)]
enum TimelineCommand {
    #[command(args_override_self = true)]
    Following {
        #[command(flatten)]
        options: Box<pixiv_cli_rs::timeline::FollowingOptions>,
        #[command(flatten)]
        connection: ProxyOptions,
    },
    #[command(args_override_self = true)]
    Latest {
        #[command(flatten)]
        options: Box<pixiv_cli_rs::timeline::TimelineOptions>,
        #[command(flatten)]
        connection: ProxyOptions,
    },
}

#[derive(Subcommand)]
enum CommentCommand {
    #[command(flatten)]
    Mutation(pixiv_cli_rs::comment_mutations::CommentMutation),
    #[command(args_override_self = true)]
    Stamps {
        #[command(flatten)]
        options: pixiv_cli_rs::comment_reads::StampsOptions,
        #[command(flatten)]
        connection: ProxyOptions,
    },
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "Manage local Pixiv authentication")]
    Auth,
    #[command(about = "Manage global Pixiv CLI settings")]
    Config,
    #[command(args_override_self = true)]
    Comment {
        #[command(flatten)]
        options: pixiv_cli_rs::comment_reads::CommentOptions,
        #[command(flatten)]
        connection: ProxyOptions,
        #[command(subcommand)]
        command: Option<CommentCommand>,
    },
    Mypixiv {
        #[command(subcommand)]
        command: MyPixivCommand,
    },
    Timeline {
        #[command(subcommand)]
        command: TimelineCommand,
    },
    Bookmark {
        #[command(subcommand)]
        command: BookmarkGroupCommand,
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
    if std::env::args().nth(1).as_deref() == Some("auth") {
        let (result, machine) = execute_auth();
        let exit = finish_command(result, false, machine, &mut io::stderr().lock());
        if exit != 0 {
            std::process::exit(exit);
        }
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("config") {
        let result = execute_config();
        let exit = finish_command(result, false, false, &mut io::stderr().lock());
        if exit != 0 {
            std::process::exit(exit);
        }
        return;
    }
    let matches = Arguments::command()
        .mut_subcommand("mypixiv", |command| {
            command
                .mut_subcommand("users", pixiv_cli_rs::timeline::configure_command)
                .mut_subcommand("works", pixiv_cli_rs::timeline::configure_command)
        })
        .mut_subcommand("timeline", |command| {
            command
                .mut_subcommand("following", pixiv_cli_rs::timeline::configure_command)
                .mut_subcommand("latest", pixiv_cli_rs::timeline::configure_command)
        })
        .try_get_matches()
        .unwrap_or_else(|error| {
            let mutation_route = matches!(
                std::env::args().nth(1).as_deref(),
                Some("bookmark" | "follow" | "user")
            );
            let command_error = if matches!(
                std::env::args().nth(1).as_deref(),
                Some("timeline" | "mypixiv")
            ) {
                pixiv_cli_rs::timeline::argument_error(&error)
            } else if std::env::args().nth(1).as_deref() == Some("comment")
                && matches!(
                    std::env::args().nth(2).as_deref(),
                    Some("create" | "delete" | "reply" | "stamp")
                )
            {
                pixiv_cli_rs::comment_mutations::argument_error(&error)
            } else if mutation_route {
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
        Command::User {
            command:
                UserCommand::Search {
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
        Command::Comment {
            options, command, ..
        } => match command {
            Some(CommentCommand::Mutation(action)) => action.input().json.is_some(),
            Some(CommentCommand::Stamps { options, .. }) => {
                options.json.is_some() || options.ndjson
            }
            None => options.listing.json.is_some() || options.listing.ndjson,
        },
        Command::User {
            command: UserCommand::Detail { options, .. },
        } => options.json.is_some(),
        Command::Mypixiv { command } => {
            let options = match command {
                MyPixivCommand::Users { options, .. } => options.as_ref(),
                MyPixivCommand::Works { options, .. } => &options.listing,
            };
            options.json.is_some() || options.ndjson
        }
        Command::Timeline { command } => {
            let options = match command {
                TimelineCommand::Following { options, .. } => &options.timeline.listing,
                TimelineCommand::Latest { options, .. } => &options.listing,
            };
            options.json.is_some() || options.ndjson
        }
        Command::Bookmark {
            command: BookmarkGroupCommand::Detail { options, .. },
        } => options.json.is_some(),
        Command::Bookmark {
            command: BookmarkGroupCommand::Tags { options, .. },
        } => options.listing.json.is_some() || options.listing.ndjson,
        Command::Bookmark {
            command: BookmarkGroupCommand::List { options, .. },
        } => options.listing.json.is_some() || options.listing.ndjson,
        Command::User {
            command: UserCommand::Bookmarks { options, .. },
        } => options.listing.json.is_some() || options.listing.ndjson,
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
        Command::Auth
        | Command::Config
        | Command::Mcp { .. }
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
        Command::Comment {
            options, command, ..
        } => match command {
            Some(CommentCommand::Mutation(_)) => false,
            Some(CommentCommand::Stamps { options, .. }) => {
                options.ndjson || (options.json.is_none() && !io::stdout().is_terminal())
            }
            None => {
                options.listing.ndjson
                    || (options.listing.json.is_none() && !io::stdout().is_terminal())
            }
        },
        Command::Mypixiv { command } => match command {
            MyPixivCommand::Users { options, .. } => options.ndjson,
            MyPixivCommand::Works { options, .. } => {
                options.listing.ndjson
                    || (options.listing.json.is_none() && !io::stdout().is_terminal())
            }
        },
        Command::Timeline { command } => {
            let options = match command {
                TimelineCommand::Following { options, .. } => &options.timeline.listing,
                TimelineCommand::Latest { options, .. } => &options.listing,
            };
            options.ndjson || (options.json.is_none() && !io::stdout().is_terminal())
        }
        Command::Bookmark {
            command: BookmarkGroupCommand::Tags { options, .. },
        } => options.listing.ndjson,
        Command::Bookmark {
            command: BookmarkGroupCommand::List { options, .. },
        } => options.listing.ndjson,
        Command::User {
            command: UserCommand::Bookmarks { options, .. },
        } => {
            options.listing.ndjson
                || (options.listing.json.is_none() && !io::stdout().is_terminal())
        }
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
        Command::Auth
        | Command::Config
        | Command::Mcp { .. }
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
    if let Command::Comment {
        options, command, ..
    } = &mut args.command
    {
        match command {
            Some(CommentCommand::Mutation(action)) => {
                action.resolve_source(&mut io::stdin().lock(), io::stdin().is_terminal())?
            }
            Some(CommentCommand::Stamps { options, .. }) => options.validate_arguments()?,
            None => options.resolve_source(&mut io::stdin().lock(), io::stdin().is_terminal())?,
        }
    }
    match &mut args.command {
        Command::Bookmark {
            command: BookmarkGroupCommand::Detail { options, .. },
        } => options.resolve_source(&mut io::stdin().lock(), io::stdin().is_terminal())?,
        Command::Bookmark {
            command: BookmarkGroupCommand::Tags { options, .. },
        } => options.resolve_source(&mut io::stdin().lock(), io::stdin().is_terminal())?,
        _ => {}
    }

    let mut bookmark_lists = match &args.command {
        Command::Bookmark {
            command: BookmarkGroupCommand::List { options, .. },
        } => Some(pixiv_cli_rs::bookmark_lists::BookmarkLists::List(
            options.as_ref().clone(),
        )),
        Command::User {
            command: UserCommand::Bookmarks { options, .. },
        } => Some(pixiv_cli_rs::bookmark_lists::BookmarkLists::User(
            options.as_ref().clone(),
        )),
        _ => None,
    };
    if let Some(options) = &mut bookmark_lists {
        options.resolve_source(&mut io::stdin().lock(), io::stdin().is_terminal())?;
    }

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
    let mut mypixiv = match &args.command {
        Command::Mypixiv {
            command: MyPixivCommand::Users { options, .. },
        } => Some(pixiv_cli_rs::mypixiv::MyPixiv::Users(
            options.as_ref().clone(),
        )),
        Command::Mypixiv {
            command: MyPixivCommand::Works { options, .. },
        } => Some(pixiv_cli_rs::mypixiv::MyPixiv::Works(
            options.as_ref().clone(),
        )),
        _ => None,
    };
    if let Some(options) = &mut mypixiv {
        options.resolve_source(&mut io::stdin().lock(), io::stdin().is_terminal())?;
    }
    let timeline = match &args.command {
        Command::Timeline {
            command: TimelineCommand::Following { options, .. },
        } => Some(pixiv_cli_rs::timeline::Timeline::Following(
            options.as_ref().clone(),
        )),
        Command::Timeline {
            command: TimelineCommand::Latest { options, .. },
        } => Some(pixiv_cli_rs::timeline::Timeline::Latest(
            options.as_ref().clone(),
        )),
        _ => None,
    };
    if let Some(options) = &timeline {
        options.validate_arguments()?;
    }
    let user_detail_source = match &args.command {
        Command::User {
            command: UserCommand::Detail { options, .. },
        } => Some(options.resolve_source(&mut io::stdin().lock(), io::stdin().is_terminal())?),
        _ => None,
    };
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
        Command::Mypixiv { .. }
            | Command::Comment { .. }
            | Command::Timeline { .. }
            | Command::Detail { .. }
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

    match &mut args.command {
        Command::User {
            command:
                UserCommand::Detail {
                    options,
                    connection,
                },
        } => {
            let (directory, config) = account_config.expect("user detail startup was resolved");
            let id = pixiv_cli_rs::user_detail::profile_user_id(
                user_detail_source
                    .as_ref()
                    .expect("user detail input was resolved"),
            )?;
            let proxy = connection.override_value()?;
            let configured_json = config
                .current()
                .and_then(|snapshot| snapshot.runtime())
                .map_err(pixiv_app::scheduler::SchedulerError::from)?
                .output_json;
            let mode = if options.json.unwrap_or(configured_json) {
                DetailOutput::Json
            } else {
                DetailOutput::Human
            };
            let database = pixiv_app::database::Database::open(&directory)
                .map_err(|error| CommandError::State(Box::new(error)))?;
            let execution = pixiv_app::execution::Execution::http(
                config,
                std::sync::Arc::new(std::sync::Mutex::new(database)),
            );
            return pixiv_cli_rs::user_detail::saved_user_profile(
                &execution,
                &pixiv_app::lifecycle::Context::new(),
                id,
                proxy,
                mode,
                &mut io::stdout().lock(),
            )
            .await;
        }
        Command::Bookmark {
            command:
                BookmarkGroupCommand::Detail {
                    options,
                    connection,
                },
        } => {
            let (directory, config) = account_config.expect("bookmark detail startup was resolved");
            options.resolve_target(&mut io::stdin().lock())?;
            options.validate()?;
            let proxy = connection.override_value()?;
            let configured_json = config
                .current()
                .and_then(|snapshot| snapshot.runtime())
                .map_err(pixiv_app::scheduler::SchedulerError::from)?
                .output_json;
            let mode = options.output_mode(configured_json, io::stdout().is_terminal())?;
            let database = pixiv_app::database::Database::open(&directory)
                .map_err(|error| CommandError::State(Box::new(error)))?;
            let execution = pixiv_app::execution::Execution::http(
                config,
                std::sync::Arc::new(std::sync::Mutex::new(database)),
            );
            return pixiv_cli_rs::bookmark_reads::saved_bookmark_detail(
                &execution,
                &pixiv_app::lifecycle::Context::new(),
                options.as_ref().clone(),
                proxy,
                mode,
                &mut io::stdout(),
            )
            .await;
        }
        Command::Bookmark {
            command:
                BookmarkGroupCommand::Tags {
                    options,
                    connection,
                },
        } => {
            let (directory, config) = account_config.expect("bookmark tags startup was resolved");
            options.resolve_target(&mut io::stdin().lock())?;
            options.validate()?;
            let proxy = connection.override_value()?;
            let configured_json = if options.listing.ndjson {
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
            *ndjson_output = options.listing.ndjson;
            let database = pixiv_app::database::Database::open(&directory)
                .map_err(|error| CommandError::State(Box::new(error)))?;
            let execution = pixiv_app::execution::Execution::http(
                config,
                std::sync::Arc::new(std::sync::Mutex::new(database)),
            );
            return pixiv_cli_rs::bookmark_reads::saved_bookmark_tags(
                &execution,
                &pixiv_app::lifecycle::Context::new(),
                options.as_ref().clone(),
                proxy,
                mode,
                io::stdout(),
            )
            .await;
        }
        _ => {}
    }
    if let Command::Comment {
        options,
        connection,
        command,
    } = &args.command
    {
        let (directory, config) = account_config.expect("comment startup was resolved");
        if let Some(CommentCommand::Mutation(action)) = command {
            action.validate()?;
            action.proxy_override()?;
            let json = action.input().json.unwrap_or(
                config
                    .current()
                    .and_then(|snapshot| snapshot.runtime())
                    .map_err(pixiv_app::scheduler::SchedulerError::from)?
                    .output_json,
            );
            *ndjson_output = false;
            let database = pixiv_app::database::Database::open(&directory)
                .map_err(|error| CommandError::State(Box::new(error)))?;
            let execution = pixiv_app::execution::Execution::http(
                config,
                std::sync::Arc::new(std::sync::Mutex::new(database)),
            );
            return pixiv_cli_rs::comment_mutations::saved_mutation(
                &execution,
                &pixiv_app::lifecycle::Context::new(),
                action.clone(),
                json,
                &mut io::stdout(),
            )
            .await;
        }

        let (json, ndjson) = match command {
            Some(CommentCommand::Stamps { options, .. }) => (options.json, options.ndjson),
            Some(CommentCommand::Mutation(_)) => unreachable!("mutation was handled"),
            None => (options.listing.json, options.listing.ndjson),
        };
        if command.is_none() {
            options.validate()?;
        }
        let connection = match command {
            Some(CommentCommand::Stamps { connection, .. }) => connection,
            Some(CommentCommand::Mutation(_)) => unreachable!("mutation was handled"),
            None => connection,
        };
        let proxy = connection.override_value()?;
        let configured = if ndjson {
            false
        } else {
            config
                .current()
                .and_then(|snapshot| snapshot.runtime())
                .map_err(pixiv_app::scheduler::SchedulerError::from)?
                .output_json
        };
        let mode = match command {
            Some(CommentCommand::Stamps { options, .. }) => {
                options.output_mode(configured, io::stdout().is_terminal())?
            }
            Some(CommentCommand::Mutation(_)) => unreachable!("mutation was handled"),
            None => options.output_mode(configured, io::stdout().is_terminal())?,
        };
        *ndjson_output = ndjson || (json.is_none() && !configured && !io::stdout().is_terminal());
        let database = pixiv_app::database::Database::open(&directory)
            .map_err(|error| CommandError::State(Box::new(error)))?;
        let execution = pixiv_app::execution::Execution::http(
            config,
            std::sync::Arc::new(std::sync::Mutex::new(database)),
        );
        let context = pixiv_app::lifecycle::Context::new();
        return match command {
            Some(CommentCommand::Stamps { .. }) => {
                pixiv_cli_rs::comment_reads::saved_stamps(
                    &execution,
                    &context,
                    proxy,
                    mode,
                    &mut io::stdout(),
                )
                .await
            }
            Some(CommentCommand::Mutation(_)) => unreachable!("mutation was handled"),
            None => {
                pixiv_cli_rs::comment_reads::saved_comments(
                    &execution,
                    &context,
                    options.clone(),
                    proxy,
                    mode,
                    io::stdout(),
                )
                .await
            }
        };
    }
    if let Some(mut options) = bookmark_lists {
        let (directory, config) = account_config.expect("bookmark lists startup was resolved");
        options.resolve_target(&mut io::stdin().lock())?;
        options.validate()?;
        let connection = match &args.command {
            Command::Bookmark {
                command: BookmarkGroupCommand::List { connection, .. },
            }
            | Command::User {
                command: UserCommand::Bookmarks { connection, .. },
            } => connection,
            _ => unreachable!("bookmark lists route was resolved"),
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
        *ndjson_output = options.options().ndjson
            || (matches!(
                options,
                pixiv_cli_rs::bookmark_lists::BookmarkLists::User(_)
            ) && mode == DetailOutput::Ndjson);
        let database = pixiv_app::database::Database::open(&directory)
            .map_err(|error| CommandError::State(Box::new(error)))?;
        let execution = pixiv_app::execution::Execution::http(
            config,
            std::sync::Arc::new(std::sync::Mutex::new(database)),
        );
        return pixiv_cli_rs::bookmark_lists::saved_bookmark_lists(
            &execution,
            &pixiv_app::lifecycle::Context::new(),
            options,
            proxy,
            mode,
            io::stdout(),
        )
        .await;
    }
    if let Some(options) = mypixiv {
        let (directory, config) = account_config.expect("mypixiv startup was resolved");
        options.validate()?;
        let connection = match &args.command {
            Command::Mypixiv {
                command:
                    MyPixivCommand::Users { connection, .. } | MyPixivCommand::Works { connection, .. },
            } => connection,
            _ => unreachable!("mypixiv route was resolved"),
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
        return pixiv_cli_rs::mypixiv::saved_mypixiv(
            &execution,
            &pixiv_app::lifecycle::Context::new(),
            options,
            proxy,
            mode,
            io::stdout(),
        )
        .await;
    }
    if let Some(options) = timeline {
        let (directory, config) = account_config.expect("timeline startup was resolved");
        options.validate()?;
        let connection = match &args.command {
            Command::Timeline {
                command:
                    TimelineCommand::Following { connection, .. }
                    | TimelineCommand::Latest { connection, .. },
            } => connection,
            _ => unreachable!("timeline route was resolved"),
        };
        let proxy = connection.override_value()?;
        let configured_json = if options.options().listing.ndjson {
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
        return pixiv_cli_rs::timeline::saved_timeline(
            &execution,
            &pixiv_app::lifecycle::Context::new(),
            options,
            proxy,
            mode,
            io::stdout(),
        )
        .await;
    }
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
        Command::Bookmark {
            command: BookmarkGroupCommand::Mutation(command),
        } => Some(pixiv_cli_rs::mutation::Mutation::Bookmark(command.clone())),
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
        Command::Auth => unreachable!("auth uses local execution"),
        Command::Config => unreachable!("config uses its own local execution"),
        Command::Timeline { .. } => unreachable!("timeline uses saved account execution"),
        Command::Mypixiv { .. } => unreachable!("mypixiv uses saved account execution"),
        Command::Novel { .. } => unreachable!("novel commands were resolved"),
        Command::Comment { .. } => unreachable!("comment command was handled"),
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

fn execute_config() -> Result<(), CommandError> {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let mut input = io::stdin().lock();
    let command = pixiv_cli_rs::config_commands::ConfigCommand::parse(
        &args,
        &mut input,
        io::stdin().is_terminal(),
    )?;
    let path = if command.requires_config() {
        let name = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
        let home = std::env::var_os(name)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                CommandError::MessageText(format!(
                    "{} is not defined",
                    if cfg!(windows) {
                        "%USERPROFILE%"
                    } else {
                        "$HOME"
                    }
                ))
            })?;
        std::path::PathBuf::from(home)
            .join(".pixiv-cli")
            .join("config.toml")
    } else {
        std::path::PathBuf::new()
    };
    command.execute(
        &pixiv_app::config::Store::new(path),
        &mut input,
        &mut io::stdout().lock(),
        &mut io::stderr().lock(),
    )
}

fn execute_auth() -> (Result<(), CommandError>, bool) {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let machine = pixiv_cli_rs::auth_accounts::machine_output_requested(&args);
    let result = (|| {
        let mut input = io::stdin().lock();
        let command = pixiv_cli_rs::auth_accounts::AuthCommand::parse(
            &args,
            &mut input,
            io::stdin().is_terminal(),
        )?;
        let path = if command.requires_config() {
            let name = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
            let home = std::env::var_os(name)
                .filter(|v| !v.is_empty())
                .ok_or_else(|| {
                    CommandError::MessageText(format!(
                        "{} is not defined",
                        if cfg!(windows) {
                            "%USERPROFILE%"
                        } else {
                            "$HOME"
                        }
                    ))
                })?;
            std::path::PathBuf::from(home).join(".pixiv-cli/config.toml")
        } else {
            std::path::PathBuf::new()
        };
        command.execute(
            &pixiv_app::config::Store::new(path),
            &pixiv_app::lifecycle::Context::new(),
            &mut io::stdout().lock(),
        )
    })();
    (result, machine)
}
