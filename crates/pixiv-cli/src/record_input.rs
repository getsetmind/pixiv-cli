pub(crate) fn parse(line: &[u8]) -> Result<(String, String, String), String> {
    let value: &serde_json::value::RawValue =
        serde_json::from_slice(line).map_err(|_| "invalid record JSON object".to_owned())?;
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
        }
        normalized.push(byte);
        position += 1;
    }
    parse(&normalized)
}
