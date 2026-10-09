use crate::{auth_bundle::go_quote, scheduler::SchedulerError};
use chrono::Datelike;
use pixiv_sdk::models::Artwork;

fn message(value: impl Into<String>) -> SchedulerError {
    SchedulerError::Message(value.into())
}

pub(super) fn validate_template(template: &str) -> Result<(), SchedulerError> {
    let bytes = template.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'{' => {
                let end = template[index + 1..]
                    .find('}')
                    .map(|end| index + 1 + end)
                    .ok_or_else(|| message("filename template contains an unmatched '{'"))?;
                let placeholder = &template[index..=end];
                if !matches!(
                    placeholder,
                    "{id}" | "{title}" | "{author}" | "{author_id}" | "{date}" | "{tags}" | "{num}"
                ) {
                    return Err(message(format!(
                        "filename template contains unsupported placeholder {}",
                        go_quote(placeholder)
                    )));
                }
                index = end + 1;
            }
            b'}' => return Err(message("filename template contains an unmatched '}'")),
            _ => index += 1,
        }
    }
    Ok(())
}

pub(super) fn sanitize(name: &str) -> String {
    name.chars()
        .map(|character| {
            if matches!(
                character,
                '\\' | '/' | '*' | '?' | ':' | '"' | '<' | '>' | '|'
            ) {
                '_'
            } else {
                character
            }
        })
        .collect()
}

pub(super) fn generate(
    artwork: &Artwork,
    page0: i64,
    template: &str,
) -> Result<String, SchedulerError> {
    render(artwork, page0, template, true)
}

fn render(
    artwork: &Artwork,
    page0: i64,
    template: &str,
    append_page_suffix: bool,
) -> Result<String, SchedulerError> {
    let template = if template.is_empty() {
        "{author} - {title}_{id}"
    } else {
        template
    };
    validate_template(template)?;
    let date = if template.contains("{date}") {
        let published = artwork.published_at;
        if (published.timestamp() == -62_135_596_800 && published.timestamp_subsec_nanos() == 0)
            || !(0..=9999).contains(&published.year())
            || published.timestamp_subsec_nanos() >= 1_000_000_000
        {
            return Err(message("filename template requires a valid create date"));
        }
        published.format("%Y-%m-%d").to_string()
    } else {
        String::new()
    };
    let tags = artwork
        .tags
        .iter()
        .map(|tag| tag.name.trim())
        .filter(|tag| !tag.is_empty())
        .collect::<Vec<_>>()
        .join(",");
    let id = artwork.id.to_string();
    let author_id = artwork.user.id.to_string();
    let number = page0.to_string();
    let mut name = String::with_capacity(template.len());
    for fragment in template.split_inclusive('}') {
        if let Some(start) = fragment.find('{') {
            name.push_str(&fragment[..start]);
            name.push_str(match &fragment[start..] {
                "{author}" if artwork.user.name.is_empty() => "UnknownAuthor",
                "{author}" => &artwork.user.name,
                "{title}" if artwork.title.is_empty() => "Untitled",
                "{title}" => &artwork.title,
                "{id}" => &id,
                "{author_id}" => &author_id,
                "{date}" => &date,
                "{tags}" => &tags,
                "{num}" => &number,
                placeholder => {
                    return Err(message(format!(
                        "filename template contains unsupported placeholder {}",
                        go_quote(placeholder)
                    )));
                }
            });
        } else {
            name.push_str(fragment);
        }
    }
    let mut name = sanitize(&name);
    if append_page_suffix && artwork.page_count > 1 && !template.contains("{num}") {
        name.push_str("_p");
        name.push_str(&number);
    }
    Ok(name)
}

pub(super) fn validate_directory(template: &str) -> Result<(), SchedulerError> {
    let template = template.trim();
    if template.is_empty() {
        return Ok(());
    }
    if template.starts_with(['/', '\\']) {
        return Err(message("directory template must be relative"));
    }
    validate_template(template)?;
    if template
        .split('/')
        .any(|segment| matches!(segment, "" | "." | ".."))
    {
        return Err(message(
            "directory template contains an unsafe path segment",
        ));
    }
    Ok(())
}

pub(super) fn build_directory(template: &str, artwork: &Artwork) -> Result<String, SchedulerError> {
    let template = template.trim();
    if template.is_empty() {
        return Ok(String::new());
    }
    validate_directory(template)?;
    let mut rendered = Vec::new();
    for segment in template.split('/') {
        let value = render(artwork, 0, segment, false)?;
        if matches!(value.as_str(), "" | "." | "..") {
            return Err(message("directory template renders an unsafe path segment"));
        }
        rendered.push(value);
    }
    Ok(rendered.join("/"))
}
