use crate::{CommandError, DetailOutput};
use pixiv_app::{execution::Execution, lifecycle::Context};
use pixiv_sdk::{Client, Error, Reason, cursor::Cursor, transport::Transport};
use std::{
    io::{Read, Write},
    sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    },
};

#[derive(clap::Args, Clone, Debug, Default)]
pub struct UserWorksOptions {
    #[arg(long, short = 'l', allow_hyphen_values = true)]
    pub limit: Option<i64>,
    #[arg(long, short = 'p', allow_hyphen_values = true)]
    pub page: Option<i64>,
    #[arg(long,short='j',num_args=0..=1,require_equals=true,default_missing_value="true")]
    pub json: Option<bool>,
    #[arg(long,action=clap::ArgAction::Set,num_args=0..=1,require_equals=true,default_missing_value="true",default_value="false")]
    pub ndjson: bool,
    #[arg(num_args=0..)]
    pub sources: Vec<String>,
}
#[derive(clap::Args, Clone, Debug)]
pub struct UserArtworksOptions {
    #[command(flatten)]
    pub listing: UserWorksOptions,
    #[arg(long = "type", short = 't', default_value = "illustration")]
    pub kind: String,
}
#[derive(Clone, Debug)]
pub enum UserWorks {
    Artworks(UserArtworksOptions),
    Novels(UserWorksOptions),
}
impl UserWorks {
    pub fn options(&self) -> &UserWorksOptions {
        match self {
            Self::Artworks(options) => &options.listing,
            Self::Novels(options) => options,
        }
    }
    pub fn name(&self) -> &'static str {
        match self {
            Self::Artworks(_) => "artworks",
            Self::Novels(_) => "novels",
        }
    }
    pub fn resolve_source<R: Read>(
        &mut self,
        input: &mut R,
        terminal: bool,
    ) -> Result<(), CommandError> {
        self.validate_arguments()?;
        let options = match self {
            Self::Artworks(options) => &mut options.listing,
            Self::Novels(options) => options,
        };
        if options.sources.is_empty() && !terminal {
            let mut bytes = vec![];
            input
                .read_to_end(&mut bytes)
                .map_err(|error| CommandError::Usage(format!("read stdin value: {error}")))?;
            if bytes.ends_with(b"\r\n") {
                bytes.truncate(bytes.len() - 2);
            } else if bytes.ends_with(b"\n") {
                bytes.pop();
            }
            if !bytes.is_empty() {
                options
                    .sources
                    .push(String::from_utf8_lossy(&bytes).into_owned());
            }
        }
        Ok(())
    }
    pub fn validate_arguments(&self) -> Result<(), CommandError> {
        if self.options().sources.len() > 1 {
            return Err(CommandError::Message(match self {
                Self::Artworks(_) => "usage: pixiv user artworks [options] [USER_ID]",
                Self::Novels(_) => "usage: pixiv user novels [options] [USER_ID]",
            }));
        }
        Ok(())
    }
    fn user_id(&self) -> Result<i64, CommandError> {
        let Some(source) = self.options().sources.first() else {
            return Ok(0);
        };
        let source = source.trim();
        let invalid = |detail| {
            CommandError::LabeledSdk(
                "user_id",
                Error::new(Reason::InvalidArgument, format!("user {}", self.name()))
                    .with_detail(detail),
            )
        };
        if let Ok(id) = source.parse::<i64>() {
            return if id > 0 {
                Ok(id)
            } else {
                Err(invalid("id must be a positive integer"))
            };
        }
        pixiv_sdk::reference::parse_url(source)
            .map_err(|_| invalid("input must be a positive ID or a supported Pixiv URL"))?;
        Err(invalid("URL kind is not allowed for this command"))
    }
    pub fn validate(&self) -> Result<(), CommandError> {
        self.validate_arguments()?;
        if let Self::Artworks(options) = self
            && !matches!(
                options.kind.as_str(),
                "" | "illustration" | "illust" | "manga" | "ugoira"
            )
        {
            return Err(Error::new(Reason::InvalidArgument, "user artworks")
                .with_detail("type must be one of illustration, illust, manga, or ugoira")
                .into());
        }
        self.user_id()?;
        self.plan()?;
        Ok(())
    }
    fn plan(&self) -> Result<crate::search::SearchPlan, CommandError> {
        crate::search::SearchOptions {
            limit: self.options().limit,
            page: self.options().page,
            ..Default::default()
        }
        .plan()
    }
    pub fn output_mode(
        &self,
        configured_json: bool,
        terminal: bool,
    ) -> Result<DetailOutput, CommandError> {
        let options = self.options();
        if options.ndjson && options.json.is_some() {
            return Err(CommandError::Usage(
                "--ndjson cannot be used with --json".into(),
            ));
        }
        Ok(if options.ndjson {
            DetailOutput::Ndjson
        } else if options.json.unwrap_or(configured_json) {
            DetailOutput::Json
        } else if options.json.is_none() && !terminal {
            DetailOutput::Ndjson
        } else {
            DetailOutput::Human
        })
    }
    fn listing(&self) -> Result<Listing, CommandError> {
        self.validate()?;
        let id = self.user_id()?;
        let identity = Arc::new(AtomicI64::new(id));
        let plan = self.plan()?;
        Ok(match self {
            Self::Artworks(options) => Listing::Artwork(crate::artwork_list::Listing {
                source: crate::artwork_list::Source::User(
                    pixiv_sdk::pixiv::UserArtworksRequest {
                        user_id: id,
                        kind: options.kind.clone(),
                        cursor: Cursor::default(),
                    },
                    identity,
                ),
                heading: String::new(),
                plan,
            }),
            Self::Novels(_) => Listing::Novel(crate::novel_list::Listing {
                source: crate::novel_list::Source::User(
                    pixiv_sdk::pixiv::UserNovelsRequest {
                        user_id: id,
                        cursor: Cursor::default(),
                    },
                    identity,
                ),
                heading: format!("novels by {id}"),
                plan,
            }),
        })
    }
}
enum Listing {
    Artwork(crate::artwork_list::Listing),
    Novel(crate::novel_list::Listing),
}
pub(crate) fn current_target<T: Transport>(
    client: &Client<T>,
    identity: &AtomicI64,
) -> Result<i64, CommandError> {
    let mut id = identity.load(Ordering::Acquire);
    if id == 0 {
        id = client.user_id();
        if id <= 0 {
            return Err(Error::new(Reason::Unauthorized, "CurrentUser")
                .with_detail("cannot determine current user id")
                .into());
        }
        identity.store(id, Ordering::Release);
    }
    Ok(id)
}
pub async fn user_works<T: Transport, W: Write>(
    client: &Client<T>,
    options: &UserWorks,
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
pub async fn saved_user_works<T: Transport + 'static, W: Write + Send + 'static>(
    execution: &Execution<T>,
    context: &Context,
    options: UserWorks,
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
