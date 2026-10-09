use crate::{Client, Error, Reason, Result, models::Stamp, transport::Transport};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StampsRequest {}

pub(crate) fn validated_stamps<'a>(
    body: &'a Value,
    operation: &'static str,
) -> Result<Vec<(i64, &'a str)>> {
    let malformed = || Error::new(Reason::MalformedUpstreamResponse, operation);
    let values = body
        .get("stamps")
        .and_then(Value::as_array)
        .ok_or_else(malformed)?;
    if body.get("next_url").is_some_and(|value| !value.is_null()) {
        return Err(malformed());
    }
    let mut items = Vec::with_capacity(values.len());
    for value in values {
        let id = value
            .get("stamp_id")
            .and_then(Value::as_i64)
            .filter(|id| *id > 0)
            .ok_or_else(malformed)?;
        let raw = value
            .get("stamp_url")
            .and_then(Value::as_str)
            .ok_or_else(malformed)?;
        crate::artwork::ResourcePolicy::default()
            .validate(raw)
            .map_err(|_| malformed())?;
        items.push((id, raw));
    }
    Ok(items)
}
impl<T: Transport> Client<T> {
    pub async fn stamps(&self, _request: StampsRequest) -> Result<Vec<Stamp>> {
        let operation = "Stamps";
        let raw = self.get_json("/v1/stamps", vec![], operation).await?;
        let body = crate::user_wire::decode_stamps(&raw, operation)?;
        let values = validated_stamps(&body, operation)?;
        let mut items = Vec::with_capacity(values.len());
        for (id, url) in values {
            let image = self
                .resource_policy
                .image("stamp", id, -1, "", url, (0, 0))
                .map_err(|_| Error::new(Reason::MalformedUpstreamResponse, operation))?;
            self.remember_resource(&image.resource);
            items.push(Stamp { id, image });
        }
        Ok(items)
    }
}
