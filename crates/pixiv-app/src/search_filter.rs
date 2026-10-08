use sha2::{Digest, Sha256};
use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtworkFilter {
    pub rating: &'static str,
    pub content_type: &'static str,
    pub cursor_context: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FilterError(&'static str);
impl fmt::Display for FilterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}
impl std::error::Error for FilterError {}

fn canonical(value: &str) -> String {
    value
        .trim()
        .chars()
        .map(|character| character.to_lowercase().next().unwrap_or(character))
        .collect()
}

pub fn normalize_filter(rating: &str, content_type: &str) -> Result<ArtworkFilter, FilterError> {
    let rating = match canonical(rating).as_str() {
        "" | "all" => "all",
        "sfw" => "sfw",
        "r18" => "r18",
        "r18g" => "r18g",
        "mature" => "mature",
        _ => {
            return Err(FilterError(
                "rating must be one of sfw, r18, r18g, mature, all",
            ));
        }
    };
    let content_type = match canonical(content_type).as_str() {
        "" | "all" => "all",
        "illust" | "illustration" => "illust",
        "illust-and-ugoira" => "illust-and-ugoira",
        "manga" => "manga",
        "ugoira" => "ugoira",
        _ => {
            return Err(FilterError(
                "content-type must be one of all, illust-and-ugoira, illust, manga, ugoira",
            ));
        }
    };
    Ok(ArtworkFilter {
        rating,
        content_type,
        cursor_context: format!(
            "{:x}",
            Sha256::digest(format!("filter/v1\n{rating}\n{content_type}").as_bytes())
        ),
    })
}

impl ArtworkFilter {
    pub fn matches(&self, x_restrict: i64, kind: &str) -> bool {
        let rating = match self.rating {
            "" | "all" => true,
            "sfw" => x_restrict == 0,
            "r18" => x_restrict == 1,
            "r18g" => x_restrict == 2,
            "mature" => x_restrict == 1 || x_restrict == 2,
            _ => false,
        };
        if !rating {
            return false;
        }
        if self.content_type.is_empty() || self.content_type == "all" {
            return true;
        }
        let kind = canonical(kind);
        let kind = match kind.as_str() {
            "" | "illustration" => "illust",
            kind => kind,
        };
        match self.content_type {
            "illust" => kind == "illust",
            "illust-and-ugoira" => kind == "illust" || kind == "ugoira",
            "manga" | "ugoira" => kind == self.content_type,
            _ => false,
        }
    }
}
