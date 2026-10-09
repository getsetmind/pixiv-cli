use crate::{
    Client, Error, Reason, Result,
    artwork::WireUser,
    cursor::{Cursor, Page},
    models::UserPreview,
    ranking::{apply_offset, next_cursor, ranking_error},
    transport::Transport,
};
use serde::Deserialize;
use std::collections::BTreeMap;

const OPERATION: &str = "SearchUsers";

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SearchUsersRequest {
    pub word: String,
    pub cursor: Cursor,
}

#[derive(Default, Deserialize)]
struct WirePreview {
    user: Option<WireUser>,
}
#[derive(Deserialize)]
struct Envelope {
    user_previews: Option<Vec<Option<WirePreview>>>,
    next_url: Option<String>,
}

impl<T: Transport> Client<T> {
    pub async fn search_users(&self, request: SearchUsersRequest) -> Result<Page<UserPreview>> {
        if request.word.trim().is_empty() {
            return Err(ranking_error(
                OPERATION,
                Reason::InvalidArgument,
                "search word is required",
            ));
        }
        let mut query = BTreeMap::from([("word".into(), request.word)]);
        let digest = crate::continuation::query_digest(&query);
        apply_offset(&request.cursor, OPERATION, &digest, &mut query)?;
        let body = self
            .get("/v1/search/user", query.into_iter().collect(), OPERATION)
            .await?;
        let malformed = || Error::new(Reason::MalformedUpstreamResponse, OPERATION);
        let envelope: Envelope = serde_json::from_value(body).map_err(|_| malformed())?;
        let previews = envelope.user_previews.ok_or_else(malformed)?;
        let mut users = Vec::with_capacity(previews.len());
        for preview in previews {
            let user = preview
                .and_then(|preview| preview.user)
                .ok_or_else(malformed)?;
            if user.id.is_none_or(|id| id <= 0) {
                return Err(malformed());
            }
            users.push(user);
        }
        let offset = envelope
            .next_url
            .as_deref()
            .map(|raw| {
                crate::continuation::next_offset(raw, "v1/search/user", &["word", "offset"])
                    .ok_or_else(malformed)
            })
            .transpose()?;
        let items = users
            .into_iter()
            .map(|user| {
                let user = user.map(&self.resource_policy);
                self.remember_resource(&user.profile_image.resource);
                UserPreview {
                    user,
                    ..Default::default()
                }
            })
            .collect();
        Ok(Page {
            items,
            next: next_cursor(OPERATION, &digest, offset)?,
        })
    }
}
