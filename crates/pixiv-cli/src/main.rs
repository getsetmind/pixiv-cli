use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand};
use pixiv_cli_rs::search::{SearchInput, SearchOptions};
use pixiv_cli_rs::{
    CommandError, DetailOutput, detail_artwork_id, finish_command, saved_artwork_detail,
};
use pixiv_sdk::{Error, Reason};
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
    #[command(about = "Download illustrations")]
    Download,
    #[command(about = "Read the Pixiv encyclopedia (dic.pixiv.net)")]
    Dic,
    #[command(about = "Browse and download Pixiv FANBOX content")]
    Fanbox,
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
    #[command(args_override_self = true, about = "Show ugoira animation metadata")]
    Ugoira {
        #[command(flatten)]
        options: pixiv_cli_rs::ugoira::UgoiraOptions,
        #[command(flatten)]
        connection: ProxyOptions,
    },
}

#[tokio::main]
async fn main() {
    let owner = match pixiv_cli_rs::interrupt::OwnedSignalContext::new() {
        Ok(owner) => owner,
        Err(error) => {
            let exit = finish_command(Err(error.into()), false, false, &mut io::stderr().lock());
            std::process::exit(exit);
        }
    };
    let root_context = owner.context();
    let raw_args: Vec<String> = std::env::args().skip(1).collect();
    if pixiv_cli_rs::fanbox::is_fanbox_route(&raw_args) {
        let (result, ndjson, machine) = execute_fanbox(&root_context, &raw_args).await;
        let exit = finish_command(result, ndjson, machine, &mut io::stderr().lock());
        if exit != 0 {
            drop(owner);
            std::process::exit(exit);
        }
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("download") {
        let (result, ndjson, machine) = execute_download(&root_context).await;
        let exit = finish_command(result, ndjson, machine, &mut io::stderr().lock());
        if exit != 0 {
            drop(owner);
            std::process::exit(exit);
        }
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("auth") {
        let (result, machine) = execute_auth(&root_context).await;
        let exit = finish_command(result, false, machine, &mut io::stderr().lock());
        if exit != 0 {
            drop(owner);
            std::process::exit(exit);
        }
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("config") {
        let result = execute_config(&root_context);
        let exit = finish_command(result, false, false, &mut io::stderr().lock());
        if exit != 0 {
            drop(owner);
            std::process::exit(exit);
        }
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("dic") {
        let (result, ndjson, machine) = execute_dictionary(&root_context).await;
        let exit = finish_command(result, ndjson, machine, &mut io::stderr().lock());
        if exit != 0 {
            drop(owner);
            std::process::exit(exit);
        }
        return;
    }
    let matches = Arguments::command()
        .mut_subcommand("ugoira", pixiv_cli_rs::ugoira::configure_command)
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
            if error.kind() == clap::error::ErrorKind::DisplayHelp
                && matches!(std::env::args().nth(1).as_deref(), Some("--help" | "-h"))
            {
                let help = error.to_string().replacen(
                    "\n  download     Download illustrations\n",
                    "\n  download    Download illustrations\n",
                    1,
                );
                print!("{help}");
                std::process::exit(0);
            }
            let mutation_route = matches!(
                std::env::args().nth(1).as_deref(),
                Some("bookmark" | "follow" | "user")
            );
            let command_error = if std::env::args().nth(1).as_deref() == Some("ugoira") {
                pixiv_cli_rs::ugoira::argument_error(&error)
            } else if matches!(
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
        | Command::Download
        | Command::Config
        | Command::Dic
        | Command::Fanbox
        | Command::Mcp { .. }
        | Command::Bookmark { .. }
        | Command::Follow { .. }
        | Command::User { .. } => false,
        Command::Detail { json, ndjson, .. } => *json || *ndjson,
        Command::Ugoira { options, .. } => options.json.is_some(),
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
        | Command::Download
        | Command::Config
        | Command::Dic
        | Command::Fanbox
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
        execute(args, &root_context, &mut ndjson_output).await,
        ndjson_output,
        machine_output,
        &mut io::stderr().lock(),
    );
    if exit != 0 {
        drop(owner);
        std::process::exit(exit);
    }
}

async fn execute(
    mut args: Arguments,
    root_context: &pixiv_app::lifecycle::Context,
    ndjson_output: &mut bool,
) -> Result<(), CommandError> {
    if let Command::Ugoira { options, .. } = &args.command {
        options.validate_arguments()?;
    }
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
    pixiv_cli_rs::startup::run_system_startup(root_context, &mut io::stderr())?;
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
            | Command::Ugoira { .. }
    ) {
        let directory = pixiv_app::callback_handler::app_data_directory()
            .map_err(|error| CommandError::MessageText(error.to_string()))?;
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
        Command::Ugoira {
            options,
            connection,
        } => {
            let (directory, config) = account_config.expect("ugoira startup was resolved");
            let proxy = connection.override_value()?;
            let id = options
                .artwork_id()
                .map_err(|error| CommandError::Usage(error.to_string()))?;
            let database = pixiv_app::database::Database::open(&directory)
                .map_err(|error| CommandError::State(Box::new(error)))?;
            let configured_json = config
                .current()
                .and_then(|snapshot| snapshot.runtime())
                .map_err(pixiv_app::scheduler::SchedulerError::from)?
                .output_json;
            let json = options.output_json(configured_json);
            let execution = pixiv_app::execution::Execution::http(
                config,
                std::sync::Arc::new(std::sync::Mutex::new(database)),
            );
            return pixiv_cli_rs::ugoira::saved_ugoira(
                &execution,
                root_context,
                id,
                proxy,
                json,
                io::stdout(),
            )
            .await;
        }
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
                root_context,
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
                root_context,
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
                root_context,
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
                root_context,
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
        let context = root_context.clone();
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
            root_context,
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
            root_context,
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
            root_context,
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
            root_context,
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
            root_context,
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
            root_context,
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

    if let Command::Search {
        input, connection, ..
    } = &args.command
    {
        let source = search_word.as_deref().unwrap_or("");
        let reverse_input = pixiv_cli_rs::reverse_search::Input {
            source: source.to_owned(),
            provider: input.provider.clone().unwrap_or_default(),
            changed_flags: input
                .changed_flags()
                .iter()
                .map(|flag| (*flag).to_owned())
                .collect(),
            ndjson: input.ndjson,
            json_changed: input.changed_flags().contains(&"json"),
        };
        pixiv_cli_rs::reverse_search::validate_input(&reverse_input)?;
        if pixiv_cli_rs::reverse_search::is_image_source(source) {
            let (_, config) = account_config
                .as_ref()
                .expect("search startup was resolved");
            let runtime = config
                .current()
                .and_then(|snapshot| snapshot.runtime())
                .map_err(pixiv_app::scheduler::SchedulerError::from)?;
            let proxy = connection.override_value()?;
            let provider = pixiv_cli_rs::reverse_search::resolve_provider(
                input.provider.as_deref().unwrap_or(""),
                &runtime.reverse_search_provider,
            )?;
            let mode = input.output_mode(runtime.output_json, io::stdout().is_terminal())?;
            *ndjson_output = mode == DetailOutput::Ndjson;
            let searcher = pixiv_app::reverse_search::assembly::build(
                pixiv_app::reverse_search::assembly::Options::from_runtime(&runtime, proxy),
            )
            .map_err(CommandError::ReverseSearch)?;
            let result = pixiv_cli_rs::reverse_search::run(
                searcher.as_ref(),
                std::sync::Arc::new(root_context.clone()),
                pixiv_app::reverse_search::Request {
                    source: source.to_owned(),
                    provider,
                    pixiv_only: runtime.reverse_search_pixiv_only,
                },
                mode,
                &mut io::stdout().lock(),
                Some(&mut io::stderr().lock()),
            )
            .await;
            let cleanup = searcher.close().await.map_err(CommandError::ReverseSearch);
            return pixiv_cli_rs::finish_with_cleanup(result, cleanup);
        }
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
                root_context,
                proxy,
                mode == DetailOutput::Json,
                &mut io::stdout().lock(),
            )
            .await;
        }
        if novel_search_request.is_some() {
            return pixiv_cli_rs::novel_search::saved_novel_search(
                &execution,
                root_context,
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
                root_context,
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
            root_context,
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
            root_context,
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
            root_context,
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
            root_context,
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
                root_context,
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
                root_context,
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
            root_context,
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
        let download_runtime = config
            .current()
            .and_then(|snapshot| snapshot.runtime())
            .map_err(pixiv_app::scheduler::SchedulerError::from)?;
        let reverse_searcher = pixiv_app::reverse_search::assembly::build(
            pixiv_app::reverse_search::assembly::Options::from_runtime(&download_runtime, proxy),
        )
        .map_err(CommandError::ReverseSearch)?;
        let reverse_executor = pixiv_mcp::reverse_search::ReverseExecutor::new(
            reverse_searcher.clone(),
            pixiv_app::reverse_search::Provider::from(
                download_runtime.reverse_search_provider.as_str(),
            ),
            download_runtime.reverse_search_pixiv_only,
        );
        let result = async {
            let database = pixiv_app::database::Database::open(&directory)
                .map_err(|error| CommandError::State(Box::new(error)))?;
            let download_defaults = pixiv_mcp::download::DownloadDefaults::from(&download_runtime);
            let execution = std::sync::Arc::new(pixiv_app::execution::Execution::http(
                config,
                std::sync::Arc::new(std::sync::Mutex::new(database)),
            ));
            let random_execution = execution.clone();
            let random_defaults = download_defaults.clone();
            let random_account = pixiv_mcp::runtime::Account {
                user_id: 0,
                https_proxy_override: proxy.map(str::to_owned),
            };
            let download_execution = execution.clone();
            let download_account = pixiv_mcp::runtime::Account {
                user_id: 0,
                https_proxy_override: proxy.map(str::to_owned),
            };
            let download = move |context, input| -> pixiv_mcp::download::DownloadFuture {
                let execution = download_execution.clone();
                let defaults = download_defaults.clone();
                let account = download_account.clone();
                Box::pin(async move {
                    pixiv_mcp::download::saved_download_with_account(
                        &execution,
                        &context,
                        &defaults,
                        input,
                        &account,
                        std::sync::Arc::new(|client| {
                            std::sync::Arc::new(pixiv_app::download::NativeDownloadSaveClient::new(
                                client,
                            ))
                        }),
                    )
                    .await
                })
            };
            let random = move |context, input| -> pixiv_mcp::download::DownloadFuture {
                let execution = random_execution.clone();
                let defaults = random_defaults.clone();
                let account = random_account.clone();
                Box::pin(async move {
                    pixiv_mcp::download::saved_download_random_with_account(
                        &execution,
                        &context,
                        &defaults,
                        input,
                        &account,
                        std::sync::Arc::new(|client| {
                            std::sync::Arc::new(pixiv_app::download::NativeDownloadSaveClient::new(
                                client,
                            ))
                        }),
                    )
                    .await
                })
            };
            pixiv_mcp::stdio::serve_saved_with_reverse_and_downloads_context(
                &execution,
                proxy,
                pixiv_mcp::stdio::DownloadExecutors {
                    download: Some(&download),
                    random_from_recommendation: Some(&random),
                },
                Some(&reverse_executor),
                root_context,
                tokio::io::stdin(),
                &mut tokio::io::stdout(),
            )
            .await?;
            Ok(())
        }
        .await;
        let cleanup = reverse_searcher
            .close()
            .await
            .map_err(CommandError::ReverseSearch);
        return pixiv_cli_rs::finish_with_cleanup(result, cleanup);
    }
    match args.command {
        Command::Auth
        | Command::Config
        | Command::Dic
        | Command::Fanbox
        | Command::Download
        | Command::Comment { .. }
        | Command::Mypixiv { .. }
        | Command::Timeline { .. }
        | Command::Bookmark { .. }
        | Command::Follow { .. }
        | Command::User { .. }
        | Command::Novel { .. }
        | Command::Mcp { .. }
        | Command::Detail { .. }
        | Command::Search { .. }
        | Command::Ranking { .. }
        | Command::Series { .. }
        | Command::Recommended { .. }
        | Command::Ugoira { .. } => {
            unreachable!("commands use saved-account or local execution")
        }
    }
}

fn fanbox_runtime()
-> Result<(std::path::PathBuf, std::sync::Arc<pixiv_app::config::Store>), CommandError> {
    let directory = pixiv_app::callback_handler::app_data_directory()
        .map_err(|error| CommandError::MessageText(error.to_string()))?;
    let config = std::sync::Arc::new(pixiv_app::config::Store::new(directory.join("config.toml")));
    config
        .ensure_defaults()
        .map_err(pixiv_app::scheduler::SchedulerError::from)?;
    config
        .current()
        .and_then(|snapshot| snapshot.runtime())
        .map_err(pixiv_app::scheduler::SchedulerError::from)?;
    Ok((directory, config))
}

fn fanbox_service(
    directory: std::path::PathBuf,
    config: std::sync::Arc<pixiv_app::config::Store>,
) -> Result<Option<std::sync::Arc<pixiv_app::fanbox_facade::Facade>>, CommandError> {
    fanbox_service_owned(directory, config, &mut None)
}

fn fanbox_service_owned(
    directory: std::path::PathBuf,
    config: std::sync::Arc<pixiv_app::config::Store>,
    owned: &mut Option<std::sync::Arc<std::sync::Mutex<pixiv_app::database::Database>>>,
) -> Result<Option<std::sync::Arc<pixiv_app::fanbox_facade::Facade>>, CommandError> {
    let database = pixiv_app::database::Database::open(directory)
        .map_err(|error| CommandError::State(Box::new(error)))?;
    let database = std::sync::Arc::new(std::sync::Mutex::new(database));
    *owned = Some(database.clone());
    let accounts = pixiv_app::fanbox_account_service::AccountService::from_store(database, config);
    Ok(Some(std::sync::Arc::new(
        pixiv_app::fanbox_facade::Facade::new(Some(std::sync::Arc::new(accounts))),
    )))
}

struct SystemFanboxAuthUpdateHooks;
impl pixiv_cli_rs::fanbox_auth::AutomaticUpdateHooks for SystemFanboxAuthUpdateHooks {
    fn runtime(&self) -> Result<pixiv_cli_rs::fanbox_auth::UpdateRuntime, CommandError> {
        let directory = pixiv_app::callback_handler::app_data_directory()
            .map_err(|error| CommandError::MessageText(error.to_string()))?;
        let store = pixiv_app::config::Store::new(directory.join("config.toml"));
        let runtime = store
            .current()
            .and_then(|snapshot| snapshot.runtime())
            .map_err(pixiv_app::scheduler::SchedulerError::from)?;
        Ok(pixiv_cli_rs::fanbox_auth::UpdateRuntime {
            enabled: runtime.update_check_enabled,
            https_proxy: runtime.https_proxy,
        })
    }

    fn check(&self, _: &pixiv_app::lifecycle::Context, _: &str) -> Result<(), CommandError> {
        Err(CommandError::Message(
            "automatic update checker is not implemented",
        ))
    }
}

fn close_fanbox_database(
    database: std::sync::Arc<std::sync::Mutex<pixiv_app::database::Database>>,
) -> Result<(), CommandError> {
    let database = std::sync::Arc::try_unwrap(database).map_err(|_| {
        CommandError::Message("FANBOX account service retained the database after execution")
    })?;
    let database = database
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    database
        .close()
        .map_err(|error| CommandError::State(Box::new(error)))
}

async fn execute_fanbox(
    root_context: &pixiv_app::lifecycle::Context,
    args: &[String],
) -> (Result<(), CommandError>, bool, bool) {
    use pixiv_cli_rs::{fanbox, fanbox_auth, fanbox_mcp};
    let mut ndjson = false;
    let mut machine = false;
    let result = async {
        let route = fanbox::command_route(args);
        if route.path.get(1).is_none_or(|part| part != "auth")
            && let Some(help) = fanbox::help_route(args)?
        {
            if route.path.get(1).is_some_and(|part| part == "download") {
                return pixiv_cli_rs::fanbox_download::DownloadCommand::parse(args)?
                    .execute(
                        root_context,
                        std::path::Path::new("."),
                        || Ok(None),
                        &mut io::stdout().lock(),
                    )
                    .await;
            }
            io::stdout().write_all(help.as_bytes())?;
            return Ok(());
        }
        match route
            .path
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .as_slice()
        {
            ["fanbox", "mcp"] => {
                let command = fanbox_mcp::McpCommand::parse(&route.args)?;
                pixiv_cli_rs::startup::run_system_startup(root_context, &mut io::stderr())?;
                let (directory, config) = fanbox_runtime()?;
                command
                    .run(fanbox_mcp::Data {
                        service_factory: move || fanbox_service(directory, config),
                        run_server: |facade, proxy| async move {
                            let server = pixiv_mcp::fanbox::Server::saved(facade, proxy);
                            pixiv_mcp::fanbox::stdio::serve(
                                &server,
                                Some(root_context),
                                tokio::io::stdin(),
                                &mut tokio::io::stdout(),
                            )
                            .await
                            .map_err(Into::into)
                        },
                    })
                    .await
            }
            [
                "fanbox",
                "creators" | "posts" | "tags" | "home" | "supporting" | "post",
            ] => {
                let requested = fanbox::ReadCommand::parse(args)?;
                ndjson = requested.ndjson;
                machine = requested.json.is_some() || ndjson;
                let mut runtime = None;
                let command = fanbox::prepare_root(
                    root_context,
                    args,
                    &mut io::stdin().lock(),
                    io::stdin().is_terminal(),
                    &pixiv_cli_rs::startup::SystemStartupHooks::system(),
                    &mut io::stderr(),
                    || {
                        runtime = Some(fanbox_runtime()?);
                        Ok(())
                    },
                )?;
                let (directory, config) = runtime.expect("FANBOX read startup was resolved");
                command
                    .execute(
                        root_context,
                        move || fanbox_service(directory, config),
                        &mut io::stdout().lock(),
                    )
                    .await
            }
            ["fanbox"] => Err(CommandError::Message("usage: pixiv fanbox <command>")),
            ["fanbox", "auth", ..] => {
                let requested = fanbox_auth::AuthCommand::parse(args)?;
                machine = requested.machine_requested();
                let command = fanbox_auth::prepare_root(
                    root_context,
                    args,
                    &mut io::stdin().lock(),
                    io::stdin().is_terminal(),
                    &pixiv_cli_rs::startup::SystemStartupHooks::system(),
                    &mut io::stderr(),
                )?;
                let mut prompts = pixiv_cli_rs::terminal_prompt::TerminalPrompts::new(
                    io::stdin(),
                    io::stdout(),
                    io::stderr(),
                );
                let browser = pixiv_cli_rs::fanbox_browser::SystemBrowserProvider::system();
                let mut owned_database = None;
                let mut result = command
                    .execute(
                        root_context,
                        fanbox_auth::Data {
                            service_factory: || {
                                let directory = pixiv_app::callback_handler::app_data_directory()
                                    .map_err(|error| {
                                    CommandError::MessageText(error.to_string())
                                })?;
                                let database = pixiv_app::database::Database::open(&directory)
                                    .map_err(|error| CommandError::State(Box::new(error)))?;
                                let database = std::sync::Arc::new(std::sync::Mutex::new(database));
                                owned_database = Some(database.clone());
                                let store = std::sync::Arc::new(pixiv_app::config::Store::new(
                                    directory.join("config.toml"),
                                ));
                                Ok(Some(std::sync::Arc::new(
                                    pixiv_app::fanbox_account_service::AccountService::from_store(
                                        database, store,
                                    ),
                                )))
                            },
                            reader: &mut io::stdin().lock(),
                            writer: &mut io::stdout().lock(),
                            prompts: &mut prompts,
                            browser: &browser,
                        },
                    )
                    .await;
                if result.is_ok() {
                    let release = option_env!("PIXIV_BUILD_VERSION").unwrap_or("dev") != "dev";
                    command.post_success(
                        root_context,
                        release,
                        &SystemFanboxAuthUpdateHooks,
                        &mut io::stderr(),
                    );
                }
                if let Some(database) = owned_database {
                    result =
                        pixiv_cli_rs::finish_with_cleanup(result, close_fanbox_database(database));
                }
                result
            }
            ["fanbox", "download"] => {
                let mut runtime = None;
                let command = pixiv_cli_rs::fanbox_download::prepare_root(
                    root_context,
                    args,
                    &mut io::stdin().lock(),
                    io::stdin().is_terminal(),
                    &pixiv_cli_rs::startup::SystemStartupHooks::system(),
                    &mut io::stderr(),
                    || {
                        runtime = Some(fanbox_runtime()?);
                        Ok(())
                    },
                )?;
                let (directory, config) = runtime.expect("FANBOX download startup was resolved");
                let download_path = config
                    .current()
                    .and_then(|snapshot| snapshot.runtime())
                    .map_err(pixiv_app::scheduler::SchedulerError::from)?
                    .download_path;
                let mut owned_database = None;
                let mut result = command
                    .execute(
                        root_context,
                        std::path::Path::new(&download_path),
                        || fanbox_service_owned(directory, config, &mut owned_database),
                        &mut io::stdout().lock(),
                    )
                    .await;
                if result.is_ok() {
                    let release = option_env!("PIXIV_BUILD_VERSION").unwrap_or("dev") != "dev";
                    command.post_success(
                        root_context,
                        release,
                        &SystemFanboxAuthUpdateHooks,
                        &mut io::stderr(),
                    );
                }
                if let Some(database) = owned_database {
                    result =
                        pixiv_cli_rs::finish_with_cleanup(result, close_fanbox_database(database));
                }
                result
            }
            _ => Err(CommandError::Message("usage: pixiv fanbox <command>")),
        }
    }
    .await;
    (result, ndjson, machine)
}

async fn execute_dictionary(
    root_context: &pixiv_app::lifecycle::Context,
) -> (Result<(), CommandError>, bool, bool) {
    use pixiv_cli_rs::dictionary::{DictionaryCommand, service};
    let args: Vec<String> = std::env::args().skip(2).collect();
    let (ndjson, machine) = DictionaryCommand::output_policy_requested(&args);
    let result = async {
        let command = DictionaryCommand::parse_root(&args)?;
        let config = if command.requires_runtime() {
            pixiv_cli_rs::startup::run_system_startup(root_context, &mut io::stderr())?;
            let directory = pixiv_app::callback_handler::app_data_directory()
                .map_err(|error| CommandError::MessageText(error.to_string()))?;
            let config = pixiv_app::config::Store::new(directory.join("config.toml"));
            config
                .ensure_defaults()
                .map_err(pixiv_app::scheduler::SchedulerError::from)?;
            config
                .current()
                .and_then(|snapshot| snapshot.runtime())
                .map_err(pixiv_app::scheduler::SchedulerError::from)?;
            Some(config)
        } else {
            None
        };
        if command.render_help("pixiv dic").is_some() {
            command.write_help("pixiv dic", io::stdout());
            return Ok(());
        }
        command.validate_options()?;
        let transport = service::HttpTransport::new().map_err(CommandError::State)?;
        let client = service::Client::new(Some(transport));
        let config = config.expect("dictionary leaf startup was resolved");
        command
            .execute(&client, root_context, io::stdout(), move |override_json| {
                if let Some(value) = override_json {
                    return Ok(value);
                }
                config
                    .current()
                    .and_then(|snapshot| snapshot.runtime())
                    .map(|runtime| runtime.output_json)
                    .map_err(pixiv_app::scheduler::SchedulerError::from)
                    .map_err(Into::into)
            })
            .await
    }
    .await;
    (result, ndjson, machine)
}

fn execute_config(root_context: &pixiv_app::lifecycle::Context) -> Result<(), CommandError> {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let mut input = io::stdin().lock();
    let command = pixiv_cli_rs::config_commands::ConfigCommand::parse(
        &args,
        &mut input,
        io::stdin().is_terminal(),
    )?;
    if command.requires_config() {
        pixiv_cli_rs::startup::run_system_startup(root_context, &mut io::stderr())?;
    }
    let path = if command.requires_config() {
        pixiv_app::callback_handler::app_data_directory()
            .map_err(|error| CommandError::MessageText(error.to_string()))?
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

#[derive(Debug)]
struct DownloadConfigReadError {
    path: std::path::PathBuf,
    cause: pixiv_app::config::ConfigError,
}

impl std::fmt::Display for DownloadConfigReadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "read {}: is a directory", self.path.display())
    }
}

impl std::error::Error for DownloadConfigReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.cause)
    }
}

async fn execute_download(
    root_context: &pixiv_app::lifecycle::Context,
) -> (Result<(), CommandError>, bool, bool) {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let (ndjson, machine) = pixiv_cli_rs::download::DownloadCommand::output_policy_requested(&args);
    let result = async {
        let mut input = io::stdin().lock();
        let command = pixiv_cli_rs::download::DownloadCommand::parse_root(
            &args,
            &mut input,
            io::stdin().is_terminal(),
        )?;
        if let Some(help) = command.render_help("pixiv download") {
            io::stdout().write_all(help.as_bytes())?;
            return Ok(());
        }
        let context = root_context.clone();
        if command.requires_startup() {
            pixiv_cli_rs::startup::run_system_startup(&context, &mut io::stderr())?;
        }
        let path = if command.requires_config() {
            pixiv_app::callback_handler::app_data_directory()
                .map_err(|error| CommandError::MessageText(error.to_string()))?
                .join("config.toml")
        } else {
            std::path::PathBuf::new()
        };
        let store = pixiv_app::config::Store::new(path);
        if command.requires_config() {
            store
                .ensure_defaults()
                .map_err(pixiv_app::scheduler::SchedulerError::from)?;
            store
                .current()
                .and_then(|snapshot| snapshot.runtime())
                .map_err(|error| {
                    if cfg!(unix)
                        && matches!(&error, pixiv_app::config::ConfigError::Io(cause) if cause.kind() == io::ErrorKind::IsADirectory)
                    {
                        CommandError::State(Box::new(DownloadConfigReadError {
                            path: store.path().to_owned(),
                            cause: error,
                        }))
                    } else {
                        pixiv_app::scheduler::SchedulerError::from(error).into()
                    }
                })?;
        }
        command
            .execute_http(
                &store,
                &context,
                &mut input,
                std::sync::Arc::new(std::sync::Mutex::new(io::stdout())),
                std::sync::Arc::new(std::sync::Mutex::new(io::stderr())),
            )
            .await
    }
    .await;
    (result, ndjson, machine)
}

async fn execute_auth(
    root_context: &pixiv_app::lifecycle::Context,
) -> (Result<(), CommandError>, bool) {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let machine = pixiv_cli_rs::auth_accounts::machine_output_requested(&args);
    let result = async {
        let mut input = io::stdin().lock();
        let command = pixiv_cli_rs::auth_accounts::AuthCommand::parse(
            &args,
            &mut input,
            io::stdin().is_terminal()
                && (pixiv_cli_rs::auth_transfer::TransferCommand::discover(&args)
                    != Some("import")
                    || io::stdout().is_terminal()),
        )?;
        let context = root_context.clone();
        if command.requires_startup() {
            pixiv_cli_rs::startup::run_system_startup(&context, &mut io::stderr())?;
        }
        let path = if command.requires_config() {
            pixiv_app::callback_handler::app_data_directory()
                .map_err(|error| CommandError::MessageText(error.to_string()))?
                .join("config.toml")
        } else {
            std::path::PathBuf::new()
        };
        if command.requires_startup() {
            let store = pixiv_app::config::Store::new(&path);
            store
                .ensure_defaults()
                .map_err(pixiv_app::scheduler::SchedulerError::from)?;
            store
                .current()
                .and_then(|snapshot| snapshot.runtime())
                .map_err(pixiv_app::scheduler::SchedulerError::from)?;
        }
        drop(input);
        if let pixiv_cli_rs::auth_accounts::AuthCommand::Hidden(hidden) = &command {
            return hidden
                .execute_system(&context, &mut io::stdout(), &mut io::stderr())
                .await;
        }
        if let pixiv_cli_rs::auth_accounts::AuthCommand::Login(login) = &command {
            return login
                .execute_http(
                    &pixiv_app::config::Store::new(path),
                    &context,
                    &mut io::stdout(),
                )
                .await;
        }
        let mut prompts = pixiv_cli_rs::terminal_prompt::TerminalPrompts::new(
            io::stdin(),
            io::stdout(),
            io::stderr(),
        );
        if let pixiv_cli_rs::auth_accounts::AuthCommand::Validation(validation) = &command {
            return validation
                .execute_http(
                    &pixiv_app::config::Store::new(path),
                    root_context,
                    &mut io::stdout().lock(),
                )
                .await;
        }
        if let pixiv_cli_rs::auth_accounts::AuthCommand::Transfer(transfer) = &command {
            return transfer
                .execute_http(
                    &pixiv_app::config::Store::new(path),
                    root_context,
                    &mut io::stdout().lock(),
                    &mut prompts,
                )
                .await;
        }
        command.execute_with_prompts(
            &pixiv_app::config::Store::new(path),
            root_context,
            &mut io::stdout().lock(),
            &mut prompts,
        )
    }
    .await;
    (result, machine)
}
