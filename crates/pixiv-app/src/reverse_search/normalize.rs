use super::{Evidence, Match, PixivRef, PixivRefType, Provider, SearchResult};
use std::collections::HashMap;

pub(super) fn append_matches(
    results: &mut Vec<SearchResult>,
    canonical: &mut HashMap<PixivRef, usize>,
    provider: Provider,
    matches: &[Match],
    pixiv_only: bool,
) {
    let mut ordered: Vec<_> = matches.iter().collect();
    ordered.sort_by_key(|value| value.rank);
    for matched in ordered {
        let pixiv = pixiv_ref(matched);
        if pixiv_only && pixiv.is_none() {
            continue;
        }
        let evidence = Evidence {
            provider: provider.clone(),
            rank: matched.rank,
            similarity: matched.similarity,
            index_id: matched.index_id,
            index_name: matched.index_name.clone(),
            title: matched.title.clone(),
            author: matched.author.clone(),
            external_urls: matched.external_urls.clone(),
        };
        if let Some(key) = &pixiv {
            if let Some(index) = canonical.get(key) {
                results[*index]
                    .evidence
                    .as_mut()
                    .expect("canonical result evidence")
                    .push(evidence);
                continue;
            }
            canonical.insert(key.clone(), results.len());
        }
        results.push(SearchResult {
            pixiv,
            title: matched.title.clone(),
            author: matched.author.clone(),
            evidence: Some(vec![evidence]),
        });
    }
}
fn pixiv_ref(matched: &Match) -> Option<PixivRef> {
    if matched.artwork_id > 0 {
        return Some(PixivRef {
            kind: PixivRefType::Artwork,
            id: matched.artwork_id,
        });
    }
    let mut user = None;
    for external in &matched.external_urls {
        if let Some(candidate) = parse_pixiv_url(external) {
            if candidate.kind == PixivRefType::Artwork {
                return Some(candidate);
            }
            if user.is_none() {
                user = Some(candidate);
            }
        }
    }
    if matched.user_id > 0 {
        Some(PixivRef {
            kind: PixivRefType::User,
            id: matched.user_id,
        })
    } else {
        user
    }
}
fn parse_pixiv_url(raw: &str) -> Option<PixivRef> {
    let raw = raw.strip_prefix("https://")?;
    let (authority, mut path) = raw.split_once('/')?;
    if !authority.eq_ignore_ascii_case("www.pixiv.net") {
        return None;
    }
    if let Some((prefix, fragment)) = path.split_once('#') {
        if !fragment.is_empty() {
            return None;
        }
        path = prefix;
    }
    if let Some((prefix, query)) = path.split_once('?') {
        if !query.is_empty() {
            return None;
        }
        path = prefix;
    }
    let (kind, id) = path.split_once('/')?;
    if id.is_empty() || id.starts_with('0') || !id.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let id: i64 = id.parse().ok()?;
    if id <= 0 {
        return None;
    }
    let kind = match kind {
        "artworks" => PixivRefType::Artwork,
        "users" => PixivRefType::User,
        _ => return None,
    };
    Some(PixivRef { kind, id })
}
