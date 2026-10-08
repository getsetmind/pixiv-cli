use crate::{Error, Reason, Result};
use url::Url;

pub fn artwork_id(input: &str) -> Result<i64> {
    if let Ok(id) = input.parse::<i64>() {
        return positive(id);
    }
    let url = Url::parse(input).map_err(|_| invalid())?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("www.pixiv.net" | "pixiv.net"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return Err(invalid());
    }
    let segments: Vec<_> = url.path_segments().ok_or_else(invalid)?.collect();
    let id = match segments.as_slice() {
        ["artworks", id] | ["en", "artworks", id] => id.parse::<i64>().map_err(|_| invalid())?,
        ["member_illust.php"] => url
            .query_pairs()
            .find(|(key, _)| key == "illust_id")
            .ok_or_else(invalid)?
            .1
            .parse::<i64>()
            .map_err(|_| invalid())?,
        _ => return Err(invalid()),
    };
    positive(id)
}

fn positive(id: i64) -> Result<i64> {
    if id > 0 { Ok(id) } else { Err(invalid()) }
}

fn invalid() -> Error {
    Error::new(Reason::InvalidArgument, "artwork_reference")
}
