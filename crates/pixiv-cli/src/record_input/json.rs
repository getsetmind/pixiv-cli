pub(crate) fn parse(line: &[u8]) -> Result<(String, String, String), String> {
    let value: &serde_json::value::RawValue =
        serde_json::from_slice(line).map_err(|_| "invalid record JSON object".to_owned())?;
    if !within_go_depth(value.get().as_bytes()) {
        return Err("invalid record JSON object".into());
    }
    let fields: std::collections::BTreeMap<String, Box<serde_json::value::RawValue>> =
        serde_json::from_str(value.get())
            .map_err(|_| "invalid record JSON object: record must be a JSON object".to_owned())?;
    let id = match fields.get("id") {
        None => return Err("record id is required".into()),
        Some(raw) if raw.get().starts_with('"') => {
            let id: String =
                serde_json::from_str(raw.get()).map_err(|_| "invalid record JSON object")?;
            if id.is_empty() {
                return Err("record id must be a non-empty string".into());
            }
            id
        }
        Some(raw) => {
            let id = raw.get();
            if !id.starts_with(['1', '2', '3', '4', '5', '6', '7', '8', '9'])
                || !id.bytes().all(|b| b.is_ascii_digit())
            {
                return Err("record id must be a non-empty string or positive integer".into());
            }
            id.to_owned()
        }
    };
    let required = |name: &str| -> Result<String, String> {
        let value = fields
            .get(name)
            .ok_or_else(|| format!("record {name} is required"))?;
        let value: String = serde_json::from_str(value.get())
            .map_err(|_| format!("record {name} must be a string"))?;
        if value.is_empty() {
            return Err(format!("record {name} must be a non-empty string"));
        }
        Ok(value)
    };
    let typ = required("type")?;
    let url = required("url")?;
    Ok((id, typ, url))
}

pub(crate) fn parse_go(line: &[u8]) -> Result<(String, String, String), String> {
    parse(&normalize_go_strings(line))
}

pub(crate) fn parse_go_line(line: &[u8]) -> Result<(String, String, String), String> {
    parse_go(trim_go_space(line))
}

pub(crate) fn diagnostic_identity(line: &[u8]) -> (String, String) {
    let normalized = normalize_go_strings(line);
    let Some(Ok(value)) = serde_json::Deserializer::from_slice(&normalized)
        .into_iter::<Box<serde_json::value::RawValue>>()
        .next()
    else {
        return (String::new(), String::new());
    };
    if !within_go_depth(value.get().as_bytes()) {
        return (String::new(), String::new());
    }
    let Ok(fields) = serde_json::from_str::<
        std::collections::BTreeMap<String, Box<serde_json::value::RawValue>>,
    >(value.get()) else {
        return (String::new(), String::new());
    };
    let id = match fields.get("id").map(|raw| raw.get()) {
        Some(raw) if raw.starts_with('"') => serde_json::from_str(raw).unwrap_or_default(),
        Some(raw) if raw.starts_with('-') || raw.as_bytes()[0].is_ascii_digit() => raw.to_owned(),
        _ => String::new(),
    };
    let typ = fields
        .get("type")
        .and_then(|raw| serde_json::from_str::<String>(raw.get()).ok())
        .unwrap_or_default();
    (id, typ)
}

fn within_go_depth(value: &[u8]) -> bool {
    let mut depth = 0_u32;
    let mut string = false;
    let mut escaped = false;
    for byte in value {
        if string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                string = false;
            }
        } else {
            match byte {
                b'"' => string = true,
                b'[' | b'{' => {
                    depth += 1;
                    if depth > 10_000 {
                        return false;
                    }
                }
                b']' | b'}' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    true
}

fn utf8_width(byte: u8) -> usize {
    match byte {
        0..=0x7f => 1,
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => 0,
    }
}

fn first_scalar(bytes: &[u8]) -> Option<(char, usize)> {
    let width = utf8_width(*bytes.first()?);
    if width == 0 {
        return None;
    }
    let value = std::str::from_utf8(bytes.get(..width)?).ok()?;
    Some((value.chars().next()?, width))
}

fn last_scalar(bytes: &[u8]) -> Option<(char, usize)> {
    let offset = bytes
        .iter()
        .enumerate()
        .rev()
        .take(4)
        .find(|(_, byte)| **byte & 0xc0 != 0x80)?
        .0;
    let suffix = &bytes[offset..];
    let (ch, width) = first_scalar(suffix)?;
    (width == suffix.len()).then_some((ch, width))
}

fn go_space(ch: char) -> bool {
    matches!(ch, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{0085}' | '\u{00a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}')
}

fn trim_go_space(mut bytes: &[u8]) -> &[u8] {
    while let Some((ch, width)) = first_scalar(bytes) {
        if !go_space(ch) {
            break;
        }
        bytes = &bytes[width..];
    }
    while let Some((ch, width)) = last_scalar(bytes) {
        if !go_space(ch) {
            break;
        }
        bytes = &bytes[..bytes.len() - width];
    }
    bytes
}

fn normalize_go_strings(line: &[u8]) -> Vec<u8> {
    let mut normalized = Vec::with_capacity(line.len());
    let mut position = 0;
    let mut string = false;
    while position < line.len() {
        let byte = line[position];
        if byte == b'"' {
            string = !string;
        } else if string && byte == b'\\' {
            let unicode = |offset: usize| {
                let bytes = line.get(offset..offset + 6)?;
                if bytes[..2] != *b"\\u" {
                    return None;
                }
                std::str::from_utf8(&bytes[2..])
                    .ok()
                    .and_then(|value| u16::from_str_radix(value, 16).ok())
            };
            if let Some(code) = unicode(position) {
                if (0xd800..=0xdbff).contains(&code)
                    && unicode(position + 6).is_some_and(|next| (0xdc00..=0xdfff).contains(&next))
                {
                    normalized.extend_from_slice(&line[position..position + 12]);
                    position += 12;
                    continue;
                }
                if (0xd800..=0xdfff).contains(&code) {
                    normalized.extend_from_slice(b"\\ufffd");
                    position += 6;
                    continue;
                }
            }
            if let Some(next) = line.get(position + 1) {
                normalized.extend_from_slice(&[byte, *next]);
                position += 2;
                continue;
            }
        } else if string && byte >= 0x80 {
            if let Some((_, width)) = first_scalar(&line[position..]) {
                normalized.extend_from_slice(&line[position..position + width]);
                position += width;
            } else {
                normalized.extend_from_slice(b"\\ufffd");
                position += 1;
            }
            continue;
        }
        normalized.push(byte);
        position += 1;
    }
    normalized
}
