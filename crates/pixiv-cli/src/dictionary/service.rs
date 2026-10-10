use pixiv_app::lifecycle::Context;
use pixiv_sdk::{environment_proxy, oauth::LoginUrl, transport::encode_query_pairs};
use scraper::{ElementRef, Html};
use serde::{
    Deserialize, Deserializer,
    de::{IgnoredAny, MapAccess, Visitor},
};
use std::{error::Error as StdError, fmt, future::Future};

const ORIGIN: &str = "https://dic.pixiv.net";
pub const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/146.0.0.0 Safari/537.36";
pub type TransportError = Box<dyn StdError + Send + Sync>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorCode {
    Unknown,
    InvalidRequest,
    InvalidReference,
    NotFound,
    UpstreamStatus,
    MalformedResponse,
    Transport,
}
impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::InvalidRequest => "invalid_request",
            Self::InvalidReference => "invalid_reference",
            Self::NotFound => "not_found",
            Self::UpstreamStatus => "upstream_status",
            Self::MalformedResponse => "malformed_response",
            Self::Transport => "transport",
        }
    }
}

#[derive(Debug)]
pub struct Error {
    code: ErrorCode,
    status: u16,
    message: String,
    cause: Option<TransportError>,
}
impl Error {
    pub fn new(code: ErrorCode, message: impl Into<String>, cause: Option<TransportError>) -> Self {
        Self {
            code,
            status: 0,
            message: message.into(),
            cause,
        }
    }
    pub fn code(&self) -> ErrorCode {
        self.code
    }
    pub fn status_code(&self) -> u16 {
        self.status
    }
    fn status(code: ErrorCode, message: &str, status: u16) -> Self {
        Self {
            code,
            status,
            message: format!("{message} (HTTP {status})"),
            cause: None,
        }
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.cause
            .as_deref()
            .map(|cause| cause as &(dyn StdError + 'static))
    }
}
pub fn code_of(mut error: Option<&(dyn StdError + 'static)>) -> ErrorCode {
    while let Some(current) = error {
        if let Some(classified) = current.downcast_ref::<Error>() {
            return classified.code();
        }
        error = current.source();
    }
    ErrorCode::Unknown
}

#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}
pub trait Transport: Send + Sync {
    fn get(
        &self,
        context: Option<&Context>,
        raw_url: &str,
        accept: &str,
    ) -> impl Future<Output = Result<Response, TransportError>> + Send;
}

pub struct Client<T> {
    transport: Option<T>,
}
impl<T: Transport> Client<T> {
    pub fn new(transport: Option<T>) -> Self {
        Self { transport }
    }
    fn dependencies<'a>(
        &'a self,
        context: Option<&'a Context>,
    ) -> Result<(&'a T, &'a Context), Error> {
        let transport = self.transport.as_ref().ok_or_else(|| {
            Error::new(
                ErrorCode::Transport,
                "dic transport is not configured",
                None,
            )
        })?;
        let context = context.ok_or_else(|| {
            Error::new(
                ErrorCode::InvalidRequest,
                "dic request context is required",
                None,
            )
        })?;
        Ok((transport, context))
    }
    pub async fn article(
        &self,
        context: Option<&Context>,
        request: ArticleRequest,
    ) -> Result<Article, Error> {
        let (transport, context) = self.dependencies(context)?;
        let title = article_reference(&request.reference)?;
        let language = if request.language.is_empty() {
            "ja"
        } else {
            request.language.as_str()
        };
        if !matches!(language, "ja" | "en") {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "article language must be ja or en",
                None,
            ));
        }
        let response = transport
            .get(
                Some(context),
                &format!(
                    "{ORIGIN}/_api/get_article/{}?lang={language}",
                    path_escape(&title)
                ),
                "application/json",
            )
            .await
            .map_err(|cause| {
                Error::new(
                    ErrorCode::Transport,
                    "dic article request failed",
                    Some(cause),
                )
            })?;
        if response.status == 404 {
            return Err(Error::status(
                ErrorCode::NotFound,
                "dic.pixiv.net article was not found",
                response.status,
            ));
        }
        if !(200..300).contains(&response.status) {
            return Err(Error::status(
                ErrorCode::UpstreamStatus,
                "dic.pixiv.net article request returned an unexpected HTTP status",
                response.status,
            ));
        }
        let wire: ArticleDto = serde_json::from_slice(&response.body).map_err(|_| {
            Error::new(
                ErrorCode::MalformedResponse,
                "dic.pixiv.net article response could not be decoded",
                None,
            )
        })?;
        if wire.id <= 0 || go_trim(&wire.tag_name).is_empty() {
            return Err(Error::new(
                ErrorCode::MalformedResponse,
                "dic.pixiv.net article response is missing required fields",
                None,
            ));
        }
        let prefix = if language == "en" { "/en/a/" } else { "/a/" };
        let mut article = Article {
            id: wire.id,
            url: format!("{ORIGIN}{prefix}{}", path_escape(&wire.tag_name)),
            title: wire.tag_name,
            yomigana: wire.yomigana,
            translation: wire.translated_tag_name,
            categories: wire
                .categories
                .filter(|values| !values.is_empty())
                .map(|values| values.into_iter().map(|value| value.0).collect()),
            abstract_: wire.abstract_,
            related: wire
                .recommended_articles
                .unwrap_or_default()
                .into_iter()
                .map(|value| go_trim(&value.tag_name).to_owned())
                .filter(|value| !value.is_empty())
                .collect(),
            body: render_nodes(&wire.nodes),
            ..Article::default()
        };
        if !request.skip_counters
            && let Ok(response) = transport
                .get(
                    Some(context),
                    &format!(
                        "{ORIGIN}/_api/get_article_info/{}?lang={language}",
                        path_escape(&title)
                    ),
                    "application/json",
                )
                .await
            && (200..300).contains(&response.status)
            && let Ok(counters) = serde_json::from_slice::<CountersDto>(&response.body)
        {
            article.views = counters.article_view_count;
            article.works = counters.pixiv_work_count;
            article.comments = counters.comment_count;
            article.checklists = counters.checklist_count;
        }
        Ok(article)
    }
    pub async fn search(
        &self,
        context: Option<&Context>,
        request: SearchRequest,
    ) -> Result<Vec<SearchResult>, Error> {
        let (transport, context) = self.dependencies(context)?;
        if go_trim(&request.query).is_empty() {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "search query is required",
                None,
            ));
        }
        let query = encode_query_pairs(&[
            ("p".into(), request.page.max(1).to_string()),
            ("query".into(), request.query),
        ]);
        let response = transport
            .get(
                Some(context),
                &format!("{ORIGIN}/search?{query}"),
                "text/html",
            )
            .await
            .map_err(|cause| {
                Error::new(
                    ErrorCode::Transport,
                    "dic search request failed",
                    Some(cause),
                )
            })?;
        if response.status == 404 {
            return Ok(Vec::new());
        }
        if !(200..300).contains(&response.status) {
            return Err(Error::status(
                ErrorCode::UpstreamStatus,
                "dic.pixiv.net search returned an unexpected HTTP status",
                response.status,
            ));
        }
        parse_search(&response.body).ok_or_else(|| {
            Error::new(
                ErrorCode::MalformedResponse,
                "dic.pixiv.net search page could not be parsed",
                None,
            )
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct ArticleRequest {
    pub reference: String,
    pub language: String,
    pub skip_counters: bool,
}
#[derive(Clone, Debug, Default)]
pub struct SearchRequest {
    pub query: String,
    pub page: i64,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Article {
    pub id: i64,
    pub title: String,
    pub yomigana: String,
    pub translation: String,
    pub categories: Option<Vec<String>>,
    pub abstract_: String,
    pub related: Vec<String>,
    pub body: String,
    pub views: i64,
    pub works: i64,
    pub comments: i64,
    pub checklists: i64,
    pub url: String,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SearchResult {
    pub title: String,
    pub summary: String,
    pub updated: String,
    pub views: i64,
    pub works: i64,
    pub checklists: i64,
    pub related: Option<Vec<String>>,
    pub url: String,
    pub thumbnail: String,
}

fn article_reference(reference: &str) -> Result<String, Error> {
    let trimmed = go_trim(reference);
    if trimmed.is_empty() {
        return Err(Error::new(
            ErrorCode::InvalidReference,
            "article reference is empty",
            None,
        ));
    }
    let Some(parsed) = LoginUrl::parse(trimmed).filter(|value| !value.host().is_empty()) else {
        return Ok(trimmed.to_owned());
    };
    let host = parsed.host();
    let hostname = host
        .rsplit_once(':')
        .filter(|(_, port)| port.bytes().all(|byte| byte.is_ascii_digit()))
        .map(|(host, _)| host)
        .unwrap_or(host);
    if !hostname.eq_ignore_ascii_case("dic.pixiv.net") {
        return Err(Error::new(
            ErrorCode::InvalidReference,
            "article reference must be a dic.pixiv.net URL or a bare title",
            None,
        ));
    }
    let segments: Vec<_> = parsed.path().trim_matches('/').split('/').collect();
    for (index, segment) in segments.iter().enumerate() {
        if *segment == "a" {
            let title = segments[index + 1..].join("/");
            if let Some(title) = path_unescape(&title).filter(|value| !value.is_empty()) {
                return Ok(title);
            }
            break;
        }
    }
    Err(Error::new(
        ErrorCode::InvalidReference,
        "article URL has no article title",
        None,
    ))
}
fn path_escape(value: &str) -> String {
    let mut escaped = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~$&+:=@".contains(&byte) {
            escaped.push(char::from(byte));
        } else {
            escaped.push('%');
            escaped.push(char::from(b"0123456789ABCDEF"[usize::from(byte >> 4)]));
            escaped.push(char::from(b"0123456789ABCDEF"[usize::from(byte & 15)]));
        }
    }
    escaped
}
fn path_unescape(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = char::from(*bytes.get(index + 1)?).to_digit(16)?;
            let low = char::from(*bytes.get(index + 2)?).to_digit(16)?;
            decoded.push((high * 16 + low) as u8);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    Some(String::from_utf8_lossy(&decoded).into_owned())
}
fn go_space(value: char) -> bool {
    matches!(value, '\u{0009}'..='\u{000d}' | ' ' | '\u{0085}' | '\u{00a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}')
}
fn go_trim(value: &str) -> &str {
    value.trim_matches(go_space)
}
fn collapsed(value: &str) -> String {
    value
        .split(go_space)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Default)]
struct NullableString(String);
impl<'de> Deserialize<'de> for NullableString {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self(
            Option::<String>::deserialize(deserializer)?.unwrap_or_default(),
        ))
    }
}
macro_rules! wire_dto {
    ($name:ident { $( $key:literal => $field:ident : $kind:ty ),* $(,)? } collections { $( $listkey:literal => $listfield:ident : $listkind:ty ),* $(,)? }) => {
        #[derive(Default)]
        struct $name { $( $field: $kind, )* $( $listfield: Option<Vec<$listkind>>, )* }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                struct DtoVisitor;
                impl<'de> Visitor<'de> for DtoVisitor {
                    type Value = $name;
                    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("an object or null") }
                    fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> { Ok($name::default()) }
                    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                        let mut value = $name::default();
                        while let Some(key) = map.next_key::<String>()? {
                            match key.to_ascii_lowercase().as_str() {
                                $( $key => { if let Some(field) = map.next_value::<Option<$kind>>()? { value.$field = field; } }, )*
                                $( $listkey => { value.$listfield = map.next_value::<Option<Vec<$listkind>>>()?; }, )*
                                _ => { map.next_value::<IgnoredAny>()?; }
                            }
                        }
                        Ok(value)
                    }
                }
                deserializer.deserialize_any(DtoVisitor)
            }
        }
    };
}
wire_dto!(ArticleDto {
    "id" => id: i64, "tagname" => tag_name: String, "yomigana" => yomigana: String,
    "translatedtagname" => translated_tag_name: String, "abstract" => abstract_: String, "nodes" => nodes: String,
} collections { "categories" => categories: NullableString, "recommendedarticles" => recommended_articles: RelatedDto });
wire_dto!(RelatedDto { "tagname" => tag_name: String } collections {});
wire_dto!(CountersDto {
    "articleviewcount" => article_view_count: i64, "commentcount" => comment_count: i64,
    "pixivworkcount" => pixiv_work_count: i64, "checklistcount" => checklist_count: i64,
} collections {});
wire_dto!(NodeDto { "tag" => tag: String, "text" => text: String } collections { "children" => children: Option<NodeDto> });

fn render_nodes(raw: &str) -> String {
    if go_trim(raw).is_empty() {
        return String::new();
    }
    let Ok(nodes) = serde_json::from_str::<Option<Vec<Option<NodeDto>>>>(raw) else {
        return String::new();
    };
    let mut rendered = String::new();
    render_children(nodes.as_deref().unwrap_or_default(), &mut rendered);
    let mut cleaned = Vec::new();
    let mut blank = true;
    let normalized = rendered.replace("\r\n", "\n");
    for line in normalized.split('\n') {
        let line = go_trim(line);
        if line.is_empty() {
            if blank {
                continue;
            }
            blank = true;
        } else {
            blank = false;
        }
        cleaned.push(line);
    }
    go_trim(&cleaned.join("\n")).to_owned()
}
fn render_children(nodes: &[Option<NodeDto>], output: &mut String) {
    for node in nodes.iter().flatten() {
        let children = node.children.as_deref().unwrap_or_default();
        match node.tag.as_str() {
            "text" | "article_link" | "external_link" => output.push_str(&node.text),
            "br" => output.push('\n'),
            "header" | "sub_header" => {
                output.push_str(if node.tag == "header" {
                    "\n\n## "
                } else {
                    "\n\n### "
                });
                render_children(children, output);
                output.push('\n');
            }
            "p" => {
                output.push_str("\n\n");
                render_children(children, output);
                output.push('\n');
            }
            "list_item" => {
                output.push_str("\n- ");
                render_children(children, output);
            }
            "table_row" => {
                output.push('\n');
                render_children(children, output);
            }
            "table_header" | "table_cell" => {
                render_children(children, output);
                output.push_str(" | ");
            }
            _ => render_children(children, output),
        }
    }
}

fn first<'a>(
    root: ElementRef<'a>,
    predicate: &impl Fn(ElementRef<'a>) -> bool,
) -> Option<ElementRef<'a>> {
    if predicate(root) {
        return Some(root);
    }
    root.children()
        .filter_map(ElementRef::wrap)
        .find_map(|child| first(child, predicate))
}
fn each<'a>(root: ElementRef<'a>, tag: &str, visit: &mut impl FnMut(ElementRef<'a>)) {
    if root.value().name() == tag {
        visit(root);
        return;
    }
    for child in root.children().filter_map(ElementRef::wrap) {
        each(child, tag, visit);
    }
}
fn has_class(element: ElementRef<'_>, class: &str) -> bool {
    element
        .attr("class")
        .unwrap_or_default()
        .split(go_space)
        .any(|value| value == class)
}
fn element_text(element: ElementRef<'_>) -> String {
    collapsed(&element.text().collect::<String>())
}
fn text_before(element: ElementRef<'_>, tag: &str) -> String {
    let mut text = String::new();
    for node in element.descendants() {
        if node
            .value()
            .as_element()
            .is_some_and(|element| element.name() == tag)
        {
            break;
        }
        if let Some(value) = node.value().as_text() {
            text.push_str(value);
        }
    }
    collapsed(&text)
}
fn parse_search(body: &[u8]) -> Option<Vec<SearchResult>> {
    let text = String::from_utf8_lossy(body);
    let document = Html::parse_document(&text);
    let main = first(document.root_element(), &|element| {
        element.value().name() == "div" && element.attr("id") == Some("main")
    })?;
    let mut results = Vec::new();
    each(main, "article", &mut |card| {
        if let Some(result) = search_card(card) {
            results.push(result);
        }
    });
    Some(results)
}
fn search_card(card: ElementRef<'_>) -> Option<SearchResult> {
    let info = first(card, &|element| {
        element.value().name() == "div" && has_class(element, "info")
    })?;
    let link = first(info, &|element| {
        element.value().name() == "a" && element.attr("href").is_some_and(|value| !value.is_empty())
    })?;
    let title = element_text(link);
    if title.is_empty() {
        return None;
    }
    let href = link.attr("href").unwrap_or_default();
    let url = if href.starts_with("http://") || href.starts_with("https://") {
        href.to_owned()
    } else {
        format!("{ORIGIN}/{}", href.strip_prefix('/').unwrap_or(href))
    };
    let mut result = SearchResult {
        title,
        url,
        ..SearchResult::default()
    };
    if let Some(image) = first(card, &|element| element.value().name() == "img") {
        result.thumbnail = image.attr("src").unwrap_or_default().to_owned();
    }
    if let Some(summary) = first(info, &|element| {
        element.value().name() == "p" && has_class(element, "summary")
    }) {
        result.summary = text_before(summary, "a");
    }
    if let Some(data) = first(info, &|element| {
        element.value().name() == "ul" && has_class(element, "data")
    }) {
        each(data, "li", &mut |item| {
            let text = element_text(item);
            let Some((label, value)) = text.split_once(':') else {
                return;
            };
            match go_trim(label) {
                "更新" => result.updated = go_trim(value).to_owned(),
                "閲覧数" => result.views = display_int(value),
                "作品数" => result.works = display_int(value),
                "チェックリスト数" => result.checklists = display_int(value),
                _ => {}
            }
        });
    }
    if let Some(relation) = first(info, &|element| {
        element.value().name() == "div" && has_class(element, "relation")
    }) {
        each(relation, "li", &mut |item| {
            let text = element_text(item);
            if !text.is_empty() {
                result.related.get_or_insert_default().push(text);
            }
        });
    }
    Some(result)
}
fn display_int(value: &str) -> i64 {
    let digits: String = value.chars().filter(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return 0;
    }
    digits.parse().unwrap_or(i64::MAX)
}

#[derive(Debug)]
pub struct HttpError {
    status: u16,
    message: String,
    cause: Option<TransportError>,
}
impl HttpError {
    pub fn status_code(&self) -> u16 {
        self.status
    }
    fn wrapped(status: u16, message: String, cause: TransportError) -> Self {
        Self {
            status,
            message,
            cause: Some(cause),
        }
    }
}
impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl StdError for HttpError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.cause
            .as_deref()
            .map(|error| error as &(dyn StdError + 'static))
    }
}

fn body_error_message(error: &reqwest::Error) -> String {
    let mut current: Option<&(dyn StdError + 'static)> = Some(error);
    while let Some(cause) = current {
        if cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|error| error.kind() == std::io::ErrorKind::UnexpectedEof)
        {
            return "unexpected EOF".to_owned();
        }
        current = cause.source();
    }
    error.to_string()
}

#[derive(Clone)]
pub struct HttpTransport {
    client: reqwest::Client,
    user_agent: String,
    environment_proxy: bool,
    default_redirects: bool,
}
impl HttpTransport {
    pub fn new() -> Result<Self, TransportError> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .gzip(true)
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .no_proxy()
            .proxy(reqwest::Proxy::custom(|url| {
                environment_proxy::ProxyEnvironment::cached()
                    .proxy_for_url(url.as_str())
                    .ok()
                    .flatten()
                    .and_then(environment_proxy::reqwest_proxy)
            }))
            .build()?;
        Ok(Self {
            client,
            user_agent: DEFAULT_USER_AGENT.to_owned(),
            environment_proxy: true,
            default_redirects: true,
        })
    }
    pub fn with_client(client: reqwest::Client, user_agent: String) -> Self {
        Self {
            client,
            user_agent: if user_agent.is_empty() {
                DEFAULT_USER_AGENT.to_owned()
            } else {
                user_agent
            },
            environment_proxy: false,
            default_redirects: false,
        }
    }
    pub fn with_user_agent(mut self, user_agent: impl Into<String>) -> Self {
        let user_agent = user_agent.into();
        self.user_agent = if user_agent.is_empty() {
            DEFAULT_USER_AGENT.to_owned()
        } else {
            user_agent
        };
        self
    }
    async fn request(&self, raw_url: &str, accept: &str) -> Result<Response, TransportError> {
        let mut original = self
            .client
            .get(raw_url)
            .header(reqwest::header::USER_AGENT, &self.user_agent)
            .header(reqwest::header::ACCEPT, accept)
            .build()?;
        let mut previous = original.url().clone();
        for sent in 1..=10 {
            if self.environment_proxy
                && let Some(proxy) = environment_proxy::ProxyEnvironment::cached()
                    .proxy_for_url(original.url().as_str())?
            {
                let proxy = environment_proxy::reqwest_proxy(proxy)
                    .ok_or_else(|| std::io::Error::other("invalid dictionary environment proxy"))?;
                reqwest::Proxy::all(proxy)?;
            }
            let request = original
                .try_clone()
                .ok_or_else(|| std::io::Error::other("dic request could not be cloned"))?;
            let response = self.client.execute(request).await?;
            let status = response.status().as_u16();
            if self.default_redirects
                && matches!(status, 301 | 302 | 303 | 307 | 308)
                && let Some(location) = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .filter(|value| !value.is_empty())
            {
                if sent == 10 {
                    let cause: TransportError =
                        Box::new(std::io::Error::other("stopped after 10 redirects"));
                    return Err(Box::new(HttpError::wrapped(
                        0,
                        format!("Get {location:?}: stopped after 10 redirects"),
                        cause,
                    )));
                }
                let next = previous.join(location)?;
                if previous.scheme() == "https" && next.scheme() == "http" {
                    original.headers_mut().remove(reqwest::header::REFERER);
                } else {
                    let mut referer = previous.clone();
                    let _ = referer.set_username("");
                    let _ = referer.set_password(None);
                    original.headers_mut().insert(
                        reqwest::header::REFERER,
                        reqwest::header::HeaderValue::from_str(referer.as_str())?,
                    );
                }
                *original.url_mut() = next.clone();
                previous = next;
                continue;
            }
            let body = response.bytes().await.map_err(|error| {
                Box::new(HttpError::wrapped(
                    status,
                    body_error_message(&error),
                    Box::new(error),
                )) as TransportError
            })?;
            return Ok(Response {
                status,
                body: body.to_vec(),
            });
        }
        unreachable!()
    }
}
impl Transport for HttpTransport {
    async fn get(
        &self,
        context: Option<&Context>,
        raw_url: &str,
        accept: &str,
    ) -> Result<Response, TransportError> {
        let context = context.ok_or_else(|| {
            Box::new(HttpError::wrapped(
                0,
                "create dic request: net/http: nil Context".to_owned(),
                Box::new(std::io::Error::other("net/http: nil Context")),
            )) as TransportError
        })?;
        tokio::select! {
            biased;
            error = context.cancelled() => Err(Box::new(HttpError::wrapped(0, format!("Get {raw_url:?}: {error}"), Box::new(error))) as TransportError),
            response = self.request(raw_url, accept) => response,
        }
    }
}
