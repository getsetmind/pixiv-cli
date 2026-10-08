use crate::CommandError;
use chrono::{DateTime, FixedOffset, NaiveDate};
use clap::Args;
use pixiv_sdk::pixiv::SearchArtworksRequest;

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
