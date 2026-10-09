use crate::{CommandError, DetailOutput, user_works::UserWorksOptions};
use pixiv_app::{execution::Execution, lifecycle::Context};
use pixiv_sdk::{Client, Error, Reason, cursor::Cursor, transport::Transport};
use std::io::{Read, Write};

#[derive(clap::Args, Clone, Debug, Default)]
pub struct MyPixivWorksOptions {
    #[command(flatten)]
    pub listing: UserWorksOptions,
    #[arg(long = "type", short = 't')]
    pub entity: Option<String>,
}
#[derive(Clone, Debug)]
pub enum MyPixiv {
    Users(UserWorksOptions),
    Works(MyPixivWorksOptions),
}
impl MyPixiv {
    pub fn options(&self) -> &UserWorksOptions {
        match self {
            Self::Users(options) => options,
            Self::Works(options) => &options.listing,
        }
    }
    pub fn resolve_source<R: Read>(
        &mut self,
        input: &mut R,
        terminal: bool,
    ) -> Result<(), CommandError> {
        self.validate_arguments()?;
        if let Self::Works(options) = self {
            crate::user_works::resolve_optional_source(&mut options.listing, input, terminal)?;
        }
        Ok(())
    }
    pub fn validate_arguments(&self) -> Result<(), CommandError> {
        if match self {
            Self::Users(options) => !options.sources.is_empty(),
            Self::Works(options) => options.listing.sources.len() > 1,
        } {
            return Err(CommandError::Message(match self {
                Self::Users(_) => "usage: pixiv mypixiv users",
                Self::Works(_) => "usage: pixiv mypixiv works [USER_ID] --type artwork|manga|novel",
            }));
        }
        Ok(())
    }
    fn selection(&self) -> Result<(String, Option<i64>), CommandError> {
        let Self::Works(options) = self else {
            return Ok((String::new(), None));
        };
        let kind = match options.entity.as_deref().unwrap_or_default() {
            "artwork" => "illust",
            kind => kind,
        };
        let source = options.listing.sources.first();
        let invalid = |detail| {
            CommandError::from(
                Error::new(Reason::InvalidArgument, "mypixiv works").with_detail(detail),
            )
        };
        if !(matches!(kind, "illust" | "novel") || source.is_some() && kind == "manga") {
            return Err(invalid(if source.is_some() {
                "type with USER_ID must be one of: artwork, manga, novel"
            } else {
                "type without USER_ID must be one of: artwork, novel"
            }));
        }
        let id = source
            .map(|source| {
                source
                    .parse::<i64>()
                    .ok()
                    .filter(|id| *id > 0)
                    .ok_or_else(|| invalid("user_id must be a positive integer"))
            })
            .transpose()?;
        Ok((kind.into(), id))
    }
    fn plan(&self) -> Result<crate::search::SearchPlan, CommandError> {
        crate::search::SearchOptions {
            limit: self.options().limit,
            page: self.options().page,
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
        match self {
            Self::Users(options) => {
                crate::user_relationships::UserRelationships::MyPixiv(options.clone())
                    .output_mode(configured_json, terminal)
            }
            Self::Works(options) => crate::user_works::UserWorks::Novels(options.listing.clone())
                .output_mode(configured_json, terminal),
        }
    }
    fn listing(&self) -> Result<Listing, CommandError> {
        self.validate()?;
        let (kind, id) = self.selection()?;
        let plan = self.plan()?;
        Ok(match self {
            Self::Users(options) => Listing::Users(
                crate::user_relationships::UserRelationships::MyPixiv(options.clone()),
            ),
            Self::Works(_) if kind == "novel" => Listing::Novel(crate::novel_list::Listing {
                source: if let Some(user_id) = id {
                    crate::novel_list::Source::MyPixivUser(pixiv_sdk::pixiv::UserNovelsRequest {
                        user_id,
                        cursor: Cursor::default(),
                    })
                } else {
                    crate::novel_list::Source::MyPixiv(pixiv_sdk::pixiv::MyPixivNovelsRequest {
                        cursor: Cursor::default(),
                    })
                },
                heading: id
                    .map(|id| format!("novels by {id}"))
                    .unwrap_or_else(|| "MyPixiv novels".into()),
                plan,
            }),
            Self::Works(_) => Listing::Artwork(crate::artwork_list::Listing {
                source: if let Some(user_id) = id {
                    crate::artwork_list::Source::MyPixivUser(
                        pixiv_sdk::pixiv::UserArtworksRequest {
                            user_id,
                            kind,
                            cursor: Cursor::default(),
                        },
                    )
                } else {
                    crate::artwork_list::Source::MyPixiv(pixiv_sdk::pixiv::MyPixivArtworksRequest {
                        cursor: Cursor::default(),
                    })
                },
                heading: id
                    .map(|id| format!("artworks by {id}"))
                    .unwrap_or_else(|| "MyPixiv artworks".into()),
                plan,
            }),
        })
    }
}
enum Listing {
    Artwork(crate::artwork_list::Listing),
    Novel(crate::novel_list::Listing),
    Users(crate::user_relationships::UserRelationships),
}
pub async fn mypixiv<T: Transport, W: Write>(
    client: &Client<T>,
    options: &MyPixiv,
    mode: DetailOutput,
    out: &mut W,
) -> Result<(), CommandError> {
    let spool = match options.listing()? {
        Listing::Artwork(listing) => {
            crate::artwork_list::attempt(client, &listing, mode, out).await?
        }
        Listing::Novel(listing) => crate::novel_list::attempt(client, &listing, mode, out).await?,
        Listing::Users(listing) => {
            return crate::user_relationships::user_relationships(client, &listing, mode, out)
                .await;
        }
    };
    if let Some(mut spool) = spool {
        spool.commit(out)?;
    }
    Ok(())
}
pub async fn saved_mypixiv<T: Transport + 'static, W: Write + Send + 'static>(
    execution: &Execution<T>,
    context: &Context,
    options: MyPixiv,
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
        Listing::Users(listing) => {
            crate::user_relationships::saved_user_relationships(
                execution, context, listing, proxy, mode, output,
            )
            .await
        }
    }
}
