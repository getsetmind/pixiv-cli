pub mod search;

use pixiv_sdk::{Client, Error, Reason, transport::Transport};
use std::{
    fmt,
    io::{self, Write},
};

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
    Output(io::Error),
}
impl From<io::Error> for CommandError {
    fn from(error: io::Error) -> Self {
        Self::Output(error)
    }
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
            Self::Output(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for CommandError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sdk(error) => Some(error),
            Self::Output(error) => Some(error),
            Self::Message(_) => None,
        }
    }
}
impl CommandError {
    pub fn code(&self) -> &str {
        match self {
            Self::Sdk(error) => error.code.as_str(),
            Self::Message(_) | Self::Output(_) => "command_failed",
        }
    }
    pub fn sdk_error(&self) -> Option<&Error> {
        match self {
            Self::Sdk(error) => Some(error),
            Self::Message(_) | Self::Output(_) => None,
        }
    }
}

pub fn finish_command<W: Write>(
    result: Result<(), CommandError>,
    ndjson_output: bool,
    machine_output: bool,
    diagnostics: &mut W,
) -> i32 {
    let Err(error) = result else {
        return 0;
    };
    if ndjson_output
        && matches!(&error, CommandError::Output(cause) if cause.kind() == io::ErrorKind::BrokenPipe)
    {
        return 0;
    }
    if machine_output {
        let mut body = serde_json::json!({"code": error.code(), "message": error.to_string()});
        if let Some(seconds) = error
            .sdk_error()
            .and_then(|error| error.retry_after_seconds_at(std::time::SystemTime::now().into()))
            .filter(|seconds| *seconds > 0)
        {
            body["retry_after_seconds"] = seconds.into();
        }
        let envelope = go_json_escape(serde_json::json!({"error": body}).to_string());
        if writeln!(diagnostics, "{envelope}").is_ok() {
            return 1;
        }
    }
    let _ = writeln!(diagnostics, "error: {error}");
    1
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
            write!(
                out,
                "url: https://www.pixiv.net/artworks/{}\nid: {}\ntitle: {}\nauthor: {} ({})\ntype: {}\npage_count: {}\nbookmarks: {}\nviews: {}\ntags: {}\n",
                artwork.id,
                artwork.id,
                artwork.title,
                artwork.user.name,
                artwork.user.id,
                kind.as_str().unwrap_or_default(),
                artwork.page_count,
                artwork.total_bookmarks,
                artwork.total_views,
                tags
            )?;
            let caption = plain_caption(&artwork.caption);
            if !caption.is_empty() {
                writeln!(out, "caption:\n{caption}")?;
            }
        }
        DetailOutput::Json => {
            let encoded = serde_json::to_string_pretty(&pixiv_sdk::dto::ArtworkDto::from(&artwork))
                .map_err(|_| local())?;
            writeln!(out, "{}", go_json_escape(encoded))?;
        }
        DetailOutput::Ndjson => {
            let record = pixiv_record::from_artwork(&artwork)
                .map_err(|error| CommandError::Message(error.message()))?;
            let encoded = serde_json::to_string(&record).map_err(|_| local())?;
            writeln!(out, "{}", go_json_escape(encoded))?;
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
