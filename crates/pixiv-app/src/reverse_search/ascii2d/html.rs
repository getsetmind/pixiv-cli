use crate::reverse_search::Match;
use scraper::{ElementRef, Html, Selector};

fn selector(value: &str) -> Selector {
    Selector::parse(value).expect("valid ASCII2D selector")
}
fn text(node: ElementRef<'_>) -> String {
    node.text()
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
pub(super) fn upload_form(bytes: &[u8]) -> Option<String> {
    let html = Html::parse_document(&String::from_utf8_lossy(bytes));
    for form in html.select(&selector("form")) {
        if form.value().attr("id") != Some("file_upload")
            || form.value().attr("action") != Some("/search/file")
            || !form
                .value()
                .attr("method")
                .unwrap_or("")
                .eq_ignore_ascii_case("post")
            || !form
                .value()
                .attr("enctype")
                .unwrap_or("")
                .eq_ignore_ascii_case("multipart/form-data")
        {
            continue;
        }
        let mut token = String::new();
        let mut file = false;
        for input in form.select(&selector("input")) {
            let kind = input.value().attr("type").unwrap_or("");
            match input.value().attr("name") {
                Some("authenticity_token") if kind.eq_ignore_ascii_case("hidden") => {
                    token = input.value().attr("value").unwrap_or("").trim().to_owned()
                }
                Some("file") if kind.eq_ignore_ascii_case("file") => file = true,
                _ => {}
            }
        }
        if file && !token.is_empty() {
            return Some(token);
        }
    }
    None
}
pub(super) fn results(bytes: &[u8]) -> Option<Vec<Match>> {
    let html = Html::parse_document(&String::from_utf8_lossy(bytes));
    let items: Vec<_> = html.select(&selector(".item-box")).collect();
    if items.is_empty() {
        return None;
    }
    let mut matches = Vec::new();
    for item in &items[1..] {
        let Some(info) = item.select(&selector(".info-box")).next() else {
            if item.select(&selector(".image-box")).next().is_some() {
                return None;
            }
            continue;
        };
        let detail = info.select(&selector(".detail-box")).next()?;
        let external = detail.select(&selector(".external")).next();
        let root = external.unwrap_or(detail);
        let links: Vec<_> = root
            .select(&selector("a"))
            .filter(|node| !node.value().attr("href").unwrap_or("").trim().is_empty())
            .collect();
        if links.is_empty() {
            return None;
        }
        let (source, title) = if let Some(external) = external {
            let mut values = Vec::new();
            for node in external.descendants() {
                if let Some(value) = node.value().as_text()
                    && !node
                        .ancestors()
                        .take_while(|ancestor| ancestor.id() != external.id())
                        .any(|ancestor| {
                            ancestor
                                .value()
                                .as_element()
                                .is_some_and(|element| element.name() == "a")
                        })
                {
                    values.push(value.to_string());
                }
            }
            (
                text(links[0]),
                values
                    .join(" ")
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" "),
            )
        } else {
            (
                detail
                    .select(&selector("small"))
                    .next()
                    .map(text)
                    .unwrap_or_default(),
                text(links[0]),
            )
        };
        if source.is_empty() || (external.is_some() && title.is_empty()) {
            return None;
        }
        matches.push(Match {
            rank: matches.len() as i64 + 1,
            index_name: source,
            title,
            author: links.get(1).copied().map(text).unwrap_or_default(),
            external_urls: links
                .iter()
                .map(|node| node.value().attr("href").unwrap_or("").trim().to_owned())
                .collect(),
            ..Match::default()
        });
    }
    Some(matches)
}
pub(super) fn challenge(bytes: &[u8]) -> bool {
    let html = Html::parse_document(&String::from_utf8_lossy(bytes));
    for node in html.tree.nodes() {
        if let Some(element) = node.value().as_element() {
            if element.attrs().any(|(key, value)| {
                let value = format!("{key}={value}").to_lowercase();
                ["cf-chl", "cf_chl", "challenge-platform"]
                    .iter()
                    .any(|marker| value.contains(marker))
            }) {
                return true;
            }
            if matches!(element.name(), "title" | "h1" | "h2")
                && ElementRef::wrap(node).is_some_and(|node| challenge_text(&text(node)))
            {
                return true;
            }
        }
        if let Some(text) = node.value().as_text()
            && challenge_text(text)
        {
            return true;
        }
        if let Some(comment) = node.value().as_comment()
            && challenge_text(comment)
        {
            return true;
        }
    }
    false
}
fn challenge_text(value: &str) -> bool {
    let value = value
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    [
        "just a moment",
        "verify you are human",
        "checking your browser before accessing",
        "challenge-platform",
        "cf-chl-",
        "cf_chl",
    ]
    .iter()
    .any(|marker| value.contains(marker))
}
