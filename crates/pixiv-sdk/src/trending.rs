use crate::{Client, Error, Reason, Result, models::TrendingTag, transport::Transport};
use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Debug, Default)]
pub struct TrendingArtworkTagsRequest {}

#[derive(Deserialize)]
struct Entry {
    tag: Option<String>,
    translated_name: Option<String>,
    illust: Option<Value>,
}

impl<T: Transport> Client<T> {
    pub async fn trending_artwork_tags(
        &self,
        _: TrendingArtworkTagsRequest,
    ) -> Result<Vec<TrendingTag>> {
        let malformed = || Error::new(Reason::MalformedUpstreamResponse, "TrendingArtworkTags");
        let body = self
            .get("/v1/trending-tags/illust", vec![], "TrendingArtworkTags")
            .await?;
        let values = body
            .get("trend_tags")
            .filter(|value| value.is_array())
            .ok_or_else(malformed)?;
        let entries: Vec<Option<Entry>> =
            serde_json::from_value(values.clone()).map_err(|_| malformed())?;
        let mut tags = vec![];
        let mut artworks = vec![];
        for entry in entries {
            let entry = entry.ok_or_else(malformed)?;
            let tag = entry
                .tag
                .filter(|tag| !tag.is_empty())
                .ok_or_else(malformed)?;
            let artwork = entry
                .illust
                .filter(|value| value.is_object())
                .ok_or_else(malformed)?;
            tags.push((tag, entry.translated_name.unwrap_or_default()));
            artworks.push(artwork);
        }
        crate::artwork::validate_list(&artworks, "TrendingArtworkTags")?;
        tags.into_iter()
            .zip(artworks)
            .map(|((tag, translated_name), value)| {
                let artwork = crate::artwork::map(value, "Artwork", false, &self.resource_policy)?;
                self.remember_artwork(&artwork);
                Ok(TrendingTag {
                    tag,
                    translated_name,
                    artwork,
                })
            })
            .collect()
    }
}
