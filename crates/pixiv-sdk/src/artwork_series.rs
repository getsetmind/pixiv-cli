use crate::{
    Client, Error, Reason, Result,
    artwork::WireUser,
    cursor::{Cursor, Page},
    models::Artwork,
    transport::Transport,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

const OPERATION: &str = "ArtworkSeries";
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ArtworkSeriesRequest {
    pub series_id: i64,
    pub cursor: Cursor,
}
#[derive(Deserialize)]
struct Envelope {
    illust_series_detail: Option<Detail>,
    illusts: Option<Vec<Value>>,
    next_url: Option<String>,
}
#[derive(Deserialize)]
struct Detail {
    user: Option<WireUser>,
}
impl<T: Transport> Client<T> {
    pub async fn artwork_series(&self, request: ArtworkSeriesRequest) -> Result<Page<Artwork>> {
        if request.series_id <= 0 {
            return Err(Error::new(Reason::InvalidArgument, OPERATION)
                .with_detail("series ID must be positive"));
        }
        let mut query =
            BTreeMap::from([("illust_series_id".into(), request.series_id.to_string())]);
        let digest = crate::continuation::query_digest(&query);
        crate::ranking::apply_keyed_value(
            &request.cursor,
            OPERATION,
            &digest,
            &mut query,
            &["offset", "last_order"],
        )?;
        let body = self
            .get("/v1/illust/series", query.into_iter().collect(), OPERATION)
            .await?;
        let malformed = || Error::new(Reason::MalformedUpstreamResponse, OPERATION);
        let envelope: Envelope = serde_json::from_value(body).map_err(|_| malformed())?;
        if envelope
            .illust_series_detail
            .and_then(|detail| detail.user)
            .and_then(|user| user.id)
            .unwrap_or_default()
            <= 0
        {
            return Err(malformed());
        }
        let values = envelope.illusts.ok_or_else(malformed)?;
        crate::artwork::validate_list(&values, OPERATION)?;
        for value in &values {
            if value
                .get("user")
                .and_then(|user| user.get("id"))
                .and_then(Value::as_i64)
                .unwrap_or_default()
                <= 0
            {
                return Err(malformed());
            }
        }
        let next = envelope
            .next_url
            .as_deref()
            .map(|raw| {
                crate::continuation::next_keyed_value(
                    raw,
                    "v1/illust/series",
                    &["illust_series_id", "offset", "last_order"],
                    &["offset", "last_order"],
                )
                .ok_or_else(malformed)
            })
            .transpose()?;
        let mut items = Vec::with_capacity(values.len());
        for value in values {
            let artwork = crate::artwork::map(value, "Artwork", false, &self.resource_policy)?;
            self.remember_artwork(&artwork);
            items.push(artwork);
        }
        let cursor = match next {
            Some((key, value)) => {
                crate::ranking::value_cursor(OPERATION, &digest, &key, Some(value))?
            }
            None => Cursor::default(),
        };
        Ok(Page {
            items,
            next: cursor,
        })
    }
}
