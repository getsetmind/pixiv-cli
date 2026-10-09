use crate::{CommandError, DetailOutput, json_spool::JsonSpool};
use pixiv_app::{execution::Execution, lifecycle::Context};
use pixiv_sdk::{Client, cursor::Cursor, pixiv::ArtworkRankingRequest, transport::Transport};
use std::io::Write;

#[derive(clap::Args, Clone, Debug)]
pub struct RankingOptions {
    #[arg(long = "type", short = 't', default_value = "artwork")]
    pub entity: String,
    #[arg(long, default_value = "day")]
    pub mode: String,
    #[arg(long)]
    pub date: Option<String>,
    #[arg(long, short = 'l', allow_hyphen_values = true)]
    pub limit: Option<i64>,
    #[arg(long, short = 'p', allow_hyphen_values = true)]
    pub page: Option<i64>,
    #[arg(long,short='j',num_args=0..=1,require_equals=true,default_missing_value="true")]
    pub json: Option<bool>,
    #[arg(long,action=clap::ArgAction::Set,num_args=0..=1,require_equals=true,default_missing_value="true",default_value="false")]
    pub ndjson: bool,
    #[arg(num_args=0..)]
    pub query: Vec<String>,
}
impl RankingOptions {
    pub fn validate_arguments(&self) -> Result<(), CommandError> {
        if self.query.is_empty() {
            Ok(())
        } else {
            Err(CommandError::Message("usage: pixiv ranking [options]"))
        }
    }
    fn plan(&self) -> Result<crate::search::SearchPlan, CommandError> {
        self.validate_arguments()?;
        if !matches!(self.entity.as_str(), "artwork" | "novel") {
            return Err(CommandError::Message("type must be one of: artwork, novel"));
        }
        if self.entity == "novel" && self.date.is_some() {
            return Err(CommandError::Message(
                "--date is only supported when --type artwork",
            ));
        }
        crate::search::SearchOptions {
            limit: self.limit,
            page: self.page,
            ..Default::default()
        }
        .plan()
    }
    pub fn validate(&self) -> Result<(), CommandError> {
        self.plan().map(|_| ())
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

pub async fn ranking<T: Transport, W: Write>(
    client: &Client<T>,
    options: &RankingOptions,
    mode: DetailOutput,
    out: &mut W,
) -> Result<(), CommandError> {
    if let Some(mut spool) = attempt(client, options, mode, out).await? {
        spool.commit(out)?;
    }
    Ok(())
}

pub async fn saved_ranking<T: Transport + 'static, W: Write + Send + 'static>(
    execution: &Execution<T>,
    context: &Context,
    options: RankingOptions,
    proxy: Option<&str>,
    mode: DetailOutput,
    output: W,
) -> Result<(), CommandError> {
    options.validate()?;
    if options.entity == "novel" {
        return crate::novel_list::saved(
            execution,
            context,
            crate::novel_list::Listing {
                source: crate::novel_list::Source::Ranking(pixiv_sdk::pixiv::NovelRankingRequest {
                    mode: options.mode.clone(),
                    cursor: Cursor::default(),
                }),
                heading: format!("{} novel ranking", options.mode),
                plan: options.plan()?,
            },
            proxy,
            mode,
            output,
        )
        .await;
    }

    crate::artwork_list::saved(
        execution,
        context,
        artwork_listing(&options)?,
        proxy,
        mode,
        output,
    )
    .await
}

fn artwork_listing(options: &RankingOptions) -> Result<crate::artwork_list::Listing, CommandError> {
    Ok(crate::artwork_list::Listing {
        source: crate::artwork_list::Source::Ranking(ArtworkRankingRequest {
            mode: options.mode.clone(),
            date: options.date.clone().unwrap_or_default(),
            cursor: Cursor::default(),
        }),
        heading: format!("{} ranking", options.mode),
        plan: options.plan()?,
    })
}
async fn attempt<T: Transport, W: Write>(
    client: &Client<T>,
    options: &RankingOptions,
    mode: DetailOutput,
    out: &mut W,
) -> Result<Option<JsonSpool>, CommandError> {
    let plan = options.plan()?;
    if options.entity == "novel" {
        return crate::novel_list::attempt(
            client,
            &crate::novel_list::Listing {
                source: crate::novel_list::Source::Ranking(pixiv_sdk::pixiv::NovelRankingRequest {
                    mode: options.mode.clone(),
                    cursor: Cursor::default(),
                }),
                heading: format!("{} novel ranking", options.mode),
                plan,
            },
            mode,
            out,
        )
        .await;
    }
    crate::artwork_list::attempt(client, &artwork_listing(options)?, mode, out).await
}
