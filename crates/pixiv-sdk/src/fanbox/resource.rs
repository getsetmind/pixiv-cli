use super::{
    Client, Failure, options,
    safe_media_body::SafeMediaBody,
    transport::{BodyFuture, ExternalError, Headers, RawBody, RawRead},
};
use crate::{
    Error, Reason, Result,
    context::RequestContext,
    error::Cause,
    resource::{
        OpenResourceRequest, Resource, ResourceRef, ResourceResponse, SaveOptions, SaveProgress,
        SavedResource,
    },
    save::Destination,
};
use futures_util::FutureExt;
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{IgnoredAny, MapAccess, Visitor},
};
use std::{fmt, panic::AssertUnwindSafe, path::Path, sync::Arc};

#[derive(Default, Serialize)]
struct ResourceIdentity {
    #[serde(rename = "k")]
    kind: String,
    #[serde(rename = "c", skip_serializing_if = "String::is_empty")]
    creator: String,
    #[serde(rename = "p", skip_serializing_if = "String::is_empty")]
    post: String,
    #[serde(rename = "a", skip_serializing_if = "String::is_empty")]
    asset: String,
}
impl<'de> Deserialize<'de> for ResourceIdentity {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct IdentityVisitor;
        impl<'de> Visitor<'de> for IdentityVisitor {
            type Value = ResourceIdentity;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a FANBOX resource identity")
            }
            fn visit_map<M: MapAccess<'de>>(
                self,
                mut map: M,
            ) -> std::result::Result<Self::Value, M::Error> {
                let mut value = ResourceIdentity::default();
                while let Some(key) = map.next_key::<String>()? {
                    let field = match key.to_ascii_lowercase().as_str() {
                        "k" => Some(&mut value.kind),
                        "c" => Some(&mut value.creator),
                        "p" => Some(&mut value.post),
                        "a" => Some(&mut value.asset),
                        _ => None,
                    };
                    if let Some(field) = field {
                        if let Some(text) = map.next_value::<Option<String>>()? {
                            *field = text;
                        }
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }
                Ok(value)
            }
        }
        deserializer.deserialize_map(IdentityVisitor)
    }
}
fn resource_error(operation: &str, reason: Reason, message: &str) -> Error {
    Error::with_product("fanbox", reason, operation).with_cause(Cause::Redacted(message.into()))
}
pub(super) fn validate_media_url(raw: &str) -> std::result::Result<options::ParsedUrl, Failure> {
    let target = options::parse_url(raw)
        .filter(|target| target.parsed.scheme() == "https" && !target.authority.contains('@'))
        .ok_or_else(|| Failure::message("FANBOX URL is not an allowed HTTPS URL"))?;
    let host = target.parsed.host_str().unwrap_or("").to_ascii_lowercase();
    if !(host == "fanbox.cc"
        || host.ends_with(".fanbox.cc")
        || host == "i.pximg.net"
        || host.ends_with(".pximg.net")
        || host == "fanbox.pixiv.net"
        || host.ends_with(".fanbox.pixiv.net"))
    {
        return Err(Failure::message("FANBOX media URL host is not allowed"));
    }
    Ok(target)
}
fn validate_resource_url(raw: &str) -> bool {
    validate_media_url(raw).ok().is_some_and(|target| {
        crate::reference::decode_url_component(&target.path, false)
            .is_some_and(|path| !path.is_empty() && path != "/")
    })
}
fn locator(reference: ResourceRef, url: &str) -> Resource {
    let requires_credentials = options::parse_url(url).is_some_and(|target| {
        target
            .parsed
            .host_str()
            .is_some_and(|host| host.eq_ignore_ascii_case("downloads.fanbox.cc"))
    });
    Resource {
        reference,
        url: url.into(),
        request_headers: std::collections::BTreeMap::from([(
            "Referer".into(),
            super::WEB_BASE_URL.into(),
        )]),
        expires_at: None,
        requires_credentials,
    }
}
impl Client {
    pub(super) fn new_resource(
        &self,
        kind: &str,
        creator: &str,
        post: &str,
        asset: &str,
        url: &str,
    ) -> Result<Resource> {
        if url.is_empty() {
            return Ok(Resource::default());
        }
        if !validate_resource_url(url) {
            return Err(resource_error(
                "resource",
                Reason::ResourceForbidden,
                "resource host is not allowed",
            ));
        }
        let identity = ResourceIdentity {
            kind: kind.into(),
            creator: creator.into(),
            post: post.into(),
            asset: asset.into(),
        };
        let payload = serde_json::to_string(&identity)
            .map_err(|_| {
                resource_error(
                    "resource",
                    Reason::UpstreamError,
                    "encode resource reference",
                )
            })?
            .replace('<', "\\u003c")
            .replace('>', "\\u003e")
            .replace('&', "\\u0026")
            .replace('\u{2028}', "\\u2028")
            .replace('\u{2029}', "\\u2029");
        let reference = ResourceRef::new("fanbox", payload.as_bytes())?;
        let resource = locator(reference, url);
        self.resources
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert(resource.reference.as_str().into(), resource.clone());
        Ok(resource)
    }
    async fn resolve_resource_url(
        &self,
        context: Arc<dyn RequestContext>,
        reference: &ResourceRef,
    ) -> Result<String> {
        let invalid = || {
            resource_error(
                "OpenResource",
                Reason::InvalidArgument,
                "invalid resource reference",
            )
        };
        if reference.is_zero() || reference.product().map_err(|_| invalid())? != "fanbox" {
            return Err(invalid());
        }
        let payload = reference.payload().map_err(|_| invalid())?;
        let normalized = crate::codec::normalize_json(&payload).map_err(|_| invalid())?;
        let identity: ResourceIdentity =
            serde_json::from_str(&normalized).map_err(|_| invalid())?;
        if identity.kind.is_empty() {
            return Err(invalid());
        }
        if let Some(resource) = self
            .resources
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(reference.as_str())
            .filter(|resource| !resource.url.is_empty())
        {
            return Ok(resource.url.clone());
        }
        let missing = || {
            resource_error(
                "OpenResource",
                Reason::MalformedUpstreamResponse,
                "resource metadata has no usable URL",
            )
        };
        let url = match identity.kind.as_str() {
            "creator_icon" | "creator_cover" => {
                if identity.creator.is_empty() {
                    return Err(missing());
                }
                let (icon, cover) = self
                    .resource_creator_metadata(context, &identity.creator)
                    .await?;
                let url = if identity.kind == "creator_icon" {
                    icon
                } else {
                    cover
                };
                if url.is_empty() {
                    return Err(missing());
                }
                url
            }
            "post_image" | "post_file" => {
                if identity.post.is_empty() || identity.asset.is_empty() {
                    return Err(missing());
                }
                self.resource_post_metadata(context, &identity.post)
                    .await?
                    .into_iter()
                    .find(|(asset, url)| asset == &identity.asset && !url.is_empty())
                    .map(|(_, url)| url)
                    .ok_or_else(missing)?
            }
            _ => {
                return Err(resource_error(
                    "OpenResource",
                    Reason::InvalidArgument,
                    "resource kind is unsupported",
                ));
            }
        };
        if !validate_resource_url(&url) {
            return Err(resource_error(
                "OpenResource",
                Reason::ResourceForbidden,
                "resolved resource URL is not allowed",
            ));
        }
        self.resources
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert(reference.as_str().into(), locator(reference.clone(), &url));
        Ok(url)
    }
    pub async fn open_resource(
        &self,
        context: Arc<dyn RequestContext>,
        request: OpenResourceRequest,
    ) -> Result<ResourceResponse<ResourceBody>> {
        request.validate()?;
        let url = self
            .resolve_resource_url(context.clone(), &request.reference)
            .await?;
        if !validate_resource_url(&url) {
            return Err(resource_error(
                "OpenResource",
                Reason::ResourceForbidden,
                "resource URL is not allowed",
            ));
        }
        let mut headers = Headers::new();
        for (name, value) in [
            ("Range", request.range),
            ("If-None-Match", request.if_none_match),
            ("If-Modified-Since", request.if_modified_since),
            ("If-Range", request.if_range),
        ] {
            if !value.is_empty() {
                headers.insert(name.into(), vec![value]);
            }
        }
        let method = if request.method.is_empty() {
            "GET"
        } else {
            &request.method
        };
        let mut response = self
            .session
            .request_media(context.clone(), &url, method, headers)
            .await
            .map_err(|failure| failure.classify("OpenResource"))?;
        let body = response.body.take().ok_or_else(|| {
            resource_error(
                "OpenResource",
                Reason::UpstreamError,
                "FANBOX media response has no body",
            )
        })?;
        Ok(ResourceResponse::new(
            i64::from(response.status),
            &response.headers,
            ResourceBody::new(context, body),
        ))
    }
    pub async fn save_resource(
        &self,
        context: Arc<dyn RequestContext>,
        reference: ResourceRef,
        options: SaveOptions,
    ) -> Result<SavedResource> {
        if options.path.trim().is_empty() {
            return Err(resource_error(
                "SaveResource",
                Reason::InvalidArgument,
                "destination path is required",
            ));
        }
        let mut response = self
            .open_resource(
                context.clone(),
                OpenResourceRequest {
                    reference,
                    method: "GET".into(),
                    ..Default::default()
                },
            )
            .await?;
        let written = AssertUnwindSafe(async {
            if !(200..300).contains(&response.status_code) {
                return Err(resource_error(
                    "SaveResource",
                    Reason::UpstreamError,
                    "resource returned a non-success status",
                ));
            }
            let local = || {
                resource_error(
                    "SaveResource",
                    Reason::LocalStateError,
                    "cannot write resource",
                )
            };
            let mut destination =
                Destination::create(Path::new(&options.path)).map_err(|_| local())?;
            let mut done = 0;
            let mut buffer = [0u8; 64 * 1024];
            loop {
                if context.error().is_some() {
                    return Err(local());
                }
                let read = response.body.read(&mut buffer).await;
                if read.count > buffer.len() {
                    return Err(local());
                }
                if read.count > 0 {
                    done += read.count as i64;
                    if let Some(progress) = &options.progress {
                        progress(SaveProgress { total: 0, done });
                    }
                    destination
                        .write(&buffer[..read.count])
                        .map_err(|_| local())?;
                }
                if read.error.is_some() {
                    return Err(local());
                }
                if read.eof {
                    break;
                }
            }
            destination
                .publish(Path::new(&options.path))
                .map_err(|_| local())?;
            Ok(SavedResource {
                path: options.path,
                size: done,
                content_type: String::new(),
            })
        })
        .catch_unwind()
        .await;
        let _ = response.body.close().await;
        match written {
            Ok(result) => result,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }
}
/// A fallible byte stream retaining simultaneous bytes and errors, with explicit caller-owned close.
pub struct ResourceBody {
    body: SafeMediaBody,
}
impl ResourceBody {
    pub(super) fn new(context: Arc<dyn RequestContext>, body: Box<dyn RawBody>) -> Self {
        Self {
            body: SafeMediaBody::new(context, body),
        }
    }
}
impl fmt::Debug for ResourceBody {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResourceBody")
            .finish_non_exhaustive()
    }
}
impl RawBody for ResourceBody {
    fn read<'a>(&'a mut self, output: &'a mut [u8]) -> BodyFuture<'a, RawRead> {
        self.body.read(output)
    }
    fn close(&mut self) -> BodyFuture<'_, std::result::Result<(), ExternalError>> {
        self.body.close()
    }
}
