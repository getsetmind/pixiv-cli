use crate::{CommandError, DetailOutput};
use pixiv_app::{execution::Execution, lifecycle::Context};
use pixiv_sdk::{Client, Error, Reason, cursor::Cursor, transport::Transport};
use std::io::Write;

#[derive(clap::Args, Clone, Debug)]
pub struct NovelSeriesOptions {
    #[arg(long = "type", short = 't')]
    pub entity: Option<String>,
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
enum Listing {
    Novel(crate::novel_list::Listing),
    Artwork(crate::artwork_list::Listing),
}

impl NovelSeriesOptions {
    pub fn resolve_source<R: std::io::Read>(
        &mut self,
        input: &mut R,
        terminal: bool,
    ) -> Result<(), CommandError> {
        if self.sources.is_empty() {
            self.sources.push(crate::search::read_text_value(
                input,
                terminal,
                "usage: pixiv series [options] SERIES_ID_OR_URL",
                "stdin series ID is not valid UTF-8",
            )?);
        }
        self.validate_arguments()
    }

    pub fn validate_arguments(&self) -> Result<(), CommandError> {
        if self.sources.len() != 1 {
            return Err(CommandError::Message(
                "usage: pixiv series [options] SERIES_ID_OR_URL",
            ));
        }
        Ok(())
    }
    fn listing(&self) -> Result<Listing, CommandError> {
        self.validate_arguments()?;
        let entity = self
            .entity
            .as_deref()
            .ok_or(CommandError::Message("--type is required for series"))?;
        if !matches!(entity, "artwork" | "novel") {
            return Err(CommandError::Message("type must be one of artwork, novel"));
        }
        let invalid = |detail| Error::new(Reason::InvalidArgument, "series").with_detail(detail);
        let source = self.sources[0].trim();
        let id = if let Ok(id) = source.parse::<i64>() {
            if id <= 0 {
                return Err(invalid("id must be a positive integer").into());
            }
            id
        } else {
            let reference = pixiv_sdk::reference::parse_url(source)
                .map_err(|_| invalid("input must be a positive ID or a supported Pixiv URL"))?;
            let kind = reference.kind.as_str();
            if !matches!(kind, "novel_series" | "artwork_series") {
                return Err(invalid("URL kind is not allowed for this command").into());
            }
            if (entity == "novel") != (kind == "novel_series") {
                return Err(invalid("URL namespace conflicts with the selected type").into());
            }
            reference.id
        };
        let plan = crate::search::SearchOptions {
            limit: self.limit,
            page: self.page,
            ..Default::default()
        }
        .plan()?;
        if entity == "artwork" {
            return Ok(Listing::Artwork(crate::artwork_list::Listing {
                source: crate::artwork_list::Source::Series(
                    pixiv_sdk::pixiv::ArtworkSeriesRequest {
                        series_id: id,
                        cursor: Cursor::default(),
                    },
                ),
                heading: format!("artworks in series {id}"),
                plan,
            }));
        }
        Ok(Listing::Novel(crate::novel_list::Listing {
            source: crate::novel_list::Source::Series(pixiv_sdk::pixiv::NovelSeriesRequest {
                series_id: id,
                cursor: Cursor::default(),
            }),
            heading: String::new(),
            plan,
        }))
    }
    pub fn validate(&self) -> Result<(), CommandError> {
        self.listing().map(|_| ())
    }
    pub fn output_mode(
        &self,
        configured_json: bool,
        terminal: bool,
    ) -> Result<DetailOutput, CommandError> {
        if self.ndjson && self.json.is_some() {
            return Err(CommandError::Usage(
                "--ndjson cannot be used with --json".into(),
            ));
        }
        Ok(if self.ndjson {
            DetailOutput::Ndjson
        } else if self.json.unwrap_or(configured_json) {
            DetailOutput::Json
        } else if self.json.is_none() && !terminal {
            DetailOutput::Ndjson
        } else {
            DetailOutput::Human
        })
    }
}
pub async fn novel_series<T: Transport, W: Write>(
    client: &Client<T>,
    options: &NovelSeriesOptions,
    mode: DetailOutput,
    out: &mut W,
) -> Result<(), CommandError> {
    let spool = match options.listing()? {
        Listing::Novel(listing) => crate::novel_list::attempt(client, &listing, mode, out).await?,
        Listing::Artwork(listing) => {
            crate::artwork_list::attempt(client, &listing, mode, out).await?
        }
    };
    if let Some(mut spool) = spool {
        spool.commit(out)?;
    }
    Ok(())
}
pub async fn saved_novel_series<T: Transport + 'static, W: Write + Send + 'static>(
    execution: &Execution<T>,
    context: &Context,
    options: NovelSeriesOptions,
    proxy: Option<&str>,
    mode: DetailOutput,
    output: W,
) -> Result<(), CommandError> {
    match options.listing()? {
        Listing::Novel(listing) => {
            crate::novel_list::saved(execution, context, listing, proxy, mode, output).await
        }
        Listing::Artwork(listing) => {
            crate::artwork_list::saved(execution, context, listing, proxy, mode, output).await
        }
    }
}
