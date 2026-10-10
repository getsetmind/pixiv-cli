mod json;

pub(crate) use json::{diagnostic_identity, parse, parse_go, parse_go_line};

use crate::CommandError;
fn invalid(operation: &str, detail: &str) -> CommandError {
    pixiv_sdk::Error::new(pixiv_sdk::Reason::InvalidArgument, operation)
        .with_detail(detail)
        .into()
}
fn detail_type<'a>(entity: &'a str, operation: &str) -> Result<&'a str, CommandError> {
    let entity = entity.trim();
    let entity = if entity.is_empty() { "artwork" } else { entity };
    if !matches!(entity, "artwork" | "novel") {
        return Err(invalid(
            operation,
            &format!(
                "type {} is not supported by this command",
                crate::search::quote(entity)
            ),
        ));
    }
    Ok(entity)
}
pub(crate) fn bookmark_target(
    source: Option<&str>,
    record: Option<&str>,
    entity: &str,
    operation: &str,
    detail_command: bool,
) -> Result<(i64, String), CommandError> {
    let id = if let Some(record) = record {
        record_target(record, entity, operation, detail_command)?
    } else {
        text_target(
            source.unwrap_or_default(),
            entity,
            operation,
            detail_command,
        )?
    };
    Ok((
        id,
        if detail_command {
            detail_type(entity, operation)?.into()
        } else {
            entity.into()
        },
    ))
}
fn text_target(
    value: &str,
    entity: &str,
    operation: &str,
    detail_command: bool,
) -> Result<i64, CommandError> {
    let invalid = |detail: &str| invalid(operation, detail);
    let value = value.trim();
    if value.is_empty() {
        return Err(invalid("input value is required"));
    }
    if let Ok(id) = value.parse::<i64>() {
        return if id > 0 {
            if detail_command {
                detail_type(entity, operation)?;
            }
            Ok(id)
        } else {
            Err(invalid("id must be a positive integer"))
        };
    }
    let reference = pixiv_sdk::reference::parse_url(value)
        .map_err(|_| invalid("input must be a positive ID or a supported Pixiv URL"))?;
    if detail_command {
        if !matches!(reference.kind.as_str(), "artwork" | "novel") {
            return Err(invalid("URL kind is not allowed for this command"));
        }
        let entity = detail_type(entity, operation)?;
        if reference.kind != entity {
            return Err(invalid("URL namespace conflicts with the selected type"));
        }
        return Ok(reference.id);
    }
    match reference.kind.as_str() {
        "user" => Ok(reference.id),
        "user_bookmarks" if entity == "artwork" => Ok(reference.id),
        "user_bookmarks" => Err(invalid("URL namespace conflicts with the selected type")),
        _ => Err(invalid("URL kind is not allowed for this command")),
    }
}
fn record_target(
    value: &str,
    entity: &str,
    operation: &str,
    detail_command: bool,
) -> Result<i64, CommandError> {
    let invalid = |detail: &str| invalid(operation, detail);
    let (id, typ, url) =
        crate::record_input::parse_go(value.as_bytes()).map_err(CommandError::MessageText)?;
    let id = id
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| invalid("record id must be a positive integer"))?;
    let reference = pixiv_sdk::reference::parse_url(&url)
        .map_err(|_| invalid("record url must be a supported Pixiv URL"))?;
    if reference.id != id {
        return Err(invalid("record id does not match record url"));
    }
    let visual = matches!(
        typ.as_str(),
        "artwork" | "illustration" | "illust" | "manga" | "ugoira"
    );
    if !visual && !matches!(typ.as_str(), "novel" | "user" | "series" | "user_bookmarks") {
        return Err(invalid(
            "record type is not a supported Pixiv namespace or subtype",
        ));
    }
    let conflict = match reference.kind.as_str() {
        "artwork" if !visual => Some("record type conflicts with artwork URL"),
        "novel" if typ != "novel" => Some("record type conflicts with novel URL"),
        "user" if typ != "user" => Some("record type conflicts with user URL"),
        "user_bookmarks" if !visual && typ != "user_bookmarks" => {
            Some("record type conflicts with user bookmarks URL")
        }
        "artwork_series" if !visual && typ != "series" => {
            Some("record type conflicts with artwork series URL")
        }
        "novel_series" if typ != "novel" && typ != "series" => {
            Some("record type conflicts with novel series URL")
        }
        _ => None,
    };
    if let Some(detail) = conflict {
        return Err(invalid(detail));
    }
    if detail_command {
        let entity = detail_type(entity, operation)?;
        if !matches!(reference.kind.as_str(), "artwork" | "novel") {
            return Err(invalid("URL type relation is not supported"));
        }
        if reference.kind != entity {
            return Err(invalid("URL namespace conflicts with the selected type"));
        }
        return Ok(id);
    }
    match reference.kind.as_str() {
        "user" => {}
        "user_bookmarks" if entity == "artwork" => {}
        "user_bookmarks" => return Err(invalid("URL namespace conflicts with the selected type")),
        _ => return Err(invalid("URL type relation is not supported")),
    }
    if matches!(typ.as_str(), "illustration" | "illust" | "manga" | "ugoira") && entity != "artwork"
    {
        return Err(invalid(
            "record artwork subtype conflicts with the selected type",
        ));
    }
    Ok(id)
}

pub(crate) fn resolve_source<R: std::io::Read>(
    sources: &mut Vec<String>,
    record: &mut Option<String>,
    pending: &mut bool,
    input: &mut R,
    terminal: bool,
) -> Result<(), CommandError> {
    if !sources.is_empty() || terminal {
        return Ok(());
    }
    let mut bytes = vec![];
    loop {
        let mut one = [0_u8];
        let count = input
            .read(&mut one)
            .map_err(|error| CommandError::Usage(format!("read stdin input: {error}")))?;
        if count == 0 {
            break;
        }
        bytes.push(one[0]);
        if !matches!(one[0], b' ' | b'\t' | b'\n' | b'\r') {
            if one[0] == b'{' {
                *record = Some(String::from_utf8_lossy(&bytes).into_owned());
                *pending = true;
                return Ok(());
            }
            input
                .read_to_end(&mut bytes)
                .map_err(|error| CommandError::Usage(format!("read stdin input: {error}")))?;
            break;
        }
    }
    if bytes.ends_with(b"\r\n") {
        bytes.truncate(bytes.len() - 2);
    } else if bytes.ends_with(b"\n") {
        bytes.pop();
    }
    if !bytes.is_empty() {
        sources.push(String::from_utf8_lossy(&bytes).into_owned());
    }
    Ok(())
}
pub(crate) fn read_pending<R: std::io::Read>(
    record: &mut Option<String>,
    pending: &mut bool,
    input: &mut R,
    label: &str,
) -> Result<(), CommandError> {
    if *pending {
        let mut bytes = record.take().unwrap_or_default().into_bytes();
        input
            .read_to_end(&mut bytes)
            .map_err(|error| CommandError::MessageText(format!("{label}: {error}")))?;
        *record = Some(String::from_utf8_lossy(&bytes).into_owned());
        *pending = false;
    }
    Ok(())
}
