use crate::{Error, Reason, Result};
use serde_json::Value;
use std::collections::BTreeSet;

pub(crate) fn resolve(
    kind: &str,
    id: i64,
    variant: &str,
    body: &Value,
    operation: &'static str,
) -> Result<String> {
    let malformed = || Error::new(Reason::MalformedUpstreamResponse, operation);
    let unavailable = || malformed().with_detail("resource metadata has no usable URL");
    match kind {
        "ugoira_archive" => {
            let (urls, _) = validated_ugoira(body, operation)?;
            let selected = if variant.is_empty() {
                text(Some(urls), "original").or_else(|| text(Some(urls), "medium"))
            } else if matches!(variant, "original" | "medium") {
                text(Some(urls), variant)
            } else {
                None
            };
            selected.map(str::to_owned).ok_or_else(unavailable)
        }
        "novel_cover" => {
            let novel = body
                .get("novel")
                .filter(|value| value.is_object())
                .ok_or_else(malformed)?;
            if positive(novel, "id").is_none()
                || !valid_user(novel.get("user"))
                || novel
                    .get("user")
                    .and_then(|user| positive(user, "id"))
                    .is_none()
                || !fields(
                    Some(novel),
                    &["title", "caption", "create_date"],
                    &["is_original"],
                    &[
                        "id",
                        "x_restrict",
                        "text_length",
                        "total_bookmarks",
                        "total_view",
                    ],
                )
                || !fields(
                    novel.get("image_urls"),
                    &["original", "large", "medium", "square_medium"],
                    &[],
                    &[],
                )
                || !tags(novel.get("tags"))
            {
                return Err(malformed());
            }
            for name in ["series_next", "series_prev"] {
                if let Some(series) = body.get(name).filter(|value| !value.is_null())
                    && (positive(series, "id").is_none()
                        || !fields(Some(series), &["title"], &[], &["id"]))
                {
                    return Err(malformed());
                }
            }
            let images = novel.get("image_urls");
            let url = if variant.is_empty() {
                ["original", "large", "medium", "square_medium"]
                    .into_iter()
                    .find_map(|name| text(images, name))
            } else if matches!(variant, "original" | "large" | "medium" | "square_medium") {
                text(images, variant)
            } else {
                None
            };
            url.map(str::to_owned).ok_or_else(unavailable)
        }
        "user_profile" => {
            if ["user", "profile", "profile_publicity", "workspace"]
                .iter()
                .any(|name| !body.get(name).is_some_and(Value::is_object))
                || !valid_user(body.get("user"))
                || body
                    .get("user")
                    .and_then(|user| positive(user, "id"))
                    .is_none()
                || !fields(
                    body.get("profile"),
                    &[
                        "webpage",
                        "gender",
                        "birth",
                        "birth_day",
                        "region",
                        "country_code",
                        "job",
                        "background_image_url",
                        "twitter_account",
                        "twitter_url",
                        "pawoo_url",
                    ],
                    &["is_premium", "is_using_custom_profile_image"],
                    &[
                        "birth_year",
                        "address_id",
                        "job_id",
                        "total_follow_users",
                        "total_mypixiv_users",
                        "total_illusts",
                        "total_manga",
                        "total_novels",
                        "total_illust_bookmarks_public",
                        "total_illust_series",
                        "total_novel_series",
                    ],
                )
                || !fields(
                    body.get("workspace"),
                    &[
                        "pc",
                        "monitor",
                        "tool",
                        "scanner",
                        "tablet",
                        "mouse",
                        "printer",
                        "desktop",
                        "music",
                        "desk",
                        "chair",
                        "comment",
                        "workspace_image_url",
                    ],
                    &[],
                    &[],
                )
            {
                return Err(malformed());
            }
            let publicity = &body["profile_publicity"];
            for name in [
                "gender",
                "region",
                "birth_day",
                "birth_year",
                "job",
                "pawoo",
            ] {
                if let Some(value) = publicity.get(name)
                    && !(value.is_boolean() || matches!(value.as_str(), Some("public" | "private")))
                {
                    return Err(malformed());
                }
            }
            text(body["user"].get("profile_image_urls"), "medium")
                .map(str::to_owned)
                .ok_or_else(unavailable)
        }
        "stamp" => crate::stamps::validated_stamps(body, operation)?
            .into_iter()
            .find(|(stamp_id, _)| *stamp_id == id)
            .map(|(_, url)| url.to_owned())
            .ok_or_else(unavailable),

        _ => Err(Error::new(Reason::InvalidArgument, operation)
            .with_detail("resource kind is unsupported")),
    }
}

pub(crate) fn validated_ugoira<'a>(
    body: &'a Value,
    operation: &'static str,
) -> Result<(&'a Value, &'a [Value])> {
    let malformed = || Error::new(Reason::MalformedUpstreamResponse, operation);
    let metadata = body
        .get("ugoira_metadata")
        .filter(|value| value.is_object())
        .ok_or_else(malformed)?;
    let urls = metadata
        .get("zip_urls")
        .filter(|value| value.is_object())
        .ok_or_else(malformed)?;
    let frames = metadata
        .get("frames")
        .and_then(Value::as_array)
        .ok_or_else(malformed)?;
    if !fields(Some(urls), &["original", "medium"], &[], &[])
        || (text(Some(urls), "original").is_none() && text(Some(urls), "medium").is_none())
        || frames.is_empty()
    {
        return Err(malformed());
    }
    let mut names = BTreeSet::new();
    for frame in frames {
        if !fields(Some(frame), &["file"], &[], &["delay"]) {
            return Err(malformed());
        }
        let file = text(Some(frame), "file").ok_or_else(malformed)?;
        let normalized = file.replace('\\', "/");
        if file.contains('\0')
            || normalized.starts_with('/')
            || normalized.as_bytes().get(1) == Some(&b':')
            || normalized
                .split('/')
                .any(|part| matches!(part, "" | "." | ".."))
            || !names.insert(normalized)
        {
            return Err(malformed());
        }
    }
    Ok((urls, frames))
}

fn fields(value: Option<&Value>, strings: &[&str], booleans: &[&str], integers: &[&str]) -> bool {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return true;
    };
    value.is_object()
        && strings.iter().all(|name| {
            value
                .get(name)
                .is_none_or(|value| value.is_null() || value.is_string())
        })
        && booleans.iter().all(|name| {
            value
                .get(name)
                .is_none_or(|value| value.is_null() || value.is_boolean())
        })
        && integers.iter().all(|name| {
            value
                .get(name)
                .is_none_or(|value| value.is_null() || value.as_i64().is_some())
        })
}
fn valid_user(user: Option<&Value>) -> bool {
    fields(
        user,
        &["name", "account", "comment"],
        &["is_followed"],
        &["id"],
    ) && fields(
        user.and_then(|user| user.get("profile_image_urls")),
        &["medium"],
        &[],
        &[],
    )
}
fn tags(value: Option<&Value>) -> bool {
    value.is_none_or(|value| {
        value.is_null()
            || value.as_array().is_some_and(|tags| {
                tags.iter()
                    .all(|tag| fields(Some(tag), &["name", "translated_name"], &[], &[]))
            })
    })
}
fn positive(value: &Value, name: &str) -> Option<i64> {
    value.get(name).and_then(Value::as_i64).filter(|id| *id > 0)
}
fn text<'a>(value: Option<&'a Value>, name: &str) -> Option<&'a str> {
    value
        .and_then(|value| value.get(name))
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
}
