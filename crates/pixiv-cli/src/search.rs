use crate::CommandError;
use chrono::{DateTime, FixedOffset, NaiveDate};
use clap::Args;
use pixiv_app::{
    execution::Execution, facade::UseOutcome, lifecycle::Context, scheduler::SchedulerError,
};
use pixiv_sdk::pixiv::SearchArtworksRequest;
use pixiv_sdk::{
    Client,
    models::{Artwork, ArtworkKind},
    transport::Transport,
};
use std::io::Write;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

#[derive(Args, Clone, Debug)]
pub struct SearchInput {
    #[arg(required = true, num_args = 1..)]
    pub query: Vec<String>,
    #[arg(long, short = 'j', num_args = 0..=1, require_equals = true, default_missing_value = "true")]
    pub json: Option<bool>,
    #[arg(long, action = clap::ArgAction::Set, num_args = 0..=1, require_equals = true, default_missing_value = "true", default_value = "false")]
    pub ndjson: bool,
}

impl SearchInput {
    pub fn word(&self) -> String {
        self.query.join(" ")
    }

    pub fn machine_output(&self) -> bool {
        self.json.is_some() || self.ndjson
    }

    pub fn output_mode(
        &self,
        configured_json: bool,
        terminal: bool,
    ) -> Result<crate::DetailOutput, CommandError> {
        if self.ndjson && self.json.is_some() {
            return Err(CommandError::Usage("--ndjson cannot be used with --json"));
        }
        if self.ndjson {
            Ok(crate::DetailOutput::Ndjson)
        } else if self.json.unwrap_or(configured_json) {
            Ok(crate::DetailOutput::Json)
        } else if self.json.is_none() && !terminal {
            Ok(crate::DetailOutput::Ndjson)
        } else {
            Ok(crate::DetailOutput::Human)
        }
    }
}

struct SearchWriter<W> {
    output: Arc<Mutex<W>>,
    committed: Arc<AtomicBool>,
}

impl<W: Write> Write for SearchWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.committed.store(true, Ordering::Release);
        self.output
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .write(bytes)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.output
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .flush()
    }
}

pub async fn saved_artwork_search<T: Transport + 'static, W: Write + Send + 'static>(
    execution: &Execution<T>,
    context: &Context,
    request: SearchArtworksRequest,
    options: SearchOptions,
    proxy: Option<&str>,
    mode: crate::DetailOutput,
    output: W,
) -> Result<(), CommandError> {
    options.validate_bookmark_strategy()?;
    let output = Arc::new(Mutex::new(output));
    let terminal_error = Arc::new(Mutex::new(None));
    let retained = terminal_error.clone();
    let result = execution
        .use_client(
            Some(context),
            0,
            proxy,
            Some(Arc::new(move |_, client| {
                let request = request.clone();
                let options = options.clone();
                let committed = Arc::new(AtomicBool::new(false));
                let mut writer = SearchWriter {
                    output: output.clone(),
                    committed: committed.clone(),
                };
                let retained = retained.clone();
                Box::pin(async move {
                    let error =
                        match artwork_search(&client, request, &options, mode, &mut writer).await {
                            Ok(()) => None,
                            Err(CommandError::Sdk(error)) => Some(SchedulerError::from(error)),
                            Err(CommandError::App(error)) => Some(error),
                            Err(error) => {
                                let message = error.to_string();
                                *retained.lock().unwrap_or_else(|error| error.into_inner()) =
                                    Some(error);
                                Some(SchedulerError::Message(message))
                            }
                        };
                    UseOutcome {
                        committed: committed.load(Ordering::Acquire),
                        error,
                    }
                })
            })),
        )
        .await;
    if let Some(error) = terminal_error
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take()
    {
        return Err(error);
    }
    result.map_err(Into::into)
}

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
    #[arg(long, allow_hyphen_values = true)]
    pub bookmark_min: Option<i64>,
    #[arg(long, allow_hyphen_values = true)]
    pub bookmark_max: Option<i64>,
    #[arg(long, allow_hyphen_values = true)]
    pub bookmark_strategy: Option<String>,
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
            bookmark_min: None,
            bookmark_max: None,
            bookmark_strategy: None,
            ai_mode: "all".into(),
            aspect_ratio: "all".into(),
            resolution: "all".into(),
            draw_tool: String::new(),
            dates: SearchDateOptions::default(),
        }
    }
}

impl SearchOptions {
    pub fn validate_bookmark_strategy(&self) -> Result<(), CommandError> {
        if let Some(strategy) = &self.bookmark_strategy {
            if self.bookmark_min.is_none() && self.bookmark_max.is_none() {
                return Err(CommandError::Message(
                    "--bookmark-strategy requires --bookmark-min or --bookmark-max",
                ));
            }
            if !matches!(
                strategy.as_str(),
                "auto" | "local" | "best_effort" | "server"
            ) {
                return Err(CommandError::Message(
                    "bookmark-strategy must be one of auto, local, best_effort, server",
                ));
            }
        }
        Ok(())
    }

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
        if self.bookmark_min.is_some_and(|value| value < 0) {
            return Err(CommandError::Message(
                "bookmark-min must be greater than or equal to zero",
            ));
        }
        if self.bookmark_max.is_some_and(|value| value < 0) {
            return Err(CommandError::Message(
                "bookmark-max must be greater than or equal to zero",
            ));
        }
        if self
            .bookmark_min
            .zip(self.bookmark_max)
            .is_some_and(|(min, max)| min > max)
        {
            return Err(CommandError::Message(
                "bookmark-min cannot be greater than bookmark-max",
            ));
        }
        request.bookmark_min = self.bookmark_min;
        request.bookmark_max = self.bookmark_max;
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
    let local = pixiv_app::search_filter::normalize_filter(&options.rating, &options.content_type)
        .map_err(|error| CommandError::Message(error.message()))?
        .rating
        != "all";
    if options.bookmark_min.is_some()
        || options.bookmark_max.is_some()
        || options.bookmark_strategy.is_some()
    {
        return bookmark_search(client, request, options, mode, out).await;
    }
    if mode == crate::DetailOutput::Json {
        if local {
            let items = collect_search(client, request, options).await?;
            return write_search_json(&items, out);
        }
        let mut spool = crate::json_spool::JsonSpool::new()?;
        visit_search(client, request, options, |batch| spool.append(&batch)).await?;
        return spool.commit(out);
    }
    let mut heading_written = false;
    let mut present = |items: Vec<Artwork>| -> Result<(), CommandError> {
        present_search(&word, &items, mode, &mut heading_written, out)
    };
    if local {
        present(collect_search(client, request, options).await?)
    } else {
        visit_search(client, request, options, present).await
    }
}

fn present_search<W: Write>(
    word: &str,
    items: &[Artwork],
    mode: crate::DetailOutput,
    heading_written: &mut bool,
    out: &mut W,
) -> Result<(), CommandError> {
    if mode == crate::DetailOutput::Human && !*heading_written {
        writeln!(out, "illustrations for {}", quote(word))?;
        *heading_written = true;
    }
    for item in items {
        if mode == crate::DetailOutput::Ndjson {
            let record = pixiv_record::from_artwork(item)
                .map_err(|error| CommandError::Message(error.message()))?;
            let encoded = serde_json::to_string(&record)
                .map_err(|_| pixiv_sdk::Error::new(pixiv_sdk::Reason::LocalStateError, "output"))?;
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
}

async fn bookmark_search<T: Transport, W: Write>(
    client: &Client<T>,
    mut request: SearchArtworksRequest,
    options: &SearchOptions,
    mode: crate::DetailOutput,
    out: &mut W,
) -> Result<(), CommandError> {
    if options.bookmark_min.is_none() && options.bookmark_max.is_none() {
        return Err(CommandError::Message(
            "--bookmark-strategy requires --bookmark-min or --bookmark-max",
        ));
    }
    let strategy = match options.bookmark_strategy.as_deref().unwrap_or("auto") {
        "auto" | "local" => "local",
        "best_effort" => "best_effort",
        "server" => {
            return Err(pixiv_sdk::Error::new(
                pixiv_sdk::Reason::UpstreamUnavailable,
                "SearchArtworks",
            )
            .with_detail(
                "server bookmark strategy requires verified premium membership and evidence",
            )
            .into());
        }
        _ => {
            return Err(CommandError::Message(
                "bookmark-strategy must be one of auto, local, best_effort, server",
            ));
        }
    };
    let context = pixiv_app::search_filter::bookmark_context(
        options.bookmark_min,
        options.bookmark_max,
        strategy,
    );
    request.cursor_context =
        pixiv_app::search_filter::combine_contexts(&[&request.cursor_context, &context]);
    if strategy == "local" {
        request.bookmark_min = None;
        request.bookmark_max = None;
    }
    let filter = pixiv_app::search_filter::normalize_filter(&options.rating, &options.content_type)
        .map_err(|error| CommandError::Message(error.message()))?;
    let plan = options.plan()?;
    let initial = request.cursor.clone();
    let page = pixiv_app::pagination::collect_pages(
        pixiv_app::pagination::Plan {
            skip: plan.skip as i64,
            limit: plan.limit as i64,
            one_batch: plan.one_batch,
        },
        initial,
        |cursor| {
            let mut query = request.clone();
            query.cursor = cursor;
            async move {
                let page = client
                    .search_artworks(query)
                    .await
                    .map_err(CommandError::from)?;
                Ok((page.items, page.next))
            }
        },
        |item: &Artwork| {
            if item.total_bookmarks < 0 {
                return Err(pixiv_sdk::Error::new(
                    pixiv_sdk::Reason::MalformedUpstreamResponse,
                    "SearchArtworks",
                )
                .with_detail("artwork bookmark count is negative")
                .into());
            }
            if options
                .bookmark_min
                .is_some_and(|min| item.total_bookmarks < min)
                || options
                    .bookmark_max
                    .is_some_and(|max| item.total_bookmarks > max)
            {
                return Ok(false);
            }
            let kind = match item.kind {
                ArtworkKind::Illust => "illust",
                ArtworkKind::Manga => "manga",
                ArtworkKind::Ugoira => "ugoira",
                ArtworkKind::Unknown => "unknown",
            };
            Ok(filter.rating == "all" || filter.matches(item.x_restrict, kind))
        },
        Some(|cursor, consumed| {
            let mut query = request.clone();
            query.cursor = cursor;
            client
                .checkpoint_search_artworks(query, consumed as i64)
                .map_err(CommandError::from)
        }),
    )
    .await
    .map_err(traversal_error)?;
    let items = page.items;
    let more = page.result.has_more;
    if mode == crate::DetailOutput::Json {
        let mut metadata = serde_json::json!({"membership":"unknown","strategy":strategy,"completeness":if more{"partial"}else{"complete_for_source"}});
        if let Some(min) = options.bookmark_min {
            metadata["min"] = min.into();
        }
        if let Some(max) = options.bookmark_max {
            metadata["max"] = max.into();
        }
        let dtos = items
            .iter()
            .map(pixiv_sdk::dto::ArtworkDto::from)
            .collect::<Vec<_>>();
        let body =
            serde_json::to_string_pretty(&serde_json::json!({"illusts":dtos,"filter":metadata}))
                .map_err(|_| pixiv_sdk::Error::new(pixiv_sdk::Reason::LocalStateError, "output"))?;
        writeln!(out, "{}", crate::go_json_escape(body))?;
        Ok(())
    } else {
        present_search(&request.word, &items, mode, &mut false, out)
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

fn traversal_error(error: pixiv_app::pagination::Failure<CommandError>) -> CommandError {
    match error.cause {
        pixiv_app::pagination::Cause::Source(error) => error,
        pixiv_app::pagination::Cause::Message(message) => CommandError::MessageText(message),
    }
}

async fn visit_search<T: Transport>(
    client: &Client<T>,
    request: SearchArtworksRequest,
    options: &SearchOptions,
    mut consume: impl FnMut(Vec<Artwork>) -> Result<(), CommandError>,
) -> Result<(), CommandError> {
    let filter = pixiv_app::search_filter::normalize_filter(&options.rating, &options.content_type)
        .map_err(|error| CommandError::Message(error.message()))?;
    let local = filter.rating != "all";
    let plan = options.plan()?;
    let checkpoint = local.then_some(|cursor, consumed| {
        let mut query = request.clone();
        query.cursor = cursor;
        client
            .checkpoint_search_artworks(query, consumed as i64)
            .map_err(CommandError::from)
    });
    let plan = pixiv_app::pagination::Plan {
        skip: plan.skip as i64,
        limit: plan.limit as i64,
        one_batch: plan.one_batch,
    };
    let fetch = |cursor| {
        let mut query = request.clone();
        query.cursor = cursor;
        async move {
            let page = client
                .search_artworks(query)
                .await
                .map_err(CommandError::from)?;
            Ok((page.items, page.next))
        }
    };
    let include = |item: &Artwork| {
        let kind = match item.kind {
            ArtworkKind::Illust => "illust",
            ArtworkKind::Manga => "manga",
            ArtworkKind::Ugoira => "ugoira",
            ArtworkKind::Unknown => "unknown",
        };
        Ok(!local || filter.matches(item.x_restrict, kind))
    };
    if local {
        let page = pixiv_app::pagination::collect_pages(
            plan,
            request.cursor.clone(),
            fetch,
            include,
            checkpoint,
        )
        .await
        .map_err(traversal_error)?;
        if !page.items.is_empty() {
            consume(page.items)?;
        }
    } else {
        pixiv_app::pagination::traverse_pages(
            plan,
            request.cursor.clone(),
            fetch,
            include,
            checkpoint,
            consume,
        )
        .await
        .map_err(traversal_error)?;
    }
    Ok(())
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
