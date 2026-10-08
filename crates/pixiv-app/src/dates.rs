use chrono::{DateTime, Datelike, FixedOffset, Months, NaiveDate};
use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DateRange {
    pub start_date: String,
    pub end_date: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DateRangeOverflow;
impl fmt::Display for DateRangeOverflow {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("date range overflow")
    }
}
impl std::error::Error for DateRangeOverflow {}

fn format_date(value: NaiveDate) -> String {
    let year = value.year();
    let sign = if year < 0 { "-" } else { "" };
    format!(
        "{sign}{:04}-{:02}-{:02}",
        year.unsigned_abs(),
        value.month(),
        value.day()
    )
}

pub fn quick_date_range(
    duration: &str,
    now: DateTime<FixedOffset>,
) -> Result<Option<DateRange>, DateRangeOverflow> {
    let months = match duration {
        "within_half_year" => 6,
        "within_year" => 12,
        _ => return Ok(None),
    };
    let today = now
        .with_timezone(&FixedOffset::east_opt(9 * 3600).expect("Tokyo offset is valid"))
        .date_naive();
    let start = today
        .checked_sub_months(Months::new(months))
        .ok_or(DateRangeOverflow)?;
    Ok(Some(DateRange {
        start_date: format_date(start),
        end_date: format_date(today),
    }))
}
