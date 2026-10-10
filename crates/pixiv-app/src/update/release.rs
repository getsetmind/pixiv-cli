use super::{
    CallerContext, ExternalError, Release, ReleaseAsset, ReleaseCache, ReleaseCheckOptions,
    ReleaseCheckResult, ReleaseChecker, UpdateFuture, check_context, go_quote, message,
    source::{ReleaseSource, ReleaseSourceKind, ReleaseSourceSelector},
    wrap,
};
use crate::reverse_search::http::{HttpRequest, HttpTransport};
use chrono::{DateTime, Datelike, FixedOffset, Utc};
use pixiv_sdk::{
    context::{ContextError, ContextFuture, ContextKey, ContextValue, RequestContext},
    diagnostics::Scope,
    fanbox::transport::{Headers, RawBody, RawResponse},
};
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use std::{
    any::TypeId,
    cmp::Ordering,
    collections::{HashMap, HashSet},
    fmt,
    sync::Arc,
    time::{Duration, Instant},
};

pub const DEFAULT_GITHUB_REPOSITORY: &str = "FlanChanXwO/pixiv-cli";
pub const CACHE_FILENAME: &str = "github-releases.json";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticVersion {
    numbers: [String; 3],
    prerelease: Vec<String>,
    build: Vec<String>,
}
impl SemanticVersion {
    pub fn parse(tag: &str) -> Result<Self, ExternalError> {
        let text = tag
            .strip_prefix('v')
            .ok_or_else(|| message("must start with v"))?;
        if text.is_empty() {
            return Err(message("missing version"));
        }
        let (main_pre, build) = text
            .split_once('+')
            .map_or((text, None), |(a, b)| (a, Some(b)));
        if let Some(b) = build
            && !valid_identifiers(b, false)
        {
            return Err(message(format!("invalid build metadata {}", go_quote(b))));
        }
        let (main, prerelease) = main_pre
            .split_once('-')
            .map_or((main_pre, None), |(a, b)| (a, Some(b)));
        if let Some(p) = prerelease
            && !valid_identifiers(p, true)
        {
            return Err(message(format!("invalid prerelease {}", go_quote(p))));
        }
        let parts: Vec<_> = main.split('.').collect();
        if parts.len() != 3 {
            return Err(message("must contain major.minor.patch"));
        }
        for (index, part) in parts.iter().enumerate() {
            if !numeric(part) || (part.len() > 1 && part.starts_with('0')) {
                return Err(wrap(
                    format!("invalid {} version", ["major", "minor", "patch"][index]),
                    message(format!(
                        "{} is not a canonical numeric identifier",
                        go_quote(part)
                    )),
                ));
            }
        }
        Ok(Self {
            numbers: [parts[0].into(), parts[1].into(), parts[2].into()],
            prerelease: prerelease
                .map_or_else(Vec::new, |p| p.split('.').map(str::to_owned).collect()),
            build: build.map_or_else(Vec::new, |b| b.split('.').map(str::to_owned).collect()),
        })
    }
    pub fn is_prerelease(&self) -> bool {
        !self.prerelease.is_empty()
    }
    pub fn compare(&self, other: &Self) -> i32 {
        for (a, b) in self.numbers.iter().zip(&other.numbers) {
            let order = decimal_order(a, b);
            if order != Ordering::Equal {
                return sign(order);
            }
        }
        if !self.is_prerelease() || !other.is_prerelease() {
            return sign(other.is_prerelease().cmp(&self.is_prerelease()));
        }
        for (a, b) in self.prerelease.iter().zip(&other.prerelease) {
            let order = match (numeric(a), numeric(b)) {
                (true, true) => decimal_order(a, b),
                (true, false) => Ordering::Less,
                (false, true) => Ordering::Greater,
                (false, false) => a.cmp(b),
            };
            if order != Ordering::Equal {
                return sign(order);
            }
        }
        sign(self.prerelease.len().cmp(&other.prerelease.len()))
    }
}
impl fmt::Display for SemanticVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}.{}.{}",
            self.numbers[0], self.numbers[1], self.numbers[2]
        )?;
        if self.is_prerelease() {
            write!(f, "-{}", self.prerelease.join("."))?;
        }
        if !self.build.is_empty() {
            write!(f, "+{}", self.build.join("."))?;
        }
        Ok(())
    }
}
fn sign(order: Ordering) -> i32 {
    match order {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}
fn numeric(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit())
}
fn decimal_order(a: &str, b: &str) -> Ordering {
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}
fn valid_identifiers(value: &str, leading_zero: bool) -> bool {
    !value.is_empty()
        && value.split('.').all(|part| {
            !part.is_empty()
                && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && !(leading_zero && numeric(part) && part.len() > 1 && part.starts_with('0'))
        })
}

pub type ReleaseClock = Arc<dyn Fn() -> DateTime<FixedOffset> + Send + Sync>;
#[derive(Default)]
pub struct ReleaseClientOptions {
    pub api_base_url: String,
    pub repository: String,
    pub transport: Option<Arc<dyn HttpTransport>>,
    pub cache: Option<Arc<dyn ReleaseCache>>,
    pub now: Option<ReleaseClock>,
    pub enable_public_release_sources: bool,
    pub source_selector: Option<Arc<ReleaseSourceSelector>>,
}
pub struct GitHubReleaseClient {
    endpoint: PageUrl,
    transport: Arc<dyn HttpTransport>,
    cache: Arc<dyn ReleaseCache>,
    now: ReleaseClock,
    selector: Option<Arc<ReleaseSourceSelector>>,
}
impl GitHubReleaseClient {
    pub fn new(options: ReleaseClientOptions) -> Result<Self, ExternalError> {
        let cache = options
            .cache
            .ok_or_else(|| message("release cache is required"))?;
        let base = if options.api_base_url.is_empty() {
            "https://api.github.com"
        } else {
            &options.api_base_url
        };
        let mut endpoint = PageUrl::parse(base).map_err(|e| {
            wrap(
                format!("parse GitHub API base URL {}", go_quote(base)),
                wrap(format!("parse {}", go_quote(base)), e),
            )
        })?;
        if endpoint.scheme.is_empty() || endpoint.host.is_empty() {
            return Err(message(format!(
                "GitHub API base URL {} must be absolute",
                go_quote(base)
            )));
        }
        let repository = if options.repository.is_empty() {
            DEFAULT_GITHUB_REPOSITORY
        } else {
            &options.repository
        };
        let (owner, name) = repository
            .split_once('/')
            .filter(|(a, b)| !a.is_empty() && !b.is_empty() && !b.contains('/'))
            .ok_or_else(|| {
                message(format!(
                    "GitHub repository {} must have owner/name form",
                    go_quote(repository)
                ))
            })?;
        endpoint.raw_path = escape_path(&clean_path(&format!(
            "{}/repos/{owner}/{name}/releases",
            endpoint.path()
        )));
        let transport = options
            .transport
            .unwrap_or_else(super::http::default_transport);
        let selector = options.source_selector.or_else(|| {
            options
                .enable_public_release_sources
                .then(|| Arc::new(ReleaseSourceSelector::default(transport.clone())))
        });
        Ok(Self {
            endpoint,
            transport,
            cache,
            now: options
                .now
                .unwrap_or_else(|| Arc::new(|| Utc::now().fixed_offset())),
            selector,
        })
    }
    pub async fn check(
        &self,
        context: CallerContext,
        options: ReleaseCheckOptions,
    ) -> Result<ReleaseCheckResult, ExternalError> {
        let context = if options.automatic {
            Arc::new(DeadlineContext::new(context)) as CallerContext
        } else {
            context
        };
        let (previous, exists) = self.read_cache(context.clone()).await?;
        check_context(&context, "read GitHub Releases cache")?;
        let now = (self.now)();
        if options.automatic
            && exists
            && previous.schema_version == 2
            && previous.checked_at.as_ref().is_some_and(|checked| {
                now.signed_duration_since(*checked) < chrono::Duration::hours(24)
            })
        {
            return Ok(ReleaseCheckResult {
                release: select_release(
                    previous.releases.as_deref().unwrap_or_default(),
                    options.include_prerelease,
                )?,
                throttled: true,
            });
        }
        let mut refreshed = self.fetch_cache(context.clone(), previous).await?;
        check_context(&context, "fetch GitHub Releases pages")?;
        let selected = select_release(
            refreshed.releases.as_deref().unwrap_or_default(),
            options.include_prerelease,
        )?;
        refreshed.checked_at = Some(now);
        let encoded =
            encode_cache(&refreshed).map_err(|e| wrap("encode GitHub Releases cache", e))?;
        check_context(&context, "write GitHub Releases cache")?;
        self.cache.write(context, encoded).await?;
        Ok(ReleaseCheckResult {
            release: selected,
            throttled: false,
        })
    }
    async fn read_cache(
        &self,
        context: CallerContext,
    ) -> Result<(StoredCache, bool), ExternalError> {
        check_context(&context, "read GitHub Releases cache")?;
        let Some(bytes) = self.cache.read(context.clone()).await? else {
            return Ok((StoredCache::default(), false));
        };
        let cache = decode_cache(&bytes).map_err(|e| wrap("decode GitHub Releases cache", e))?;
        if cache.checked_at.is_none_or(|date| {
            date.timestamp() == -62135596800 && date.timestamp_subsec_nanos() == 0
        }) {
            return Err(message("decode GitHub Releases cache: missing checked_at"));
        }
        check_context(&context, "read GitHub Releases cache")?;
        Ok((cache, true))
    }
    async fn fetch_cache(
        &self,
        context: CallerContext,
        previous: StoredCache,
    ) -> Result<StoredCache, ExternalError> {
        let mut cached = HashMap::new();
        if previous.schema_version == 2 {
            for page in previous.pages {
                let parsed = self.parse_page(&page.url).map_err(|e| {
                    wrap(
                        format!(
                            "validate cached GitHub Releases page {}",
                            go_quote(&page.url)
                        ),
                        e,
                    )
                })?;
                if cached.contains_key(&parsed.canonical()) {
                    return Err(message(format!(
                        "cached GitHub Releases page {} appears more than once",
                        go_quote(&page.url)
                    )));
                }
                cached.insert(parsed.canonical(), page);
            }
        }
        let sources = match &self.selector {
            Some(selector) => {
                selector
                    .ordered(
                        context.clone(),
                        ReleaseSourceKind::API,
                        &self.endpoint.to_string(),
                    )
                    .await?
            }
            None => Vec::new(),
        };
        let mut current = self.endpoint.clone();
        let mut visited = HashSet::new();
        let mut refreshed = StoredCache {
            schema_version: 2,
            ..StoredCache::default()
        };
        loop {
            check_context(&context, "fetch GitHub Releases page")?;
            let id = current.canonical();
            if !visited.insert(id.clone()) {
                return Err(message(format!(
                    "GitHub Releases pagination loop at {}",
                    go_quote(&id)
                )));
            }
            let (page, next) = self
                .fetch_page(context.clone(), &current, cached.get(&id), sources.first())
                .await?;
            if let Some(releases) = &page.releases
                && !releases.is_empty()
            {
                refreshed
                    .releases
                    .get_or_insert_with(Vec::new)
                    .extend(releases.iter().cloned());
            }
            refreshed.pages.push(page);
            match next {
                Some(next) => current = next,
                None => return Ok(refreshed),
            }
        }
    }
    async fn fetch_page(
        &self,
        context: CallerContext,
        url: &PageUrl,
        cached: Option<&StoredPage>,
        source: Option<&ReleaseSource>,
    ) -> Result<(StoredPage, Option<PageUrl>), ExternalError> {
        let canonical = url.to_string();
        let route = match source {
            Some(source) => source.api_url(&canonical).map_err(|e| {
                wrap(
                    format!(
                        "transform GitHub Releases page {} through source {}",
                        go_quote(&canonical),
                        go_quote(source.id())
                    ),
                    e,
                )
            })?,
            None => canonical.clone(),
        };
        let mut headers = Headers::new();
        headers.insert("User-Agent".into(), vec!["pixiv-cli".into()]);
        if let Some(page) = cached
            && !page.etag.is_empty()
        {
            headers.insert("If-None-Match".into(), vec![page.etag.clone()]);
        }
        let response = self
            .transport
            .send(HttpRequest {
                method: "GET".into(),
                url: route,
                logical_host: None,
                headers,
                body: None,
                content_length: 0,
                context: context.clone(),
            })
            .await
            .map_err(|e| {
                wrap(
                    format!("request GitHub Releases page {}", go_quote(&canonical)),
                    e,
                )
            })?;
        let mut response = response.ok_or_else(|| {
            wrap(
                format!("request GitHub Releases page {}", go_quote(&canonical)),
                message("HTTP transport returned no response"),
            )
        })?;
        let result = self.decode_page(context, url, cached, &mut response).await;
        if let Some(body) = &mut response.body {
            let _ = body.close().await;
        }
        result
    }
    async fn decode_page(
        &self,
        context: CallerContext,
        url: &PageUrl,
        cached: Option<&StoredPage>,
        response: &mut RawResponse,
    ) -> Result<(StoredPage, Option<PageUrl>), ExternalError> {
        let text = url.to_string();
        if response.status == 304 {
            let page = cached.ok_or_else(|| {
                message(format!(
                    "GitHub Releases page {} returned HTTP 304 without a cached response",
                    go_quote(&text)
                ))
            })?;
            let next = if page.next_url.is_empty() {
                None
            } else {
                Some(self.parse_page(&page.next_url).map_err(|e| {
                    wrap(
                        format!(
                            "read cached next GitHub Releases page after {}",
                            go_quote(&text)
                        ),
                        e,
                    )
                })?)
            };
            return Ok((page.clone(), next));
        }
        if response.status != 200 {
            return Err(message(format!(
                "GitHub Releases page {} returned HTTP {} {}",
                go_quote(&text),
                response.status,
                status_text(response.status)
            )));
        }
        let releases = decode_body(response.body.as_deref_mut())
            .await
            .map_err(|e| {
                wrap(
                    format!("decode GitHub Releases page {}", go_quote(&text)),
                    e,
                )
            })?;
        check_context(&context, "decode GitHub Releases page")?;
        let next = self
            .next_page(header_values(&response.headers, "Link"), url)
            .map_err(|e| {
                wrap(
                    format!("parse next GitHub Releases page after {}", go_quote(&text)),
                    e,
                )
            })?;
        Ok((
            StoredPage {
                url: text,
                etag: header_values(&response.headers, "ETag")
                    .first()
                    .cloned()
                    .unwrap_or_default(),
                next_url: next.as_ref().map(ToString::to_string).unwrap_or_default(),
                releases: Some(
                    releases
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|r| !r.draft)
                        .collect(),
                ),
            },
            next,
        ))
    }
    fn parse_page(&self, value: &str) -> Result<PageUrl, ExternalError> {
        self.validate_page(PageUrl::parse(value).map_err(|e| wrap("parse URL", e))?)
    }
    fn validate_page(&self, url: PageUrl) -> Result<PageUrl, ExternalError> {
        let text = url.to_string();
        if url.scheme.is_empty() {
            return Err(message(format!("URL {} is not absolute", go_quote(&text))));
        }
        if !url.fragment.is_empty() {
            return Err(message(format!("URL {} has a fragment", go_quote(&text))));
        }
        if url.scheme != self.endpoint.scheme
            || url.host != self.endpoint.host
            || url.user != self.endpoint.user
        {
            return Err(message(format!(
                "URL {} is not on the GitHub API origin {}",
                go_quote(&text),
                go_quote(&self.endpoint.to_string())
            )));
        }
        let path = clean_path(&url.path());
        let endpoint = clean_path(&self.endpoint.path());
        if path != endpoint {
            return Err(message(format!(
                "URL {} path {} is not the GitHub Releases endpoint path {}",
                go_quote(&text),
                go_quote(&path),
                go_quote(&endpoint)
            )));
        }
        Ok(url)
    }
    fn next_page(
        &self,
        headers: &[String],
        current: &PageUrl,
    ) -> Result<Option<PageUrl>, ExternalError> {
        let mut next = None;
        for header in headers {
            for link in header.split(',') {
                let link = link.trim();
                if link.is_empty() {
                    return Err(message("empty Link value"));
                }
                let parts: Vec<_> = link.split(';').collect();
                let target = parts[0];
                if target.len() < 3 || !target.starts_with('<') || !target.ends_with('>') {
                    return Err(message(format!("invalid Link value {}", go_quote(link))));
                }
                let mut is_next = false;
                for parameter in &parts[1..] {
                    let (name, value) = parameter.trim().split_once('=').ok_or_else(|| {
                        message(format!("invalid Link parameter {}", go_quote(parameter)))
                    })?;
                    if name.trim().eq_ignore_ascii_case("rel")
                        && value
                            .trim()
                            .trim_matches('"')
                            .split_whitespace()
                            .any(|v| v == "next")
                    {
                        is_next = true;
                    }
                }
                if !is_next {
                    continue;
                }
                if next.is_some() {
                    return Err(message("more than one next Link"));
                }
                let reference = PageUrl::parse(&target[1..target.len() - 1])
                    .map_err(|e| wrap("parse Link URL", e))?;
                next = Some(self.validate_page(current.resolve(reference))?);
            }
        }
        Ok(next)
    }
}
impl ReleaseChecker for GitHubReleaseClient {
    fn check(
        &self,
        context: CallerContext,
        options: ReleaseCheckOptions,
    ) -> UpdateFuture<'_, Result<ReleaseCheckResult, ExternalError>> {
        Box::pin(GitHubReleaseClient::check(self, context, options))
    }
}
pub fn validate_official_github_release_asset_url(
    release: &Release,
    asset: &ReleaseAsset,
) -> Result<(), ExternalError> {
    let expected = format!(
        "https://github.com/{DEFAULT_GITHUB_REPOSITORY}/releases/download/{}/{}",
        release.tag_name, asset.name
    );
    if asset.download_url != expected {
        return Err(message(format!(
            "release asset {} has untrusted download URL {}: expected exact GitHub HTTPS release URL {}",
            go_quote(&asset.name),
            go_quote(&asset.download_url),
            go_quote(&expected)
        )));
    }
    Ok(())
}
fn header_values<'a>(headers: &'a Headers, name: &str) -> &'a [String] {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_slice())
        .unwrap_or_default()
}
fn status_text(status: u16) -> &'static str {
    reqwest::StatusCode::from_u16(status)
        .ok()
        .and_then(|s| s.canonical_reason())
        .unwrap_or("")
}
fn select_release(
    releases: &[GitHubRelease],
    include: bool,
) -> Result<Option<Release>, ExternalError> {
    let mut selected = None;
    let mut selected_version: Option<SemanticVersion> = None;
    for candidate in releases {
        if candidate.draft || (candidate.prerelease && !include) {
            continue;
        }
        let version = SemanticVersion::parse(&candidate.tag_name).map_err(|e| {
            wrap(
                format!("parse GitHub release tag {}", go_quote(&candidate.tag_name)),
                e,
            )
        })?;
        let prerelease = candidate.prerelease || version.is_prerelease();
        if prerelease && !include {
            continue;
        }
        if selected_version
            .as_ref()
            .is_none_or(|current| version.compare(current) > 0)
        {
            selected = Some(Release {
                tag_name: candidate.tag_name.clone(),
                version: version.to_string(),
                prerelease,
                assets: candidate.assets.clone().unwrap_or_default(),
            });
            selected_version = Some(version);
        }
    }
    Ok(selected)
}

#[derive(Debug)]
struct DeadlineContext {
    parent: CallerContext,
    deadline: Instant,
}
impl DeadlineContext {
    fn new(parent: CallerContext) -> Self {
        let deadline = Instant::now() + Duration::from_secs(3);
        let deadline = parent.deadline().map_or(deadline, |d| d.min(deadline));
        Self { parent, deadline }
    }
}
impl RequestContext for DeadlineContext {
    fn error(&self) -> Option<ContextError> {
        self.parent
            .error()
            .or_else(|| (Instant::now() >= self.deadline).then_some(ContextError::DeadlineExceeded))
    }
    fn deadline(&self) -> Option<Instant> {
        Some(self.deadline)
    }
    fn cancelled(&self) -> ContextFuture<'_> {
        Box::pin(async move {
            if let Some(e) = self.error() {
                return e;
            }
            tokio::select! {biased;e=self.parent.cancelled()=>e,_=tokio::time::sleep_until(tokio::time::Instant::from_std(self.deadline))=>ContextError::DeadlineExceeded}
        })
    }
    fn value(&self, key: &ContextKey) -> Option<ContextValue> {
        self.parent.value(key)
    }
    fn extension(&self, key: TypeId) -> Option<ContextValue> {
        self.parent.extension(key)
    }
    fn scope(&self) -> Option<&Scope> {
        self.parent.scope()
    }
}

#[derive(Clone, Default, Serialize)]
struct GitHubRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Option<Vec<ReleaseAsset>>,
}
#[derive(Clone, Default, Serialize)]
struct StoredPage {
    url: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    etag: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    next_url: String,
    releases: Option<Vec<GitHubRelease>>,
}
#[derive(Default)]
struct StoredCache {
    schema_version: i64,
    checked_at: Option<DateTime<FixedOffset>>,
    releases: Option<Vec<GitHubRelease>>,
    pages: Vec<StoredPage>,
}
fn encode_cache(cache: &StoredCache) -> Result<Vec<u8>, ExternalError> {
    let checked = cache.checked_at.expect("refreshed cache has timestamp");
    if !(0..=9999).contains(&checked.year()) {
        return Err(wrap(
            "json: error calling MarshalJSON for type *time.Time",
            message("year outside of range [0,9999]"),
        ));
    }
    #[derive(Serialize)]
    struct Wire<'a> {
        schema_version: i64,
        checked_at: String,
        releases: &'a Option<Vec<GitHubRelease>>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        pages: &'a Vec<StoredPage>,
    }
    let wire = Wire {
        schema_version: cache.schema_version,
        checked_at: format_cache_time(checked),
        releases: &cache.releases,
        pages: &cache.pages,
    };
    let text = serde_json::to_string(&wire).map_err(|e| Box::new(e) as ExternalError)?;
    let mut bytes = crate::auth_bundle::escape_json_html(text).into_bytes();
    bytes.push(b'\n');
    Ok(bytes)
}

fn format_cache_time(time: DateTime<FixedOffset>) -> String {
    let mut text = time.format("%Y-%m-%dT%H:%M:%S").to_string();
    let nanos = time.timestamp_subsec_nanos();
    if nanos != 0 {
        text.push('.');
        text.push_str(format!("{nanos:09}").trim_end_matches('0'));
    }
    let offset = time.offset().local_minus_utc();
    if offset == 0 {
        text.push('Z');
    } else {
        let minutes = offset / 60;
        text.push(if minutes < 0 { '-' } else { '+' });
        let minutes = minutes.unsigned_abs();
        text.push_str(&format!("{:02}:{:02}", minutes / 60, minutes % 60));
    }
    text
}

enum Node {
    Null,
    String(String),
    Number(String),
    Bool(bool),
    Array(Vec<Node>),
    Object(Vec<(String, Node)>),
}
impl<'de> Deserialize<'de> for Node {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Nodes;
        impl<'de> Visitor<'de> for Nodes {
            type Value = Node;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("JSON")
            }
            fn visit_unit<E: de::Error>(self) -> Result<Node, E> {
                Ok(Node::Null)
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Node, E> {
                Ok(Node::Bool(v))
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<Node, E> {
                Ok(Node::String(v))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Node, E> {
                Ok(Node::String(v.into()))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Node, E> {
                Ok(Node::Number(v.to_string()))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Node, E> {
                Ok(Node::Number(v.to_string()))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Node, E> {
                Ok(Node::Number(v.to_string()))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Node, A::Error> {
                let mut values = Vec::new();
                while let Some(v) = a.next_element()? {
                    values.push(v);
                }
                Ok(Node::Array(values))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<Node, A::Error> {
                let mut fields = Vec::new();
                while let Some(k) = a.next_key()? {
                    fields.push((k, a.next_value()?));
                }
                Ok(Node::Object(fields))
            }
        }
        let raw = <&serde_json::value::RawValue>::deserialize(d)?;
        if matches!(raw.get().as_bytes().first(), Some(b'-' | b'0'..=b'9')) {
            return Ok(Node::Number(raw.get().into()));
        }
        serde_json::Deserializer::from_str(raw.get())
            .deserialize_any(Nodes)
            .map_err(de::Error::custom)
    }
}
fn kind(n: &Node) -> &str {
    match n {
        Node::Null => "null",
        Node::String(_) => "string",
        Node::Number(_) => "number",
        Node::Bool(_) => "bool",
        Node::Array(_) => "array",
        Node::Object(_) => "object",
    }
}
fn invalid_type(n: &Node, path: &str, ty: &str) -> ExternalError {
    message(format!(
        "json: cannot unmarshal {} into Go struct field {path} of type {ty}",
        kind(n)
    ))
}
fn string_value(n: Node, path: &str, target: &mut String) -> Result<(), ExternalError> {
    match n {
        Node::Null => Ok(()),
        Node::String(v) => {
            *target = v;
            Ok(())
        }
        n => Err(invalid_type(&n, path, "string")),
    }
}
fn bool_value(n: Node, path: &str, target: &mut bool) -> Result<(), ExternalError> {
    match n {
        Node::Null => Ok(()),
        Node::Bool(v) => {
            *target = v;
            Ok(())
        }
        n => Err(invalid_type(&n, path, "bool")),
    }
}
fn asset(n: Node, path: &str) -> Result<ReleaseAsset, ExternalError> {
    let mut asset = ReleaseAsset::default();
    match n {
        Node::Null => {}
        Node::Object(fields) => {
            for (k, n) in fields {
                if crate::auth_bundle::folded(&k, "name") {
                    string_value(n, &format!("{path}.name"), &mut asset.name)?;
                } else if crate::auth_bundle::folded(&k, "browser_download_url") {
                    string_value(
                        n,
                        &format!("{path}.browser_download_url"),
                        &mut asset.download_url,
                    )?;
                }
            }
        }
        n => return Err(invalid_type(&n, path, "release.ReleaseAsset")),
    };
    Ok(asset)
}
fn releases(n: Node, path: &str) -> Result<Option<Vec<GitHubRelease>>, ExternalError> {
    let list = match n {
        Node::Null => return Ok(None),
        Node::Array(v) => v,
        n => {
            return Err(message(format!(
                "json: cannot unmarshal {} into Go value of type []release.githubRelease",
                kind(&n)
            )));
        }
    };
    let mut result = Vec::new();
    for (index, n) in list.into_iter().enumerate() {
        let mut r = GitHubRelease::default();
        let path = format!("{path}.{index}");
        match n {
            Node::Null => {}
            Node::Object(fields) => {
                for (k, n) in fields {
                    if crate::auth_bundle::folded(&k, "tag_name") {
                        string_value(n, &format!("{path}.tag_name"), &mut r.tag_name)?;
                    } else if crate::auth_bundle::folded(&k, "draft") {
                        bool_value(n, &format!("{path}.draft"), &mut r.draft)?;
                    } else if crate::auth_bundle::folded(&k, "prerelease") {
                        bool_value(n, &format!("{path}.prerelease"), &mut r.prerelease)?;
                    } else if crate::auth_bundle::folded(&k, "assets") {
                        r.assets = match n {
                            Node::Null => None,
                            Node::Array(v) => Some(
                                v.into_iter()
                                    .enumerate()
                                    .map(|(i, n)| asset(n, &format!("{path}.assets.{i}")))
                                    .collect::<Result<_, _>>()?,
                            ),
                            n => {
                                return Err(invalid_type(
                                    &n,
                                    &format!("{path}.assets"),
                                    "[]release.ReleaseAsset",
                                ));
                            }
                        };
                    }
                }
            }
            n => return Err(invalid_type(&n, &path, "release.githubRelease")),
        };
        result.push(r);
    }
    Ok(Some(result))
}
fn decode_cache(bytes: &[u8]) -> Result<StoredCache, ExternalError> {
    let mut d = serde_json::Deserializer::from_slice(bytes);
    let n = Node::deserialize(&mut d).map_err(|e| json_error(bytes, &e, true))?;
    d.end().map_err(|e| json_error(bytes, &e, true))?;
    let mut cache = StoredCache::default();
    let fields = match n {
        Node::Null => return Ok(cache),
        Node::Object(v) => v,
        n => return Err(invalid_type(&n, "", "release.releaseCache")),
    };
    for (k, n) in fields {
        if crate::auth_bundle::folded(&k, "schema_version") {
            match n {
                Node::Null => {}
                Node::Number(v) => {
                    cache.schema_version = v.parse().map_err(|_| {
                        invalid_type(&Node::Number(v), "releaseCache.schema_version", "int")
                    })?
                }
                n => return Err(invalid_type(&n, "releaseCache.schema_version", "int")),
            }
        } else if crate::auth_bundle::folded(&k, "checked_at") {
            match n {
                Node::Null => {}
                Node::String(v) => cache.checked_at = Some(parse_time(&v)?),
                n => return Err(invalid_type(&n, "releaseCache.checked_at", "time.Time")),
            }
        } else if crate::auth_bundle::folded(&k, "releases") {
            cache.releases = releases(n, "releaseCache.releases")?;
        } else if crate::auth_bundle::folded(&k, "pages") {
            cache.pages = match n {
                Node::Null => Vec::new(),
                Node::Array(v) => v.into_iter().map(decode_page).collect::<Result<_, _>>()?,
                n => {
                    return Err(invalid_type(
                        &n,
                        "releaseCache.pages",
                        "[]release.releaseCachePage",
                    ));
                }
            };
        }
    }
    Ok(cache)
}
fn decode_page(n: Node) -> Result<StoredPage, ExternalError> {
    let mut page = StoredPage::default();
    match n {
        Node::Null => {}
        Node::Object(v) => {
            for (k, n) in v {
                if crate::auth_bundle::folded(&k, "url") {
                    string_value(n, "releaseCache.pages.url", &mut page.url)?;
                } else if crate::auth_bundle::folded(&k, "etag") {
                    string_value(n, "releaseCache.pages.etag", &mut page.etag)?;
                } else if crate::auth_bundle::folded(&k, "next_url") {
                    string_value(n, "releaseCache.pages.next_url", &mut page.next_url)?;
                } else if crate::auth_bundle::folded(&k, "releases") {
                    page.releases = releases(n, "releaseCache.pages.releases")?;
                }
            }
        }
        n => {
            return Err(invalid_type(
                &n,
                "releaseCache.pages",
                "release.releaseCachePage",
            ));
        }
    };
    Ok(page)
}
fn parse_time(value: &str) -> Result<DateTime<FixedOffset>, ExternalError> {
    DateTime::parse_from_rfc3339(value).map_err(|_| {
        message(format!(
            "parsing time {} as {}: cannot parse {} as {}",
            go_quote(value),
            go_quote("2006-01-02T15:04:05Z07:00"),
            go_quote(value),
            go_quote("2006"),
        ))
    })
}
async fn decode_body(
    body: Option<&mut (dyn RawBody + 'static)>,
) -> Result<Option<Vec<GitHubRelease>>, ExternalError> {
    let Some(body) = body else {
        return Err(message("EOF"));
    };
    let mut bytes = Vec::new();
    let mut capacity = 64;
    loop {
        let mut d = serde_json::Deserializer::from_slice(&bytes);
        match Node::deserialize(&mut d) {
            Ok(n) => return releases(n, ""),
            Err(e) if !e.is_eof() => return Err(json_error(&bytes, &e, false)),
            Err(_) => {}
        }
        if bytes.len() >= capacity * 3 / 4 {
            capacity *= 2;
        }
        let mut buffer = vec![0; capacity - bytes.len()];
        let read = body.read(&mut buffer).await;
        bytes.extend_from_slice(&buffer[..read.count]);
        if read.count > 0 {
            continue;
        }
        if let Some(error) = read.error {
            return Err(error);
        }
        if read.eof {
            return Err(message(if bytes.iter().any(|b| !b.is_ascii_whitespace()) {
                "unexpected EOF"
            } else {
                "EOF"
            }));
        }
    }
}
fn json_error(bytes: &[u8], error: &serde_json::Error, cache: bool) -> ExternalError {
    if error.is_eof() {
        return message(if cache {
            "unexpected end of JSON input"
        } else {
            "unexpected EOF"
        });
    }
    let offset = bytes
        .iter()
        .enumerate()
        .scan((1usize, 0usize), |(line, column), (i, b)| {
            if *b == b'\n' {
                *line += 1;
                *column = 0;
            } else {
                *column += 1;
            }
            Some((i, *line, *column))
        })
        .find(|(_, l, c)| *l == error.line() && *c == error.column())
        .map(|(i, _, _)| i)
        .unwrap_or(0);
    let byte = bytes.get(offset).copied().unwrap_or(b'?');
    if error.to_string().starts_with("trailing characters") {
        return message(format!(
            "invalid character {} after top-level value",
            go_char(byte)
        ));
    }
    if error.to_string().starts_with("expected ident") {
        let start = bytes
            .iter()
            .position(|b| !b.is_ascii_whitespace())
            .unwrap_or(0);
        let literal = match bytes.get(start) {
            Some(b'n') => "null",
            Some(b't') => "true",
            Some(b'f') => "false",
            _ => "",
        };
        let index = offset.saturating_sub(start);
        let expected = literal.as_bytes().get(index).copied().unwrap_or(b'?');
        return message(format!(
            "invalid character {} in literal {literal} (expecting {})",
            go_char(byte),
            go_char(expected)
        ));
    }
    message(format!(
        "invalid character {} looking for beginning of value",
        go_char(byte)
    ))
}
fn go_char(b: u8) -> String {
    match b {
        b'\'' => "'\\\''".into(),
        b'\\' => "'\\\\'".into(),
        b'\n' => "'\\n'".into(),
        b'\r' => "'\\r'".into(),
        b'\t' => "'\\t'".into(),
        _ => format!("'{}'", char::from(b)),
    }
}

#[derive(Clone)]
struct PageUrl {
    scheme: String,
    user: String,
    host: String,
    raw_path: String,
    query: Option<String>,
    fragment: String,
}
impl PageUrl {
    fn parse(value: &str) -> Result<Self, ExternalError> {
        let (before_fragment, fragment) =
            value.split_once('#').map_or((value, ""), |(a, b)| (a, b));
        let (before_query, query) = before_fragment
            .split_once('?')
            .map_or((before_fragment, None), |(a, b)| (a, Some(b.to_owned())));
        let (scheme, rest) = if let Some((a, b)) = before_query.split_once(':') {
            if a.is_empty() {
                return Err(message("missing protocol scheme"));
            }
            if a.bytes().enumerate().all(|(i, c)| {
                c.is_ascii_alphabetic()
                    || (i > 0 && (c.is_ascii_digit() || matches!(c, b'+' | b'-' | b'.')))
            }) {
                (a.to_ascii_lowercase(), b)
            } else {
                ("".into(), before_query)
            }
        } else {
            ("".into(), before_query)
        };
        let (authority, path) = if let Some(rest) = rest.strip_prefix("//") {
            rest.split_once('/').map_or((rest, ""), |(a, b)| {
                (a, &rest[a.len()..a.len() + 1 + b.len()])
            })
        } else {
            ("", rest)
        };
        let (user, host) = authority
            .rsplit_once('@')
            .map_or(("", authority), |(a, b)| (a, b));
        validate_escapes(host)?;
        validate_escapes(path)?;
        validate_escapes(fragment)?;
        Ok(Self {
            scheme,
            user: user.into(),
            host: host.into(),
            raw_path: path.into(),
            query,
            fragment: fragment.into(),
        })
    }
    fn path(&self) -> String {
        unescape(&self.raw_path)
    }
    fn canonical(&self) -> String {
        let mut url = self.clone();
        url.raw_path = escape_path(&clean_path(&url.path()));
        url.fragment.clear();
        url.to_string()
    }
    fn resolve(&self, mut reference: Self) -> Self {
        if !reference.scheme.is_empty() {
            reference.raw_path = resolve_path("", &reference.raw_path);
            return reference;
        }
        reference.scheme = self.scheme.clone();
        if !reference.host.is_empty() {
            reference.raw_path = resolve_path("", &reference.raw_path);
            return reference;
        }
        reference.host = self.host.clone();
        reference.user = self.user.clone();
        if reference.raw_path.is_empty() {
            reference.raw_path = self.raw_path.clone();
            if reference.query.is_none() {
                reference.query = self.query.clone();
            }
        } else {
            reference.raw_path = resolve_path(&self.raw_path, &reference.raw_path);
        }
        reference
    }
}
impl fmt::Display for PageUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.scheme.is_empty() {
            write!(f, "{}:", self.scheme)?;
        }
        if !self.host.is_empty() {
            f.write_str("//")?;
            if !self.user.is_empty() {
                write!(f, "{}@", self.user)?;
            }
            f.write_str(&self.host)?;
        }
        f.write_str(&self.raw_path)?;
        if let Some(query) = &self.query {
            write!(f, "?{query}")?;
        }
        if !self.fragment.is_empty() {
            write!(f, "#{}", self.fragment)?;
        }
        Ok(())
    }
}
pub(super) fn validate_escapes(value: &str) -> Result<(), ExternalError> {
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len()
                || !bytes[i + 1].is_ascii_hexdigit()
                || !bytes[i + 2].is_ascii_hexdigit()
            {
                let end = (i + 3).min(bytes.len());
                return Err(message(format!(
                    "invalid URL escape {}",
                    go_quote_bytes(&bytes[i..end])
                )));
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    Ok(())
}
fn go_quote_bytes(mut bytes: &[u8]) -> String {
    let mut output = String::from("\"");
    while !bytes.is_empty() {
        let (valid, invalid) = match std::str::from_utf8(bytes) {
            Ok(text) => (text.len(), 0),
            Err(error) => (
                error.valid_up_to(),
                error
                    .error_len()
                    .unwrap_or(bytes.len() - error.valid_up_to()),
            ),
        };
        if valid > 0 {
            let quoted =
                go_quote(std::str::from_utf8(&bytes[..valid]).expect("validated UTF-8 prefix"));
            output.push_str(&quoted[1..quoted.len() - 1]);
        }
        for byte in &bytes[valid..valid + invalid] {
            output.push_str(&format!("\\x{byte:02x}"));
        }
        bytes = &bytes[valid + invalid..];
    }
    output.push('"');
    output
}

fn unescape(value: &str) -> String {
    let mut result = Vec::new();
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let v = u8::from_str_radix(&value[i + 1..i + 3], 16).expect("validated percent escape");
            result.push(v);
            i += 3;
        } else {
            result.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&result).into_owned()
}
fn escape_path(value: &str) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        if b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'-' | b'_' | b'.' | b'~' | b'/' | b':' | b'@' | b'&' | b'=' | b'+' | b'$' | b','
            )
        {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
fn clean_path(value: &str) -> String {
    let mut parts = Vec::new();
    for part in value.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    format!("/{}", parts.join("/"))
}
fn resolve_path(base: &str, reference: &str) -> String {
    let path = if reference.starts_with('/') {
        reference.to_owned()
    } else {
        format!(
            "{}{reference}",
            base.rsplit_once('/')
                .map_or("", |(a, _)| &base[..a.len() + 1])
        )
    };
    let trailing = path.ends_with('/') || path.ends_with("/.") || path.ends_with("/..");
    let mut path = clean_path(&path);
    if trailing && path != "/" {
        path.push('/');
    }
    path
}
