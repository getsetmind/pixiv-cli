use crate::CommandError;
use chrono::{DateTime, FixedOffset, NaiveDate};
use clap::Args;
use pixiv_sdk::pixiv::SearchArtworksRequest;

#[derive(Args, Clone, Debug)]
pub struct SearchOptions {
    #[arg(long, default_value = "tag-partial", allow_hyphen_values = true)]
    pub search_by: String,
    #[arg(long, default_value = "date_desc", allow_hyphen_values = true)]
    pub sort: String,
    #[arg(long, default_value = "all", allow_hyphen_values = true)]
    pub content_type: String,
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
        let content: String = self
            .content_type
            .trim()
            .chars()
            .map(|character| character.to_lowercase().next().unwrap_or(character))
            .collect();
        let content_type = match content.as_str() {
            "" | "all" => "all",
            "illust" | "illustration" => "illust",
            "illust-and-ugoira" => "illust-and-ugoira",
            "manga" => "manga",
            "ugoira" => "ugoira",
            _ => {
                return Err(CommandError::Message(
                    "content-type must be one of all, illust-and-ugoira, illust, manga, ugoira",
                ));
            }
        };
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
            content_type: content_type.into(),
            ai_mode: self.ai_mode.clone(),
            aspect_ratio: self.aspect_ratio.clone(),
            resolution: self.resolution.clone(),
            tool: self.draw_tool.clone(),
            ..Default::default()
        };
        self.dates.apply(&mut request, now)?;
        Ok(request)
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
