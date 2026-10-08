use pixiv_sdk::{Client, Error, Reason, models::ArtworkKind, transport::Transport};
use std::{fmt, io::Write};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DetailOutput {
    Human,
    Json,
    Ndjson,
}

#[derive(Debug)]
pub enum CommandError {
    Sdk(Error),
    Message(&'static str),
}
impl From<Error> for CommandError {
    fn from(error: Error) -> Self {
        Self::Sdk(error)
    }
}
impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sdk(error) => error.fmt(f),
            Self::Message(message) => f.write_str(message),
        }
    }
}
impl std::error::Error for CommandError {}
impl CommandError {
    pub fn code(&self) -> &str {
        match self {
            Self::Sdk(error) => error.code.as_str(),
            Self::Message(_) => "command_failed",
        }
    }
    pub fn sdk_error(&self) -> Option<&Error> {
        match self {
            Self::Sdk(error) => Some(error),
            Self::Message(_) => None,
        }
    }
}

pub async fn artwork_detail<T: Transport, W: Write>(
    client: &Client<T>,
    id: i64,
    mode: DetailOutput,
    out: &mut W,
) -> Result<(), CommandError> {
    let artwork = client.artwork(id).await?;
    match mode {
        DetailOutput::Human => {
            let kind = serde_json::to_value(&artwork.kind).map_err(|_| local())?;
            let tags = artwork
                .tags
                .iter()
                .map(|tag| tag.name.as_str())
                .collect::<Vec<_>>()
                .join(",");
            write!(out,"url: https://www.pixiv.net/artworks/{}\nid: {}\ntitle: {}\nauthor: {} ({})\ntype: {}\npage_count: {}\nbookmarks: {}\nviews: {}\ntags: {}\n",artwork.id,artwork.id,artwork.title,artwork.user.name,artwork.user.id,kind.as_str().unwrap_or_default(),artwork.page_count,artwork.total_bookmarks,artwork.total_views,tags).map_err(|_|local())?;
            let caption = plain_caption(&artwork.caption);
            if !caption.is_empty() {
                writeln!(out, "caption:\n{caption}").map_err(|_| local())?;
            }
        }
        DetailOutput::Json => {
            let encoded = serde_json::to_string_pretty(&pixiv_sdk::dto::ArtworkDto::from(&artwork))
                .map_err(|_| local())?;
            writeln!(out, "{}", go_json_escape(encoded)).map_err(|_| local())?;
        }
        DetailOutput::Ndjson => {
            let record_type = match artwork.kind {
                ArtworkKind::Illust => "illust",
                ArtworkKind::Manga => "manga",
                ArtworkKind::Ugoira => "ugoira",
                ArtworkKind::Unknown => {
                    return Err(CommandError::Message("unsupported artwork kind for record"));
                }
            };
            if artwork.id <= 0 {
                return Err(CommandError::Message("record id must be positive"));
            }
            let mut record = serde_json::to_value(pixiv_sdk::dto::ArtworkDto::from(&artwork))
                .map_err(|_| local())?;
            record["id"] = artwork.id.to_string().into();
            record["type"] = record_type.into();
            record["url"] = format!("https://www.pixiv.net/artworks/{}", artwork.id).into();
            let encoded = serde_json::to_string(&record).map_err(|_| local())?;
            writeln!(out, "{}", go_json_escape(encoded)).map_err(|_| local())?;
        }
    }
    Ok(())
}
fn local() -> Error {
    Error::new(Reason::LocalStateError, "output")
}
fn go_json_escape(value: String) -> String {
    value
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}
fn plain_caption(raw: &str) -> String {
    let document = scraper::Html::parse_document(raw);
    let mut out = String::new();
    let mut stack = vec![(document.tree.root().id(), false)];
    while let Some((id, exit)) = stack.pop() {
        let node = document.tree.get(id).expect("caption tree node exists");
        match node.value() {
            scraper::Node::Text(text) if !exit => {
                out.push_str(text);
                continue;
            }
            scraper::Node::Element(element) => {
                let name = element.name();
                if matches!(name, "script" | "style") {
                    continue;
                }
                if name == "br"
                    || (exit
                        && matches!(
                            name,
                            "p" | "div"
                                | "li"
                                | "blockquote"
                                | "pre"
                                | "h1"
                                | "h2"
                                | "h3"
                                | "h4"
                                | "h5"
                                | "h6"
                        ))
                {
                    if !out.is_empty() && !out.ends_with('\n') {
                        out.push('\n');
                    }
                    continue;
                }
            }
            _ => {}
        }
        if !exit {
            stack.push((id, true));
            let children = node
                .children()
                .map(|child| (child.id(), false))
                .collect::<Vec<_>>();
            stack.extend(children.into_iter().rev());
        }
    }
    out.trim()
        .split('\n')
        .map(|line| safe_line(line.trim()))
        .collect::<Vec<_>>()
        .join("\n")
}
fn safe_line(value: &str) -> String {
    let mut result = String::new();
    for ch in value.chars() {
        match ch {
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\t' => result.push_str("\\t"),
            '\r' => result.push_str("\\r"),
            '\n' => result.push_str("\\n"),
            '\u{7}' => result.push_str("\\a"),
            '\u{8}' => result.push_str("\\b"),
            '\u{c}' => result.push_str("\\f"),
            '\u{b}' => result.push_str("\\v"),
            ch if ch < ' ' || ch == '\u{7f}' => result.push_str(&format!("\\x{:02x}", ch as u32)),
            ch if !graphic(ch) => {
                let code = ch as u32;
                if code <= 0xffff {
                    result.push_str(&format!("\\u{code:04x}"));
                } else {
                    result.push_str(&format!("\\U{code:08x}"));
                }
            }
            ch => result.push(ch),
        }
    }
    result
}

fn graphic(ch: char) -> bool {
    use unicode_general_category::{GeneralCategory as C, get_general_category};
    !matches!(
        get_general_category(ch),
        C::Control
            | C::Format
            | C::Surrogate
            | C::PrivateUse
            | C::Unassigned
            | C::LineSeparator
            | C::ParagraphSeparator
    )
}
