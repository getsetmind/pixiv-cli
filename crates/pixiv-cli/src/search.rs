use crate::CommandError;
use chrono::{DateTime, FixedOffset, NaiveDate};
use clap::Args;
use pixiv_sdk::pixiv::SearchArtworksRequest;
use pixiv_sdk::{
    Client,
    models::{Artwork, ArtworkKind},
    transport::Transport,
};
use std::collections::BTreeSet;
use std::io::Write;

#[derive(Args, Clone, Debug)]
pub struct SearchOptions {
    #[arg(long, default_value = "tag-partial", allow_hyphen_values = true)]
    pub search_by: String,
    #[arg(long, default_value = "date_desc", allow_hyphen_values = true)]
    pub sort: String,
    #[arg(long, default_value = "all", allow_hyphen_values = true)]
    pub content_type: String,
    #[arg(long, default_value = "", allow_hyphen_values = true)]
    pub rating: String,
    #[arg(long, short = 'l', allow_hyphen_values = true)]
    pub limit: Option<i64>,
    #[arg(long, short = 'p', allow_hyphen_values = true)]
    pub page: Option<i64>,
    #[arg(long, default_value = "all", allow_hyphen_values = true)]
    pub ai_mode: String,
    #[arg(long, default_value = "all", allow_hyphen_values = true)]
    pub aspect_ratio: String,
    #[arg(long, default_value = "all", allow_hyphen_values = true)]
    pub resolution: String,
    #[arg(long, default_value = "", allow_hyphen_values = true)]
    pub draw_tool: String,
    #[command(flatten)]
    pub dates: SearchDateOptions,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            search_by: "tag-partial".into(),
            sort: "date_desc".into(),
            content_type: "all".into(),
            rating: String::new(),
            limit: None,
            page: None,
            ai_mode: "all".into(),
            aspect_ratio: "all".into(),
            resolution: "all".into(),
            draw_tool: String::new(),
            dates: SearchDateOptions::default(),
        }
    }
}

impl SearchOptions {
    pub fn request(
        &self,
        word: &str,
        now: DateTime<FixedOffset>,
    ) -> Result<SearchArtworksRequest, CommandError> {
        let target = match self.search_by.as_str() {
            "tag-partial" => "partial_match_for_tags",
            "tag-exact" => "exact_match_for_tags",
            "title-caption" => "title_and_caption",
            "tag-title-caption" => "keyword",
            _ => {
                return Err(CommandError::Message(
                    "search-by must be one of tag-partial, tag-exact, title-caption, tag-title-caption",
                ));
            }
        };
        let filter = pixiv_app::search_filter::normalize_filter(&self.rating, &self.content_type)
            .map_err(|error| CommandError::Message(error.message()))?;
        if !matches!(self.ai_mode.as_str(), "all" | "exclude" | "only") {
            return Err(CommandError::Message(
                "ai-mode must be one of all, exclude, only",
            ));
        }
        if !matches!(
            self.aspect_ratio.as_str(),
            "all" | "landscape" | "portrait" | "square"
        ) {
            return Err(CommandError::Message(
                "aspect-ratio must be one of all, landscape, portrait, square",
            ));
        }
        if !matches!(self.resolution.as_str(), "all" | "high" | "medium" | "low") {
            return Err(CommandError::Message(
                "resolution must be one of all, high, medium, low",
            ));
        }
        let mut request = SearchArtworksRequest {
            word: word.into(),
            target: target.into(),
            sort: self.sort.clone(),
            content_type: filter.content_type.into(),
            ai_mode: self.ai_mode.clone(),
            aspect_ratio: self.aspect_ratio.clone(),
            resolution: self.resolution.clone(),
            tool: self.draw_tool.clone(),
            cursor_context: if filter.rating == "all" {
                String::new()
            } else {
                filter.cursor_context
            },
            ..Default::default()
        };
        self.dates.apply(&mut request, now)?;
        self.plan()?;
        Ok(request)
    }

    fn plan(&self) -> Result<SearchPlan, CommandError> {
        let limit = self.limit.unwrap_or_default();
        if limit < 0 {
            return Err(CommandError::Message(
                "limit must be zero or a positive integer",
            ));
        }
        if self.page.is_some_and(|page| page <= 0) {
            return Err(CommandError::Message("page must be a positive integer"));
        }
        let skip = if let Some(page) = self.page {
            if limit <= 0 {
                return Err(CommandError::Message(
                    "--page requires --limit to be a positive integer",
                ));
            }
            (page - 1).checked_mul(limit).ok_or(CommandError::Message(
                "page and limit overflow the logical result offset",
            ))?
        } else {
            0
        };
        Ok(SearchPlan {
            limit: limit as usize,
            skip: skip as usize,
            one_batch: self.limit.is_none(),
        })
    }
}

struct SearchPlan {
    limit: usize,
    skip: usize,
    one_batch: bool,
}

pub fn write_search_json<W: Write>(items: &[Artwork], out: &mut W) -> Result<(), CommandError> {
    let dtos: Vec<_> = items.iter().map(pixiv_sdk::dto::ArtworkDto::from).collect();
    let body = serde_json::to_string_pretty(&serde_json::json!({"illusts": dtos}))
        .map_err(|_| pixiv_sdk::Error::new(pixiv_sdk::Reason::LocalStateError, "output"))?;
    writeln!(out, "{}", crate::go_json_escape(body))?;
    Ok(())
}

pub async fn collect_search<T: Transport>(
    client: &Client<T>,
    request: SearchArtworksRequest,
    options: &SearchOptions,
) -> Result<Vec<Artwork>, CommandError> {
    let mut items = vec![];
    visit_search(client, request, options, |batch| {
        items.extend(batch);
        Ok(())
    })
    .await?;
    Ok(items)
}

pub async fn artwork_search<T: Transport, W: Write>(
    client: &Client<T>,
    request: SearchArtworksRequest,
    options: &SearchOptions,
    mode: crate::DetailOutput,
    out: &mut W,
) -> Result<(), CommandError> {
    let word = request.word.clone();
    if mode == crate::DetailOutput::Json {
        let items = collect_search(client, request, options).await?;
        return write_search_json(&items, out);
    }
    let local = pixiv_app::search_filter::normalize_filter(&options.rating, &options.content_type)
        .map_err(|error| CommandError::Message(error.message()))?
        .rating
        != "all";
    let mut heading_written = false;
    let mut present = |items: Vec<Artwork>| -> Result<(), CommandError> {
        if mode == crate::DetailOutput::Human && !heading_written {
            writeln!(out, "illustrations for {}", quote(&word))?;
            heading_written = true;
        }
        for item in items {
            if mode == crate::DetailOutput::Ndjson {
                let record = pixiv_record::from_artwork(&item)
                    .map_err(|error| CommandError::Message(error.message()))?;
                let encoded = serde_json::to_string(&record).map_err(|_| {
                    pixiv_sdk::Error::new(pixiv_sdk::Reason::LocalStateError, "output")
                })?;
                writeln!(out, "{}", crate::go_json_escape(encoded))?;
            } else {
                let url = if item.id > 0 {
                    format!("https://www.pixiv.net/artworks/{}", item.id)
                } else {
                    String::new()
                };
                writeln!(out, "{url}")?;
                let tags = item
                    .tags
                    .iter()
                    .map(|tag| tag.name.as_str())
                    .collect::<Vec<_>>()
                    .join(",");
                writeln!(
                    out,
                    "{} {} by {} bookmarks:{} views:{} tags:{}",
                    item.id,
                    quote(&item.title),
                    item.user.name,
                    item.total_bookmarks,
                    item.total_views,
                    tags
                )?;
            }
        }
        Ok(())
    };
    if local {
        present(collect_search(client, request, options).await?)
    } else {
        visit_search(client, request, options, present).await
    }
}

fn quote(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        if ch != ' '
            && unicode_general_category::get_general_category(ch)
                == unicode_general_category::GeneralCategory::SpaceSeparator
        {
            out.push_str(&format!("\\u{:04x}", ch as u32));
        } else {
            out.push_str(&crate::safe_line(&ch.to_string()));
        }
    }
    out.push('"');
    out
}

async fn visit_search<T: Transport>(
    client: &Client<T>,
    mut request: SearchArtworksRequest,
    options: &SearchOptions,
    mut consume: impl FnMut(Vec<Artwork>) -> Result<(), CommandError>,
) -> Result<(), CommandError> {
    let filter = pixiv_app::search_filter::normalize_filter(&options.rating, &options.content_type)
        .map_err(|error| CommandError::Message(error.message()))?;
    let local_filter = filter.rating != "all";
    let plan = options.plan()?;
    let mut skip = plan.skip;
    let mut returned_count = 0;
    let mut cursors = BTreeSet::new();
    loop {
        if !cursors.insert(request.cursor.as_str().to_owned()) {
            let prefix = if local_filter {
                "pagination stream 0 cursor repeated"
            } else {
                "pagination cursor repeated"
            };
            return Err(CommandError::MessageText(format!(
                "{prefix}: {}",
                request.cursor.as_str()
            )));
        }
        let page = client.search_artworks(request.clone()).await?;
        let mut batch: Vec<_> = page
            .items
            .into_iter()
            .filter(|artwork| {
                if !local_filter {
                    return true;
                }
                let kind = match artwork.kind {
                    ArtworkKind::Illust => "illust",
                    ArtworkKind::Manga => "manga",
                    ArtworkKind::Ugoira => "ugoira",
                    ArtworkKind::Unknown => "unknown",
                };
                filter.matches(artwork.x_restrict, kind)
            })
            .collect();
        let consumed = skip.min(batch.len());
        skip -= consumed;
        batch.drain(..consumed);
        if plan.limit > 0 {
            batch.truncate(plan.limit - returned_count);
        }
        let returned = !batch.is_empty();
        returned_count += batch.len();
        if returned {
            consume(batch)?;
        }
        if (plan.limit > 0 && returned_count >= plan.limit)
            || (plan.one_batch && skip == 0 && returned)
            || page.next.is_zero()
        {
            return Ok(());
        }
        request.cursor = page.next;
    }
}

#[derive(Args, Clone, Debug, Default)]
pub struct SearchDateOptions {
    #[arg(long, default_value = "", allow_hyphen_values = true)]
    pub period: String,
    #[arg(long, default_value = "", allow_hyphen_values = true)]
    pub start_date: String,
    #[arg(long, default_value = "", allow_hyphen_values = true)]
    pub end_date: String,
}

impl SearchDateOptions {
    pub fn apply(
        &self,
        request: &mut SearchArtworksRequest,
        now: DateTime<FixedOffset>,
    ) -> Result<(), CommandError> {
        let period = match self.period.as_str() {
            "" => "",
            "day" => "within_last_day",
            "week" => "within_last_week",
            "month" => "within_last_month",
            "half-year" => "within_half_year",
            "year" => "within_year",
            _ => {
                return Err(CommandError::Message(
                    "period must be one of day, week, month, half-year, year",
                ));
            }
        };
        let start = self.start_date.trim();
        let end = self.end_date.trim();
        if !period.is_empty() && (!start.is_empty() || !end.is_empty()) {
            return Err(CommandError::Message(
                "period cannot be combined with start-date or end-date",
            ));
        }
        if (!start.is_empty() && !valid_date(start)) || (!end.is_empty() && !valid_date(end)) {
            return Err(CommandError::Message(
                "start-date and end-date must use YYYY-MM-DD",
            ));
        }
        if !start.is_empty() && !end.is_empty() && start > end {
            return Err(CommandError::Message(
                "start-date cannot be later than end-date",
            ));
        }
        if let Some(range) = pixiv_app::dates::quick_date_range(period, now)
            .map_err(|_| CommandError::Message("date range overflow"))?
        {
            request.duration.clear();
            request.start_date = range.start_date;
            request.end_date = range.end_date;
        } else {
            request.duration = period.into();
            request.start_date = start.into();
            request.end_date = end.into();
        }
        Ok(())
    }
}

fn valid_date(raw: &str) -> bool {
    raw.len() == 10
        && raw.bytes().enumerate().all(|(index, byte)| {
            if index == 4 || index == 7 {
                byte == b'-'
            } else {
                byte.is_ascii_digit()
            }
        })
        && NaiveDate::parse_from_str(raw, "%Y-%m-%d").is_ok()
}
