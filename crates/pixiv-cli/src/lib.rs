mod artwork_list;
mod json_spool;
pub mod mutation;
mod novel_list;
pub mod novel_search;
pub mod novel_series;
pub mod ranking;
pub mod search;
pub mod trending;
pub mod user_search;

use pixiv_app::{execution::Execution, lifecycle::Context, scheduler::SchedulerError};
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
    Usage(String),
    MessageText(String),
    Output(io::Error),
    App(SchedulerError),
    State(Box<dyn std::error::Error + Send + Sync>),
    Pipeline,
}
impl From<SchedulerError> for CommandError {
    fn from(error: SchedulerError) -> Self {
        Self::App(error)
    }
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
            Self::Usage(message) => f.write_str(message),
            Self::MessageText(message) => f.write_str(message),
            Self::Output(error) => error.fmt(f),
            Self::App(error) => error.fmt(f),
            Self::State(error) => error.fmt(f),
            Self::Pipeline => f.write_str("pipeline records failed"),
        }
    }
}
impl std::error::Error for CommandError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sdk(error) => Some(error),
            Self::Output(error) => Some(error),
            Self::App(error) => Some(error),
            Self::State(error) => Some(error.as_ref()),
            Self::Message(_) | Self::Usage(_) | Self::MessageText(_) | Self::Pipeline => None,
        }
    }
}
impl CommandError {
    pub fn code(&self) -> &str {
        self.sdk_error()
            .map(|error| error.code.as_str())
            .unwrap_or("command_failed")
    }
    pub fn sdk_error(&self) -> Option<&Error> {
        match self {
            Self::Sdk(error) => Some(error),
            Self::App(error) => error.classified(),
            Self::Message(_)
            | Self::Usage(_)
            | Self::MessageText(_)
            | Self::Output(_)
            | Self::State(_)
            | Self::Pipeline => None,
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
    if matches!(error, CommandError::Pipeline) {
        return 1;
    }
    if matches!(error, CommandError::Usage(_)) {
        let _ = writeln!(diagnostics, "error: {error}");
        return 2;
    }
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
    write_artwork_detail(&artwork, mode, out)
}

pub async fn saved_artwork_detail<T: Transport + 'static, W: Write>(
    execution: &Execution<T>,
    context: &Context,
    id: i64,
    user_id: i64,
    proxy: Option<&str>,
    mode: DetailOutput,
    out: &mut W,
) -> Result<(), CommandError> {
    let artwork = execution
        .read(context, user_id, proxy, move |_, client| async move {
            client.artwork(id).await.map_err(Into::into)
        })
        .await?;
    write_artwork_detail(&artwork, mode, out)
}

fn write_artwork_detail<W: Write>(
    artwork: &pixiv_sdk::models::Artwork,
    mode: DetailOutput,
    out: &mut W,
) -> Result<(), CommandError> {
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
            let encoded = serde_json::to_string_pretty(&pixiv_sdk::dto::ArtworkDto::from(artwork))
                .map_err(|_| local())?;
            writeln!(out, "{}", go_json_escape(encoded))?;
        }
        DetailOutput::Ndjson => {
            let record = pixiv_record::from_artwork(artwork)
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

pub fn detail_artwork_id(source: &str) -> pixiv_sdk::Result<i64> {
    detail_entity_id(
        source,
        "artwork",
        pixiv_sdk::reference::REFERENCE_KIND_ARTWORK,
    )
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

pub async fn novel_detail<T: Transport, W: Write>(
    client: &Client<T>,
    id: i64,
    mode: DetailOutput,
    out: &mut W,
) -> Result<(), CommandError> {
    let novel = client
        .novel(pixiv_sdk::pixiv::NovelRequest { novel_id: id })
        .await?;
    write_novel_detail(&novel, mode, out)
}
pub async fn saved_novel_detail<T: Transport + 'static, W: Write>(
    execution: &Execution<T>,
    context: &Context,
    id: i64,
    user_id: i64,
    proxy: Option<&str>,
    mode: DetailOutput,
    out: &mut W,
) -> Result<(), CommandError> {
    let novel = execution
        .read(context, user_id, proxy, move |_, client| async move {
            client
                .novel(pixiv_sdk::pixiv::NovelRequest { novel_id: id })
                .await
                .map_err(Into::into)
        })
        .await?;
    write_novel_detail(&novel, mode, out)
}
fn write_novel_detail<W: Write>(
    novel: &pixiv_sdk::models::Novel,
    mode: DetailOutput,
    out: &mut W,
) -> Result<(), CommandError> {
    match mode {
        DetailOutput::Human => writeln!(out, "{} {} — {}", novel.id, novel.title, novel.user.name)?,
        DetailOutput::Json => {
            let encoded = serde_json::to_string_pretty(&pixiv_sdk::dto::NovelDto::from(novel))
                .map_err(|_| local())?;
            writeln!(out, "{}", go_json_escape(encoded))?;
        }
        DetailOutput::Ndjson => {
            let record = pixiv_record::from_novel(novel)
                .map_err(|error| CommandError::Message(error.message()))?;
            writeln!(
                out,
                "{}",
                go_json_escape(serde_json::to_string(&record).map_err(|_| local())?)
            )?;
        }
    }
    Ok(())
}
pub fn detail_novel_id(source: &str) -> pixiv_sdk::Result<i64> {
    detail_entity_id(source, "novel", pixiv_sdk::reference::REFERENCE_KIND_NOVEL)
}
fn detail_entity_id(source: &str, entity: &str, kind: &str) -> pixiv_sdk::Result<i64> {
    if let Ok(id) = source.trim().parse::<i64>()
        && id > 0
    {
        return Ok(id);
    }
    let reference = pixiv_sdk::reference::parse_url(source).map_err(|_| {
        Error::new(Reason::InvalidArgument, "detail")
            .with_detail("argument must be an entity ID or a supported Pixiv URL")
    })?;
    if reference.kind != kind {
        return Err(Error::new(Reason::InvalidArgument, "detail")
            .with_detail(format!("URL does not name a supported Pixiv {entity}")));
    }
    Ok(reference.id)
}

pub fn argument_error(error: &clap::Error) -> Option<CommandError> {
    if error.kind() != clap::error::ErrorKind::UnknownArgument {
        return None;
    }
    let value = error.get(clap::error::ContextKind::InvalidArg)?.to_string();
    let name = value.split('=').next()?;
    name.starts_with("--")
        .then(|| CommandError::Usage(format!("unknown option '{name}'")))
}

mod user_detail;
pub use user_detail::{saved_user_detail, user_detail};
pub fn detail_user_id(source: &str) -> pixiv_sdk::Result<i64> {
    detail_entity_id(source, "user", pixiv_sdk::reference::REFERENCE_KIND_USER)
}
