use super::ConfigError;
use std::ops::Range;
use toml_edit::{Document, Item, Table, Value};

pub(super) fn mutate(body: &str, path: &str, value: Option<Value>) -> Result<String, ConfigError> {
    let source = Document::parse(body).map_err(|error| syntax_error(body, error))?;
    let mut sections = sections(&source)?;
    let mut name = path.split('.').map(str::to_owned).collect::<Vec<_>>();
    let key = name
        .pop()
        .ok_or_else(|| ConfigError::Invalid("empty config key".into()))?;
    if let Some(value) = value {
        let mapping = new_mapping(key, value)?;
        let section = match sections.iter().position(|section| section.name() == name) {
            Some(index) => &mut sections[index],
            None => {
                sections.push(Section {
                    heading: Some(Heading {
                        name,
                        array: false,
                        block: Vec::new(),
                        trailer: String::new(),
                    }),
                    items: Vec::new(),
                });
                sections.last_mut().expect("new section")
            }
        };
        if let Some(existing) = section.items.iter_mut().find_map(|item| match item {
            Entry::Mapping(existing) if existing.name == mapping.name => Some(existing),
            _ => None,
        }) {
            *existing = mapping;
        } else {
            let index = section
                .items
                .iter()
                .rposition(|item| !matches!(item, Entry::Comments(_)))
                .map_or(0, |index| index + 1);
            section.items.insert(index, Entry::Mapping(mapping));
        }
    } else {
        let mut full_name = name.clone();
        full_name.push(key);
        let mut removed = false;
        for section in &mut sections {
            if section.name() == full_name && section.heading.is_some() {
                section.heading = None;
                section.items.clear();
                removed = true;
                break;
            }
            let base = section.name().to_vec();
            if remove_mapping(&mut section.items, &base, &full_name) {
                removed = true;
                break;
            }
        }
        if removed
            && let Some(index) = sections
                .iter()
                .position(|section| section.heading.is_some() && section.name() == name)
            && sections[index].items.is_empty()
        {
            sections.remove(index);
        }
    }
    Ok(format_sections(&sections))
}

struct Section {
    heading: Option<Heading>,
    items: Vec<Entry>,
}

impl Section {
    fn name(&self) -> &[String] {
        self.heading.as_ref().map_or(&[], |heading| &heading.name)
    }
}

struct Heading {
    name: Vec<String>,
    array: bool,
    block: Vec<String>,
    trailer: String,
}

enum Entry {
    Comments(Vec<String>),
    Mapping(Mapping),
}

struct Mapping {
    name: Vec<String>,
    datum: Datum,
    block: Vec<String>,
    trailer: String,
}

enum Datum {
    Token(String),
    Array(Vec<ArrayEntry>),
    Inline(Vec<Mapping>),
}

enum ArrayEntry {
    Comments(Vec<String>),
    Value(Datum, String),
}

enum SourceEntry<'a> {
    Heading(Vec<String>, bool),
    Mapping(Vec<String>, &'a Value),
}

fn sections(source: &Document<&str>) -> Result<Vec<Section>, ConfigError> {
    let body = source.raw();
    let mut entries = Vec::new();
    collect_entries(source.as_table(), &[], true, body, &mut entries)?;
    entries.sort_by_key(|(span, _)| span.start);
    let mut sections = vec![Section {
        heading: None,
        items: Vec::new(),
    }];
    let mut offset = 0;
    for (span, entry) in entries {
        let (comments, block) = comment_blocks(&body[offset..span.start]);
        sections
            .last_mut()
            .expect("global section")
            .items
            .extend(comments.into_iter().map(Entry::Comments));
        let (trailer, end) = line_trailer(body, span.end);
        offset = end;
        match entry {
            SourceEntry::Heading(name, array) => sections.push(Section {
                heading: Some(Heading {
                    name,
                    array,
                    block,
                    trailer,
                }),
                items: Vec::new(),
            }),
            SourceEntry::Mapping(name, value) => sections
                .last_mut()
                .expect("global section")
                .items
                .push(Entry::Mapping(Mapping {
                    name,
                    datum: datum(value, body)?,
                    block,
                    trailer,
                })),
        }
    }
    let (mut comments, block) = comment_blocks(&body[offset..]);
    if !block.is_empty() {
        comments.push(block);
    }
    sections
        .last_mut()
        .expect("global section")
        .items
        .extend(comments.into_iter().map(Entry::Comments));
    Ok(sections)
}

fn collect_entries<'a>(
    table: &'a Table,
    name: &[String],
    root: bool,
    body: &str,
    entries: &mut Vec<(Range<usize>, SourceEntry<'a>)>,
) -> Result<(), ConfigError> {
    if !root && !table.is_implicit() && !table.is_dotted() {
        let span = table
            .span()
            .ok_or_else(|| ConfigError::Invalid("missing TOML table syntax".into()))?;
        let array = body[span.clone()].starts_with("[[");
        entries.push((span, SourceEntry::Heading(name.to_vec(), array)));
    }
    if root || (!table.is_implicit() && !table.is_dotted()) {
        for (keys, value) in table.get_values() {
            let mut span = value
                .span()
                .ok_or_else(|| ConfigError::Invalid("missing TOML value syntax".into()))?;
            span.start = body[..span.start].rfind('\n').map_or(0, |index| index + 1);
            let keys = keys.iter().map(|key| key.get().to_owned()).collect();
            entries.push((span, SourceEntry::Mapping(keys, value)));
        }
    }
    for (key, item) in table.iter() {
        let mut child_name = name.to_vec();
        child_name.push(key.to_owned());
        match item {
            Item::Table(child) => {
                collect_entries(child, &child_name, false, body, entries)?;
            }
            Item::ArrayOfTables(array) => {
                for child in array.iter() {
                    collect_entries(child, &child_name, false, body, entries)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn datum(value: &Value, body: &str) -> Result<Datum, ConfigError> {
    let span = value
        .span()
        .ok_or_else(|| ConfigError::Invalid("missing TOML value syntax".into()))?;
    match value {
        Value::Array(array) => {
            let mut entries = Vec::new();
            let mut offset = span.start + 1;
            for value in array.iter() {
                let child = value
                    .span()
                    .ok_or_else(|| ConfigError::Invalid("missing TOML array syntax".into()))?;
                array_comments(&body[offset..child.start], &mut entries);
                entries.push(ArrayEntry::Value(datum(value, body)?, String::new()));
                offset = child.end;
            }
            array_comments(&body[offset..span.end - 1], &mut entries);
            Ok(Datum::Array(entries))
        }
        Value::InlineTable(table) => {
            let mut values = table.get_values();
            values.sort_by_key(|(_, value)| value.span().map_or(usize::MAX, |span| span.start));
            let mappings = values
                .into_iter()
                .map(|(keys, value)| {
                    Ok(Mapping {
                        name: keys.iter().map(|key| key.get().to_owned()).collect(),
                        datum: datum(value, body)?,
                        block: Vec::new(),
                        trailer: String::new(),
                    })
                })
                .collect::<Result<_, ConfigError>>()?;
            Ok(Datum::Inline(mappings))
        }
        _ => Ok(Datum::Token(body[span].to_owned())),
    }
}

fn comment_blocks(body: &str) -> (Vec<Vec<String>>, Vec<String>) {
    let mut groups = Vec::new();
    let mut block = Vec::new();
    for line in body.split_inclusive('\n') {
        let text = line.trim();
        if text.starts_with('#') {
            block.push(text.to_owned());
        } else if line.ends_with('\n') && !block.is_empty() {
            groups.push(std::mem::take(&mut block));
        }
    }
    (groups, block)
}

fn line_trailer(body: &str, offset: usize) -> (String, usize) {
    let suffix = &body[offset..];
    let count = suffix.find('\n').map_or(suffix.len(), |index| index + 1);
    let line = &suffix[..count];
    let trailer = line.find('#').map_or("", |index| line[index..].trim());
    (trailer.to_owned(), offset + count)
}

fn array_comments(body: &str, entries: &mut Vec<ArrayEntry>) {
    let mut block = Vec::new();
    for (index, line) in body.split_inclusive('\n').enumerate() {
        if let Some(start) = line.find('#') {
            let comment = line[start..].trim().to_owned();
            if index == 0
                && let Some(ArrayEntry::Value(_, trailer)) = entries.last_mut()
            {
                *trailer = comment;
            } else {
                block.push(comment);
            }
        } else if line.ends_with('\n') && !block.is_empty() {
            entries.push(ArrayEntry::Comments(std::mem::take(&mut block)));
        }
    }
    if !block.is_empty() {
        entries.push(ArrayEntry::Comments(block));
    }
}

fn new_mapping(key: String, value: Value) -> Result<Mapping, ConfigError> {
    let text = match &value {
        Value::String(value) if value.as_repr().is_none() => quoted(value.value()),
        _ => {
            let mut undecorated = value.clone();
            undecorated.decor_mut().clear();
            undecorated.to_string()
        }
    };
    let body = format!("value = {text}");
    let source = Document::parse(body.as_str()).map_err(|error| syntax_error(&body, error))?;
    let value = source["value"]
        .as_value()
        .ok_or_else(|| ConfigError::Invalid("missing TOML value".into()))?;
    Ok(Mapping {
        name: vec![key],
        datum: datum(value, &body)?,
        block: Vec::new(),
        trailer: String::new(),
    })
}

fn remove_mapping(entries: &mut Vec<Entry>, base: &[String], name: &[String]) -> bool {
    for index in 0..entries.len() {
        let Entry::Mapping(mapping) = &mut entries[index] else {
            continue;
        };
        let mut full = base.to_vec();
        full.extend(mapping.name.iter().cloned());
        if full == name {
            entries.remove(index);
            return true;
        }
        if remove_inline(&mut mapping.datum, &full, name) {
            return true;
        }
    }
    false
}

fn remove_inline(datum: &mut Datum, base: &[String], name: &[String]) -> bool {
    let Datum::Inline(mappings) = datum else {
        return false;
    };
    for index in 0..mappings.len() {
        let mut full = base.to_vec();
        full.extend(mappings[index].name.iter().cloned());
        if full == name {
            mappings.remove(index);
            return true;
        }
        if remove_inline(&mut mappings[index].datum, &full, name) {
            return true;
        }
    }
    false
}

fn format_sections(sections: &[Section]) -> String {
    let mut output = String::new();
    let mut count = 0;
    let mut previous_comment = false;
    for section in sections {
        if let Some(heading) = &section.heading {
            if count > 0 {
                output.push('\n');
            }
            format_comments(&mut output, &heading.block, "");
            output.push('[');
            if heading.array {
                output.push('[');
            }
            output.push_str(&format_key(&heading.name));
            output.push(']');
            if heading.array {
                output.push(']');
            }
            format_trailer(&mut output, &heading.trailer);
            output.push('\n');
            count += 1;
            previous_comment = false;
        }
        for item in &section.items {
            let wants_blank = match item {
                Entry::Comments(block) => !block.is_empty(),
                Entry::Mapping(mapping) => !mapping.block.is_empty(),
            };
            if count > 0 && (wants_blank || previous_comment) {
                output.push('\n');
            }
            match item {
                Entry::Comments(block) => format_comments(&mut output, block, ""),
                Entry::Mapping(mapping) => {
                    format_comments(&mut output, &mapping.block, "");
                    output.push_str(&format_key(&mapping.name));
                    output.push_str(" = ");
                    format_datum(&mut output, &mapping.datum, "");
                    format_trailer(&mut output, &mapping.trailer);
                    output.push('\n');
                }
            }
            count += 1;
            previous_comment = matches!(item, Entry::Comments(_));
        }
    }
    output
}

fn format_datum(output: &mut String, datum: &Datum, prefix: &str) {
    match datum {
        Datum::Token(token) => {
            output.push_str(prefix);
            output.push_str(token);
        }
        Datum::Array(entries) => {
            output.push_str(prefix);
            output.push('[');
            if entries.iter().any(interesting_array_entry) {
                output.push('\n');
                let child_prefix = format!("{prefix}  ");
                for entry in entries {
                    match entry {
                        ArrayEntry::Comments(block) => {
                            format_comments(output, block, &child_prefix);
                        }
                        ArrayEntry::Value(datum, trailer) => {
                            format_datum(output, datum, &child_prefix);
                            output.push(',');
                            format_trailer(output, trailer);
                        }
                    }
                    output.push('\n');
                }
                output.push_str(prefix);
            } else {
                for (index, entry) in entries.iter().enumerate() {
                    if index > 0 {
                        output.push_str(", ");
                    }
                    if let ArrayEntry::Value(datum, _) = entry {
                        format_datum(output, datum, "");
                    }
                }
            }
            output.push(']');
        }
        Datum::Inline(mappings) => {
            output.push_str(prefix);
            output.push('{');
            for (index, mapping) in mappings.iter().enumerate() {
                if index > 0 {
                    output.push_str(", ");
                }
                output.push_str(prefix);
                output.push_str(&format_key(&mapping.name));
                output.push_str(" = ");
                format_datum(output, &mapping.datum, prefix);
            }
            if !mappings.is_empty() {
                output.push_str(prefix);
            }
            output.push('}');
        }
    }
}

fn interesting_array_entry(entry: &ArrayEntry) -> bool {
    match entry {
        ArrayEntry::Comments(block) => !block.is_empty(),
        ArrayEntry::Value(datum, trailer) => {
            !trailer.is_empty()
                || match datum {
                    Datum::Token(token) => token.starts_with("\"\"\"") || token.starts_with("'''"),
                    Datum::Array(entries) => !entries.is_empty(),
                    Datum::Inline(mappings) => !mappings.is_empty(),
                }
        }
    }
}

fn format_comments(output: &mut String, block: &[String], prefix: &str) {
    for comment in block {
        output.push_str(prefix);
        output.push_str(comment.trim());
        output.push('\n');
    }
}

fn format_trailer(output: &mut String, trailer: &str) {
    if !trailer.is_empty() {
        output.push_str("  ");
        output.push_str(trailer.trim());
    }
}

fn format_key(name: &[String]) -> String {
    name.iter()
        .map(|key| {
            if !key.is_empty()
                && key
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            {
                key.clone()
            } else {
                quoted(key)
            }
        })
        .collect::<Vec<_>>()
        .join(".")
}

fn quoted(text: &str) -> String {
    let mut output = String::from("\"");
    for character in text.chars() {
        match character {
            '\u{08}' => output.push_str("\\b"),
            '\u{0c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '\\' => output.push_str("\\\\"),
            '"' => output.push_str("\\\""),
            '\u{2028}' => output.push_str("\\u2028"),
            '\u{2029}' => output.push_str("\\u2029"),
            '\u{fffd}' => output.push_str("\\ufffd"),
            character if character < ' ' => {
                use std::fmt::Write as _;
                let _ = write!(output, "\\u{:04x}", character as u32);
            }
            character => output.push(character),
        }
    }
    output.push('"');
    output
}

fn syntax_error(body: &str, error: toml_edit::TomlError) -> ConfigError {
    if error.message().contains(']')
        && let Some(span) = error.span()
        && body.as_bytes().get(span.start) == Some(&b'\n')
    {
        let before = &body[..span.start];
        let line = before.bytes().filter(|byte| *byte == b'\n').count() + 1;
        let column = before
            .rfind('\n')
            .map_or(before.len() + 1, |index| before.len() - index);
        return ConfigError::Invalid(format!("at {line}:{column}: got line break, wanted \"]\""));
    }
    ConfigError::Invalid(format!("toml: {}", error.message()))
}
