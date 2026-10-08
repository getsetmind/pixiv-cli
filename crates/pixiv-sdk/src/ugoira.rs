use crate::{
    Error, Reason, Result,
    models::{UgoiraArchive, UgoiraFrame, UgoiraMetadata},
    pixiv::ResourcePolicy,
    resource::Resource,
};
use serde_json::Value;

pub(crate) fn map(
    id: i64,
    body: &Value,
    policy: &ResourcePolicy,
    remember: impl Fn(&Resource),
) -> Result<UgoiraMetadata> {
    let (urls, wire_frames) = crate::resource_resolution::validated_ugoira(body, "UgoiraMetadata")?;
    let mut archives = Vec::new();
    for quality in ["medium", "original"] {
        if let Some(url) = urls
            .get(quality)
            .and_then(Value::as_str)
            .filter(|url| !url.is_empty())
        {
            let resource = policy.resource("ugoira_archive", id, -1, quality, url)?;
            remember(&resource);
            archives.push(UgoiraArchive {
                quality: quality.into(),
                resource,
            });
        }
    }
    let mut frames = Vec::with_capacity(wire_frames.len());
    for frame in wire_frames {
        let file = frame["file"].as_str().unwrap_or_default();
        if file.contains('\\') || file.starts_with("..") {
            return Err(
                Error::new(Reason::MalformedUpstreamResponse, "UgoiraMetadata")
                    .with_detail("ugoira frame filename is unsafe or duplicated"),
            );
        }
        frames.push(UgoiraFrame {
            filename: file.into(),
            delay_milliseconds: frame
                .get("delay")
                .and_then(Value::as_i64)
                .unwrap_or_default(),
        });
    }
    Ok(UgoiraMetadata {
        artwork_id: id,
        archives,
        frames,
    })
}
