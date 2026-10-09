use crate::{CommandError, DetailOutput, user_works::UserWorksOptions};
use pixiv_app::{execution::Execution, lifecycle::Context};
use pixiv_sdk::{Client, cursor::Cursor, transport::Transport};
use std::io::Write;

#[derive(clap::Args, Clone, Debug, Default)]
pub struct TimelineOptions {
    #[command(flatten)]
    pub listing: UserWorksOptions,
    #[arg(long = "type", short = 't')]
    pub entity: Option<String>,
    #[arg(long)]
    pub content_type: Option<String>,
}
#[derive(clap::Args, Clone, Debug)]
pub struct FollowingOptions {
    #[command(flatten)]
    pub timeline: TimelineOptions,
    #[arg(long, default_value = "public")]
    pub restrict: String,
}
#[derive(Clone, Debug)]
pub enum Timeline {
    Following(FollowingOptions),
    Latest(TimelineOptions),
}
impl Timeline {
    pub fn options(&self) -> &TimelineOptions {
        match self {
            Self::Following(options) => &options.timeline,
            Self::Latest(options) => options,
        }
    }
    pub fn validate_arguments(&self) -> Result<(), CommandError> {
        if self.options().listing.sources.is_empty() {
            return Ok(());
        }
        Err(CommandError::Message(match self {
            Self::Following(_) => "usage: pixiv timeline following --type artwork|novel",
            Self::Latest(_) => {
                "usage: pixiv timeline latest --type artwork|novel [--content-type illust|manga]"
            }
        }))
    }
    fn selection(&self) -> Result<(bool, String), CommandError> {
        let options = self.options();
        let mut entity = options.entity.as_deref().unwrap_or_default();
        let mut subtype = options.content_type.clone().unwrap_or_else(|| {
            if matches!(self, Self::Following(_)) {
                "all"
            } else {
                "illust"
            }
            .into()
        });
        match self {
            Self::Following(_) if entity == "illust" => entity = "artwork",
            Self::Latest(_) if matches!(entity, "illust" | "manga") => {
                if options.content_type.is_some() {
                    return Err(CommandError::Message(
                        "legacy artwork --type cannot be combined with --content-type",
                    ));
                }
                subtype = entity.into();
                entity = "artwork";
            }
            _ => {}
        }
        if !matches!(entity, "artwork" | "novel") {
            return Err(CommandError::Message("type must be one of: artwork, novel"));
        }
        if entity == "novel" && options.content_type.is_some() {
            return Err(CommandError::Message(
                "--content-type is only supported when --type artwork",
            ));
        }
        if entity == "artwork" {
            match self {
                Self::Following(_) => {
                    subtype = pixiv_app::search_filter::normalize_filter("all", &subtype)
                        .map_err(|error| CommandError::Message(error.message()))?
                        .content_type
                        .into();
                }
                Self::Latest(_) if !matches!(subtype.as_str(), "illust" | "manga") => {
                    return Err(CommandError::Message(
                        "content-type must be one of: illust, manga",
                    ));
                }
                _ => {}
            }
        }
        Ok((entity == "artwork", subtype))
    }
    fn plan(&self) -> Result<crate::search::SearchPlan, CommandError> {
        crate::search::SearchOptions {
            limit: self.options().listing.limit,
            page: self.options().listing.page,
            ..Default::default()
        }
        .plan()
    }
    pub fn validate(&self) -> Result<(), CommandError> {
        self.validate_arguments()?;
        self.selection()?;
        self.plan()?;
        Ok(())
    }
    pub fn output_mode(
        &self,
        configured_json: bool,
        terminal: bool,
    ) -> Result<DetailOutput, CommandError> {
        crate::user_works::UserWorks::Novels(self.options().listing.clone())
            .output_mode(configured_json, terminal)
    }
    fn listing(&self) -> Result<Listing, CommandError> {
        self.validate()?;
        let (artwork, subtype) = self.selection()?;
        let plan = self.plan()?;
        Ok(match (self, artwork) {
            (Self::Following(options), true) => Listing::Artwork(crate::artwork_list::Listing {
                source: crate::artwork_list::Source::Following(
                    pixiv_sdk::pixiv::FollowingArtworksRequest {
                        restrict: options.restrict.clone(),
                        cursor: Cursor::default(),
                    },
                    pixiv_app::search_filter::normalize_filter("all", &subtype)
                        .map_err(|error| CommandError::Message(error.message()))?,
                ),
                heading: "new artworks from followed users".into(),
                plan,
            }),
            (Self::Following(options), false) => Listing::Novel(crate::novel_list::Listing {
                source: crate::novel_list::Source::Following(
                    pixiv_sdk::pixiv::FollowingNovelsRequest {
                        restrict: options.restrict.clone(),
                        cursor: Cursor::default(),
                    },
                ),
                heading: "new novels from followed users".into(),
                plan,
            }),
            (Self::Latest(_), true) => Listing::Artwork(crate::artwork_list::Listing {
                source: crate::artwork_list::Source::Latest(
                    pixiv_sdk::pixiv::LatestArtworksRequest {
                        content_type: subtype.clone(),
                        cursor: Cursor::default(),
                    },
                ),
                heading: format!("latest {subtype}"),
                plan,
            }),
            (Self::Latest(_), false) => Listing::Novel(crate::novel_list::Listing {
                source: crate::novel_list::Source::Latest(pixiv_sdk::pixiv::LatestNovelsRequest {
                    cursor: Cursor::default(),
                }),
                heading: "latest novels".into(),
                plan,
            }),
        })
    }
}
enum Listing {
    Artwork(crate::artwork_list::Listing),
    Novel(crate::novel_list::Listing),
}
pub async fn timeline<T: Transport, W: Write>(
    client: &Client<T>,
    options: &Timeline,
    mode: DetailOutput,
    out: &mut W,
) -> Result<(), CommandError> {
    let spool = match options.listing()? {
        Listing::Artwork(listing) => {
            crate::artwork_list::attempt(client, &listing, mode, out).await?
        }
        Listing::Novel(listing) => crate::novel_list::attempt(client, &listing, mode, out).await?,
    };
    if let Some(mut spool) = spool {
        spool.commit(out)?;
    }
    Ok(())
}
pub async fn saved_timeline<T: Transport + 'static, W: Write + Send + 'static>(
    execution: &Execution<T>,
    context: &Context,
    options: Timeline,
    proxy: Option<&str>,
    mode: DetailOutput,
    output: W,
) -> Result<(), CommandError> {
    match options.listing()? {
        Listing::Artwork(listing) => {
            crate::artwork_list::saved(execution, context, listing, proxy, mode, output).await
        }
        Listing::Novel(listing) => {
            crate::novel_list::saved(execution, context, listing, proxy, mode, output).await
        }
    }
}

pub(crate) fn timeline_integer(value: &str) -> Result<i64, String> {
    let (negative, unsigned) = if let Some(value) = value.strip_prefix('-') {
        (true, value)
    } else {
        (false, value.strip_prefix('+').unwrap_or(value))
    };
    let (radix, digits, prefix) = if unsigned.starts_with("0x") || unsigned.starts_with("0X") {
        (16, &unsigned[2..], true)
    } else if unsigned.starts_with("0o") || unsigned.starts_with("0O") {
        (8, &unsigned[2..], true)
    } else if unsigned.starts_with("0b") || unsigned.starts_with("0B") {
        (2, &unsigned[2..], true)
    } else if unsigned.len() > 1 && unsigned.starts_with('0') {
        (8, &unsigned[1..], true)
    } else {
        (10, unsigned, false)
    };
    let invalid = || "invalid syntax".to_owned();
    if digits.is_empty() {
        return Err(invalid());
    }
    let mut underscore = false;
    let mut invalid_underscore = false;
    let mut magnitude = 0_u64;
    for (index, character) in digits.char_indices() {
        if character == '_' {
            invalid_underscore |=
                underscore || (index == 0 && !prefix) || index + 1 == digits.len();
            underscore = true;
        } else {
            let digit = character
                .to_digit(radix)
                .filter(|_| character.is_ascii())
                .ok_or_else(invalid)?;
            magnitude = magnitude
                .checked_mul(u64::from(radix))
                .and_then(|magnitude| magnitude.checked_add(u64::from(digit)))
                .ok_or_else(|| "value out of range".to_owned())?;
            underscore = false;
        }
    }
    if invalid_underscore {
        return Err(invalid());
    }
    let maximum = i64::MAX as u64 + u64::from(negative);
    if magnitude > maximum {
        return Err("value out of range".into());
    }
    Ok(if negative && magnitude == maximum {
        i64::MIN
    } else if negative {
        -(magnitude as i64)
    } else {
        magnitude as i64
    })
}
pub(crate) fn timeline_boolean(value: &str) -> Result<bool, String> {
    match value {
        "1" | "t" | "T" | "true" | "TRUE" | "True" => Ok(true),
        "0" | "f" | "F" | "false" | "FALSE" | "False" => Ok(false),
        _ => Err("invalid syntax".into()),
    }
}
pub fn configure_command(mut command: clap::Command) -> clap::Command {
    for name in ["limit", "page"] {
        command = command.mut_arg(name, |argument| {
            argument.value_parser(clap::builder::ValueParser::new(timeline_integer))
        });
    }
    for name in ["json", "ndjson", "no_proxy"] {
        command = command.mut_arg(name, |argument| {
            argument.value_parser(clap::builder::ValueParser::new(timeline_boolean))
        });
    }
    command
}
pub fn argument_error(error: &clap::Error) -> Option<CommandError> {
    if let Some(error) = crate::argument_error(error) {
        return Some(error);
    }
    if !matches!(
        error.kind(),
        clap::error::ErrorKind::ValueValidation | clap::error::ErrorKind::InvalidValue
    ) {
        return None;
    }
    let argument = error.get(clap::error::ContextKind::InvalidArg)?.to_string();
    let value = error
        .get(clap::error::ContextKind::InvalidValue)?
        .to_string();
    let name = argument.split([' ', '[', '<']).next()?;
    if value.is_empty() && error.kind() == clap::error::ErrorKind::InvalidValue {
        return Some(CommandError::MessageText(format!(
            "flag needs an argument: {name}"
        )));
    }
    let (flag, parser, detail) = if argument.starts_with("--limit") {
        ("-l, --limit", "ParseInt", timeline_integer(&value).err()?)
    } else if argument.starts_with("--page") {
        ("-p, --page", "ParseInt", timeline_integer(&value).err()?)
    } else if argument.starts_with("--json") {
        ("-j, --json", "ParseBool", "invalid syntax".into())
    } else if argument.starts_with("--ndjson") {
        ("--ndjson", "ParseBool", "invalid syntax".into())
    } else if argument.starts_with("--no-proxy") {
        ("--no-proxy", "ParseBool", "invalid syntax".into())
    } else {
        return None;
    };
    Some(CommandError::MessageText(format!(
        "invalid argument {} for {} flag: strconv.{parser}: parsing {}: {detail}",
        crate::search::quote(&value),
        crate::search::quote(flag),
        crate::search::quote(&value)
    )))
}
