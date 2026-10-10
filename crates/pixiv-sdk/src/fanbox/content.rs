use super::{
    Client, Failure, Session,
    identity::json_name_eq,
    models::*,
    safe_external_error,
    transport::{Headers, RawBody},
};
use crate::{Error, Reason, Result, context::RequestContext, cursor::Page, error::Cause};
use serde::{
    Deserialize, Deserializer,
    de::{MapAccess, Visitor},
};
use serde_json::value::RawValue;
use std::{collections::BTreeMap, fmt, sync::Arc};
const API: &str = "https://api.fanbox.cc/";
type DecodeResult<T> = std::result::Result<T, serde_json::Error>;
struct Entries(Vec<(String, Box<RawValue>)>);
impl<'de> Deserialize<'de> for Entries {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Entries;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("object or null")
            }
            fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Entries, E> {
                Ok(Entries(vec![]))
            }
            fn visit_map<M: MapAccess<'de>>(
                self,
                mut map: M,
            ) -> std::result::Result<Entries, M::Error> {
                let mut out = vec![];
                while let Some(pair) = map.next_entry()? {
                    out.push(pair);
                }
                Ok(Entries(out))
            }
        }
        d.deserialize_any(V)
    }
}
trait Merge: Default {
    fn merge(&mut self, raw: &str) -> DecodeResult<()>;
    fn decode(raw: &str) -> DecodeResult<Self> {
        let mut out = Self::default();
        out.merge(raw)?;
        Ok(out)
    }
}
fn primitive<T: for<'de> Deserialize<'de>>(raw: &str, target: &mut T) -> DecodeResult<()> {
    if let Some(value) = serde_json::from_str::<Option<T>>(raw)? {
        *target = value;
    }
    Ok(())
}
fn slice<T: Merge>(raw: &str) -> DecodeResult<Option<Vec<T>>> {
    let values: Option<Vec<Box<RawValue>>> = serde_json::from_str(raw)?;
    values
        .map(|values| values.iter().map(|v| T::decode(v.get())).collect())
        .transpose()
}
fn pointer<T: Merge>(raw: &str, target: &mut Option<T>) -> DecodeResult<()> {
    if raw == "null" {
        *target = None;
    } else {
        target.get_or_insert_with(T::default).merge(raw)?;
    }
    Ok(())
}
fn map<T: Merge>(raw: &str, target: &mut BTreeMap<String, T>) -> DecodeResult<()> {
    if raw == "null" {
        target.clear();
        return Ok(());
    }
    for (key, value) in serde_json::from_str::<Entries>(raw)?.0 {
        target.insert(key, T::decode(value.get())?);
    }
    Ok(())
}
macro_rules! wire {
    ($name:ident { $($field:ident: $ty:ty => $json:literal $mode:ident),* $(,)? }) => {
        #[derive(Default)]
        struct $name { $( $field: $ty ),* }
        impl Merge for $name {
            fn merge(&mut self, raw: &str) -> DecodeResult<()> {
                for (key, value) in serde_json::from_str::<Entries>(raw)?.0 {
                    $(
                        if json_name_eq(&key, $json) {
                            wire!(@read self.$field, value.get(), $mode);
                            continue;
                        }
                    )*
                }
                Ok(())
            }
        }
    };
    (@read $field:expr, $raw:expr, primitive) => { primitive($raw, &mut $field)? };
    (@read $field:expr, $raw:expr, optional) => { $field = serde_json::from_str($raw)? };
    (@read $field:expr, $raw:expr, nested) => { $field.merge($raw)? };
    (@read $field:expr, $raw:expr, pointer) => { pointer($raw, &mut $field)? };
    (@read $field:expr, $raw:expr, slice) => { $field = slice($raw)? };
    (@read $field:expr, $raw:expr, map) => { map($raw, &mut $field)? };
    (@read $field:expr, $raw:expr, raw) => { $field = $raw.to_owned() };
}
wire!(Envelope {
    body:String=>"body" raw,
});
wire!(ProfileEnvelope {
    body:ProfileWire=>"body" nested,
});
wire!(ProfileWire {
    creator_id:String=>"creatorId" primitive,
    user:ProfileUser=>"user" nested,
    has_adult_content:bool=>"hasAdultContent" primitive,
    is_following:bool=>"isFollowing" primitive,
    cover_image_url:String=>"coverImageUrl" primitive,
    plan:PlanWire=>"plan" nested,
});
wire!(ProfileUser {
    name:String=>"name" primitive,
    icon_url:String=>"iconUrl" primitive,
});
wire!(PlanWire {
    fee:i64=>"fee" primitive,
    has_supporting_plan:bool=>"hasSupportingPlan" primitive,
});
wire!(CreatorWire {
    creator_id:String=>"creatorId" primitive,
});
wire!(CreatorsWire {
    plans:Option<Vec<CreatorWire>> =>"plans" slice,
    creators:Option<Vec<CreatorWire>> =>"creators" slice,
    next_url:String=>"nextUrl" primitive,
    page_urls:Option<Vec<StringWire>> =>"pageUrls" slice,
});
#[derive(Default)]
struct StringWire(String);
impl Merge for StringWire {
    fn merge(&mut self, raw: &str) -> DecodeResult<()> {
        primitive(raw, &mut self.0)
    }
}
wire!(TagWire {
    tag:String=>"tag" primitive,
    url:String=>"url" primitive,
});
wire!(TagsWire {
    tags:Option<Vec<TagWire>> =>"tags" slice,
});
wire!(PostInfoWire {
    post:PostWire=>"post" nested,
});
wire!(PageWire {
    posts:Option<Vec<PostWire>> =>"posts" slice,
    items:Option<Vec<PostWire>> =>"items" slice,
    next_url:String=>"nextUrl" primitive,
    page_urls:Option<Vec<StringWire>> =>"pageUrls" slice,
});
wire!(PostWire {
    id:String=>"id" primitive,
    title:String=>"title" primitive,
    published_datetime:String=>"publishedDatetime" primitive,
    creator_id:String=>"creatorId" primitive,
    fee_required:i64=>"feeRequired" primitive,
    is_restricted:bool=>"isRestricted" primitive,
    is_pinned:bool=>"isPinned" primitive,
    restricted_for:i64=>"restrictedFor" primitive,
    comment_count:i64=>"commentCount" primitive,
    body:Option<BodyWire> =>"body" pointer,
});
wire!(BodyWire {
    text:String=>"text" primitive,
    files:Option<Vec<FileWire>> =>"files" slice,
    images:Option<Vec<ImageWire>> =>"images" slice,
    blocks:Option<Vec<BlockWire>> =>"blocks" slice,
    image_map:BTreeMap<String,ImageWire> =>"imageMap" map,
    file_map:BTreeMap<String,FileWire> =>"fileMap" map,
});
wire!(BlockWire {
    r#type:String=>"type" primitive,
    image_id:Option<String> =>"imageId" optional,
    file_id:Option<String> =>"fileId" optional,
});
wire!(ImageWire {
    id:String=>"id" primitive,
    extension:String=>"extension" primitive,
    original_url:String=>"originalUrl" primitive,
    thumbnail_url:String=>"thumbnailUrl" primitive,
});
wire!(FileWire {
    id:String=>"id" primitive,
    name:String=>"name" primitive,
    extension:String=>"extension" primitive,
    url:String=>"url" primitive,
});
fn malformed(message: &str) -> Failure {
    Failure::message(message)
}
fn endpoint(route: &str, query: &[(&str, &str)]) -> String {
    let parameters = query
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect::<Vec<_>>();
    let query = crate::transport::encode_query_pairs(&parameters);
    format!(
        "{API}{route}{}{}",
        if query.is_empty() { "" } else { "?" },
        query
    )
}
fn next_url(urls: Option<Vec<StringWire>>, next: String) -> String {
    urls.and_then(|v| v.into_iter().next())
        .map(|v| v.0)
        .unwrap_or(next)
}
fn input_error(op: &str, message: &str) -> Error {
    Failure::new(Reason::InvalidArgument, message).classify(op)
}
fn trimmed_required<'a>(value: &'a str, message: &str) -> std::result::Result<&'a str, Failure> {
    let value = value.trim();
    if value.is_empty() {
        Err(malformed(message))
    } else {
        Ok(value)
    }
}
impl Session {
    async fn get_json<T>(
        &self,
        context: Arc<dyn RequestContext>,
        url: &str,
        decode: impl FnOnce(&str) -> DecodeResult<T>,
    ) -> std::result::Result<T, Failure> {
        let mut response = self
            .request_at(
                context.clone(),
                url,
                "GET",
                "application/json, text/plain, */*",
                Headers::new(),
                false,
            )
            .await?;
        let Some(body) = response.body.as_mut() else {
            return Err(malformed("FANBOX API response has no body"));
        };
        let document = read_json(body.as_mut()).await;
        let decoded = document.and_then(|raw| {
            decode(&raw).map_err(|error| Box::new(error) as super::transport::ExternalError)
        });
        let closed = body.close().await;
        match (decoded, closed) {
            (Ok(value), Ok(())) => Ok(value),
            (Ok(_), Err(error)) => Err(Failure {
                code: Reason::UpstreamError,
                cause: safe_external_error(
                    context.as_ref(),
                    "close FANBOX API response failed",
                    error.as_ref(),
                ),
            }),
            (Err(error), close) => {
                let cause = safe_external_error(
                    context.as_ref(),
                    "decode FANBOX API response",
                    error.as_ref(),
                );
                let cause = match close {
                    Ok(()) => cause,
                    Err(error) => Cause::Joined(vec![
                        cause,
                        safe_external_error(
                            context.as_ref(),
                            "close FANBOX API response failed",
                            error.as_ref(),
                        ),
                    ]),
                };
                Err(Failure {
                    code: Reason::UpstreamError,
                    cause,
                })
            }
        }
    }
}
async fn read_json(
    body: &mut dyn RawBody,
) -> std::result::Result<String, super::transport::ExternalError> {
    let mut bytes = Vec::new();
    let mut capacity = 64;
    loop {
        if bytes.len() == capacity {
            capacity *= 2;
        }
        let mut buffer = vec![0; capacity - bytes.len()];
        let read = body.read(&mut buffer).await;
        if read.count > buffer.len() {
            return Err(Box::new(std::io::Error::other("invalid body read count")));
        }
        bytes.extend_from_slice(&buffer[..read.count]);
        let value = super::json::first_value(&bytes);
        if let Ok(value) = value.as_ref()
            && serde_json::from_str::<Box<RawValue>>(value).is_ok()
        {
            return Ok(value.clone());
        }
        if let Some(error) = read.error {
            return Err(error);
        }
        if read.eof {
            return value.map_err(|error| Box::new(error) as super::transport::ExternalError);
        }
    }
}
impl Client {
    async fn raw_body(
        &self,
        context: Arc<dyn RequestContext>,
        url: &str,
    ) -> std::result::Result<String, Failure> {
        self.session
            .get_json(context, url, Envelope::decode)
            .await
            .map(|v| v.body)
    }
    async fn profile_wire(
        &self,
        context: Arc<dyn RequestContext>,
        creator: &str,
    ) -> std::result::Result<ProfileWire, Failure> {
        let id = trimmed_required(creator, "FANBOX creator id is required")?;
        let value = self
            .session
            .get_json(
                context,
                &endpoint("creator.get", &[("creatorId", id)]),
                ProfileEnvelope::decode,
            )
            .await?
            .body;
        let mut value = value;
        value.creator_id = value.creator_id.trim().to_owned();
        if value.creator_id.is_empty() {
            value.creator_id = id.into();
        }
        value.user.name = value.user.name.trim().to_owned();
        value.user.icon_url = value.user.icon_url.trim().to_owned();
        value.cover_image_url = value.cover_image_url.trim().to_owned();
        if value.user.name.is_empty() {
            return Err(malformed(
                "FANBOX creator profile is missing creator id or display name",
            ));
        }
        Ok(value)
    }
    async fn post_wire(
        &self,
        context: Arc<dyn RequestContext>,
        id: &str,
    ) -> std::result::Result<PostWire, Failure> {
        let id = trimmed_required(id, "FANBOX post id is required")?;
        let raw = self
            .raw_body(context, &endpoint("post.info", &[("postId", id)]))
            .await?;
        let value = PostInfoWire::decode(&raw)
            .map_err(|_| malformed("decode FANBOX post.info response"))?
            .post;
        validate_post(&value)?;
        Ok(value)
    }
    pub(super) async fn resource_creator_metadata(
        &self,
        context: Arc<dyn RequestContext>,
        creator: &str,
    ) -> Result<(String, String)> {
        let profile = self
            .profile_wire(context, creator)
            .await
            .map_err(|e| e.classify("resource"))?;
        Ok((profile.user.icon_url, profile.cover_image_url))
    }
    pub(super) async fn resource_post_metadata(
        &self,
        context: Arc<dyn RequestContext>,
        post: &str,
    ) -> Result<Vec<(String, String)>> {
        let post = self
            .post_wire(context, post)
            .await
            .map_err(|e| e.classify("resource"))?;
        let Some(body) = post.body else {
            return Ok(vec![]);
        };
        let (images, files, _) = normalize_body(body).map_err(|e| e.classify("resource"))?;
        Ok(images
            .into_iter()
            .map(|v| (v.id, v.original_url))
            .chain(files.into_iter().map(|v| (v.id, v.url)))
            .collect())
    }
    pub async fn creator(
        &self,
        context: Arc<dyn RequestContext>,
        request: CreatorRequest,
    ) -> Result<Creator> {
        if request.creator_id.is_empty() {
            return Err(input_error("Creator", "creator id is required"));
        }
        let value = self
            .profile_wire(context, &request.creator_id)
            .await
            .map_err(|e| e.classify("Creator"))?;
        let icon = self.new_resource(
            "creator_icon",
            &value.creator_id,
            "",
            "",
            &value.user.icon_url,
        )?;
        let cover = self.new_resource(
            "creator_cover",
            &value.creator_id,
            "",
            "",
            &value.cover_image_url,
        )?;
        Ok(Creator {
            id: value.creator_id,
            name: value.user.name,
            icon: ImageResource {
                resource: icon,
                ..Default::default()
            },
            cover: ImageResource {
                resource: cover,
                ..Default::default()
            },
            has_adult_content: value.has_adult_content,
            is_following: value.is_following,
            plan_fee: value.plan.fee,
            has_supporting_plan: value.plan.has_supporting_plan,
        })
    }
    pub async fn creators(
        &self,
        context: Arc<dyn RequestContext>,
        request: CreatorsRequest,
    ) -> Result<Page<CreatorSummary>> {
        let kind = if request.kind.is_empty() {
            "supporting"
        } else {
            &request.kind
        };
        let query = [("kind", kind)];
        let next = self
            .continuation_url(context.clone(), "Creators", &query, &request.cursor)
            .await?;
        let target = if next.trim().is_empty() {
            match kind {
                "supporting" => endpoint("plan.listSupporting", &[]),
                "following" => endpoint("creator.listFollowing", &[]),
                _ => {
                    return Err(
                        malformed("FANBOX creator list kind is invalid").classify("Creators")
                    );
                }
            }
        } else {
            validate_api_url(next.trim()).map_err(|e| e.classify("Creators"))?;
            next.trim().to_owned()
        };
        let raw = self
            .raw_body(context.clone(), &target)
            .await
            .map_err(|e| e.classify("Creators"))?;
        let decoded = decode_creators(&raw, kind).map_err(|e| e.classify("Creators"))?;
        let items = decoded
            .0
            .into_iter()
            .map(|id| CreatorSummary {
                id,
                ..Default::default()
            })
            .collect();
        let next = self
            .build_cursor(context, "Creators", &query, &decoded.1)
            .await?;
        Ok(Page { items, next })
    }
    pub async fn creator_tags(
        &self,
        context: Arc<dyn RequestContext>,
        request: CreatorTagsRequest,
    ) -> Result<Vec<CreatorTag>> {
        if request.creator_id.is_empty() {
            return Err(input_error("CreatorTags", "creator id is required"));
        }
        let id = trimmed_required(&request.creator_id, "FANBOX creator id is required")
            .map_err(|e| e.classify("CreatorTags"))?;
        let raw = self
            .raw_body(context, &endpoint("tag.getFeatured", &[("creatorId", id)]))
            .await
            .map_err(|e| e.classify("CreatorTags"))?;
        let values = match slice::<TagWire>(&raw) {
            Ok(v) => v.unwrap_or_default(),
            Err(_) => TagsWire::decode(&raw)
                .map_err(|_| malformed("decode FANBOX creator tags").classify("CreatorTags"))?
                .tags
                .unwrap_or_default(),
        };
        values
            .into_iter()
            .map(|value| {
                let name = value.tag.trim().to_owned();
                if name.is_empty() {
                    return Err(
                        malformed("FANBOX creator tag response includes an empty tag")
                            .classify("CreatorTags"),
                    );
                }
                Ok(CreatorTag {
                    name,
                    url: value.url,
                })
            })
            .collect()
    }
    pub async fn post(
        &self,
        context: Arc<dyn RequestContext>,
        request: PostRequest,
    ) -> Result<Post> {
        if request.post_id.is_empty() {
            return Err(input_error("Post", "post id is required"));
        }
        let value = self
            .post_wire(context, &request.post_id)
            .await
            .map_err(|e| e.classify("Post"))?;
        self.map_post(value)
    }
    pub async fn creator_posts(
        &self,
        context: Arc<dyn RequestContext>,
        request: CreatorPostsRequest,
    ) -> Result<Page<Post>> {
        if request.creator_id.is_empty() {
            return Err(input_error("CreatorPosts", "creator id is required"));
        }
        let query = [("creatorId", request.creator_id.as_str())];
        let next = self
            .continuation_url(context.clone(), "CreatorPosts", &query, &request.cursor)
            .await?;
        let id = trimmed_required(&request.creator_id, "FANBOX creator id is required")
            .map_err(|e| e.classify("CreatorPosts"))?;
        self.post_page(
            context,
            "CreatorPosts",
            &query,
            endpoint("post.listCreator", &[("creatorId", id), ("limit", "10")]),
            next,
            false,
        )
        .await
    }
    pub async fn tagged_posts(
        &self,
        context: Arc<dyn RequestContext>,
        request: TaggedPostsRequest,
    ) -> Result<Page<Post>> {
        if request.creator_id.is_empty() || request.tag.is_empty() {
            return Err(input_error(
                "TaggedPosts",
                "creator id and tag are required",
            ));
        }
        let query = [
            ("creatorId", request.creator_id.as_str()),
            ("tag", request.tag.as_str()),
        ];
        let next = self
            .continuation_url(context.clone(), "TaggedPosts", &query, &request.cursor)
            .await?;
        let id = trimmed_required(&request.creator_id, "FANBOX creator id is required")
            .map_err(|e| e.classify("TaggedPosts"))?;
        let tag = trimmed_required(&request.tag, "FANBOX tag is required")
            .map_err(|e| e.classify("TaggedPosts"))?;
        self.post_page(
            context,
            "TaggedPosts",
            &query,
            endpoint("post.listTagged", &[("creatorId", id), ("tag", tag)]),
            next,
            false,
        )
        .await
    }
    pub async fn home(
        &self,
        context: Arc<dyn RequestContext>,
        request: HomeRequest,
    ) -> Result<Page<Post>> {
        let next = self
            .continuation_url(context.clone(), "Home", &[], &request.cursor)
            .await?;
        self.post_page(
            context,
            "Home",
            &[],
            endpoint("post.listHome", &[("limit", "10")]),
            next,
            true,
        )
        .await
    }
    pub async fn supporting(
        &self,
        context: Arc<dyn RequestContext>,
        request: SupportingRequest,
    ) -> Result<Page<Post>> {
        let next = self
            .continuation_url(context.clone(), "Supporting", &[], &request.cursor)
            .await?;
        self.post_page(
            context,
            "Supporting",
            &[],
            endpoint("post.listSupporting", &[("limit", "10")]),
            next,
            true,
        )
        .await
    }
    async fn post_page(
        &self,
        context: Arc<dyn RequestContext>,
        op: &str,
        query: &[(&str, &str)],
        target: String,
        next: String,
        accept_items: bool,
    ) -> Result<Page<Post>> {
        let target = if next.trim().is_empty() {
            target
        } else {
            validate_api_url(&next).map_err(|e| e.classify(op))?;
            next
        };
        let raw = self
            .raw_body(context.clone(), &target)
            .await
            .map_err(|e| e.classify(op))?;
        let page = PageWire::decode(&raw)
            .map_err(|_| malformed("decode FANBOX post list response").classify(op))?;
        let next_url = next_url(page.page_urls, page.next_url);
        let values = if accept_items {
            page.posts.or(page.items)
        } else {
            page.posts
        };
        let mut items = vec![];
        for value in values.unwrap_or_default() {
            validate_post(&value).map_err(|e| e.classify(op))?;
            items.push(self.map_post(value)?);
        }
        let next = self.build_cursor(context, op, query, &next_url).await?;
        Ok(Page { items, next })
    }
    fn map_post(&self, value: PostWire) -> Result<Post> {
        let published_at = parse_time(&value.published_datetime)?;
        let mut out = Post {
            id: value.id,
            title: value.title,
            published_at,
            creator_id: value.creator_id,
            fee_required: value.fee_required,
            is_restricted: value.is_restricted,
            is_pinned: value.is_pinned,
            restricted_for: value.restricted_for,
            comment_count: value.comment_count,
            ..Default::default()
        };
        let Some(body) = value.body else {
            return Ok(out);
        };
        let text = body.text.clone();
        let (images, files, blocks) = normalize_body(body).map_err(|e| e.classify("Post"))?;
        let mut result = PostBody {
            text,
            ..Default::default()
        };
        let mut image_by_id = BTreeMap::new();
        let mut file_by_id = BTreeMap::new();
        for image in images {
            let res = self.new_resource(
                "post_image",
                &out.creator_id,
                &out.id,
                &image.id,
                &image.original_url,
            )?;
            result.assets.get_or_insert_with(Vec::new).push(Asset {
                id: image.id.clone(),
                kind: AssetKind::Image,
                resource: res,
                ..Default::default()
            });
            image_by_id.insert(image.id.clone(), image);
        }
        for file in files {
            let res =
                self.new_resource("post_file", &out.creator_id, &out.id, &file.id, &file.url)?;
            result.assets.get_or_insert_with(Vec::new).push(Asset {
                id: file.id.clone(),
                kind: AssetKind::File,
                name: file.name.clone(),
                resource: res,
                ..Default::default()
            });
            file_by_id.insert(file.id.clone(), file);
        }
        for block in blocks {
            let mut value = PostBlock::default();
            match block.r#type.as_str() {
                "image" => {
                    let Some(image) = image_by_id.get(block.image_id.as_deref().unwrap_or(""))
                    else {
                        continue;
                    };
                    value.kind = PostBlockKind::Image;
                    value.image = Some(PostImageBlock {
                        resource: self.new_resource(
                            "post_image",
                            &out.creator_id,
                            &out.id,
                            &image.id,
                            &image.original_url,
                        )?,
                        ..Default::default()
                    });
                }
                "file" => {
                    let Some(file) = file_by_id.get(block.file_id.as_deref().unwrap_or("")) else {
                        continue;
                    };
                    value.kind = PostBlockKind::File;
                    value.file = Some(PostFileBlock {
                        resource: self.new_resource(
                            "post_file",
                            &out.creator_id,
                            &out.id,
                            &file.id,
                            &file.url,
                        )?,
                        name: file.name.clone(),
                        ..Default::default()
                    });
                }
                "article" => {
                    value.kind = PostBlockKind::Article;
                    value.article = Some(PostArticleBlock::default());
                }
                "video" => {
                    value.kind = PostBlockKind::Video;
                    value.video = Some(PostVideoEmbed::default());
                }
                _ => {
                    value.kind = PostBlockKind::Unknown;
                    value.unknown = Some(PostUnknownBlock {
                        raw_type: block.r#type,
                        ..Default::default()
                    });
                }
            }
            result.blocks.get_or_insert_with(Vec::new).push(value);
        }
        out.body = Some(result);
        Ok(out)
    }
}
pub(super) fn validate_api_url(raw: &str) -> std::result::Result<(), Failure> {
    let target = super::options::parse_url(raw)
        .ok_or_else(|| malformed("FANBOX URL is not an allowed HTTPS URL"))?;
    if target.parsed.scheme() != "https" || target.authority.contains('@') {
        return Err(malformed("FANBOX URL is not an allowed HTTPS URL"));
    }
    let host = target.parsed.host_str().unwrap_or("").to_ascii_lowercase();
    if host != "fanbox.cc" && !host.ends_with(".fanbox.cc") {
        return Err(malformed("FANBOX URL host is not allowed"));
    }
    Ok(())
}
fn decode_creators(raw: &str, kind: &str) -> std::result::Result<(Vec<String>, String), Failure> {
    let (values, next) = match slice::<CreatorWire>(raw) {
        Ok(values) => (values.unwrap_or_default(), String::new()),
        Err(_) => {
            let page = CreatorsWire::decode(raw)
                .map_err(|_| malformed(&format!("decode FANBOX {kind} creators")))?;
            let values = match kind {
                "supporting" => page.plans,
                "following" => page.creators,
                _ => return Err(malformed("FANBOX creator list kind is invalid")),
            }
            .ok_or_else(|| malformed(&format!("decode FANBOX {kind} creators")))?;
            (values, next_url(page.page_urls, page.next_url))
        }
    };
    let values = values
        .into_iter()
        .map(|value| {
            let id = value.creator_id.trim().to_owned();
            if id.is_empty() {
                Err(malformed(
                    "FANBOX creator response includes an empty creator id",
                ))
            } else {
                Ok(id)
            }
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok((values, next))
}
fn validate_post(post: &PostWire) -> std::result::Result<(), Failure> {
    if post.id.trim().is_empty() {
        return Err(malformed("FANBOX post has no id"));
    }
    if let Some(body) = &post.body {
        validate_body(body)?;
    }
    Ok(())
}
fn validate_image(value: &ImageWire) -> std::result::Result<(), Failure> {
    if value.id.trim().is_empty() {
        return Err(malformed("FANBOX image has no id"));
    }
    super::resource::validate_media_url(&value.original_url)?;
    if !value.thumbnail_url.is_empty() {
        super::resource::validate_media_url(&value.thumbnail_url)?;
    }
    Ok(())
}
fn validate_file(value: &FileWire) -> std::result::Result<(), Failure> {
    if value.id.trim().is_empty() {
        return Err(malformed("FANBOX file has no id"));
    }
    super::resource::validate_media_url(&value.url)?;
    Ok(())
}
fn validate_body(body: &BodyWire) -> std::result::Result<(), Failure> {
    if let Some(images) = &body.images {
        for v in images {
            validate_image(v)?;
        }
    } else if let Some(files) = &body.files {
        for v in files {
            validate_file(v)?;
        }
    } else if let Some(blocks) = &body.blocks {
        for b in blocks {
            if let Some(id) = &b.image_id {
                validate_image(
                    body.image_map
                        .get(id)
                        .ok_or_else(|| malformed("FANBOX blog block references a missing image"))?,
                )?;
            }
            if let Some(id) = &b.file_id {
                validate_file(
                    body.file_map
                        .get(id)
                        .ok_or_else(|| malformed("FANBOX blog block references a missing file"))?,
                )?;
            }
        }
    }
    Ok(())
}
type NormalizedBody = (Vec<ImageWire>, Vec<FileWire>, Vec<BlockWire>);
fn normalize_body(body: BodyWire) -> std::result::Result<NormalizedBody, Failure> {
    validate_body(&body)?;
    let mut images = vec![];
    let mut files = vec![];
    let mut blocks = vec![];
    if let Some(values) = body.images {
        images = values;
    } else if let Some(values) = body.files {
        files = values;
    } else if let Some(values) = body.blocks {
        let mut seen_images = std::collections::BTreeSet::new();
        let mut seen_files = std::collections::BTreeSet::new();
        for block in &values {
            if let Some(id) = &block.image_id {
                let value = body.image_map.get(id).expect("validated image");
                if seen_images.insert(value.id.clone()) {
                    images.push(ImageWire {
                        id: value.id.clone(),
                        extension: value.extension.clone(),
                        original_url: value.original_url.clone(),
                        thumbnail_url: value.thumbnail_url.clone(),
                    });
                }
            }
            if let Some(id) = &block.file_id {
                let value = body.file_map.get(id).expect("validated file");
                if seen_files.insert(value.id.clone()) {
                    files.push(FileWire {
                        id: value.id.clone(),
                        name: value.name.clone(),
                        extension: value.extension.clone(),
                        url: value.url.clone(),
                    });
                }
            }
        }
        blocks = values;
    }
    Ok((images, files, blocks))
}
fn parse_time(value: &str) -> Result<chrono::DateTime<chrono::Utc>> {
    if value.is_empty() {
        return Err(
            Failure::new(Reason::MalformedUpstreamResponse, "no publish time").classify("Post"),
        );
    }
    if value.as_bytes().get(4) == Some(&b'-')
        && let Some((seconds, nanos)) = super::expiry_date::parse(value)
        && let Some(value) = chrono::DateTime::from_timestamp(seconds, nanos)
    {
        return Ok(value);
    }
    let layout = "2006-01-02T15:04:05Z07:00";
    let mut remaining = value;
    let mut failed = None;
    for (token, width, numeric) in [
        ("2006", 4, true),
        ("-", 1, false),
        ("01", 2, true),
        ("-", 1, false),
        ("02", 2, true),
        ("T", 1, false),
        ("15", 2, true),
        (":", 1, false),
        ("04", 2, true),
        (":", 1, false),
        ("05", 2, true),
    ] {
        let width = if token == "15"
            && remaining
                .as_bytes()
                .get(1)
                .is_some_and(|v| !v.is_ascii_digit())
        {
            1
        } else {
            width
        };
        let prefix = remaining.get(..width);
        if prefix.is_none_or(|prefix| {
            if numeric {
                !prefix.bytes().all(|v| v.is_ascii_digit())
            } else {
                prefix != token
            }
        }) {
            failed = Some((remaining, token));
            break;
        }
        let part = prefix.expect("checked prefix");
        if numeric {
            let number = part.parse::<u32>().unwrap_or_default();
            let range = match token {
                "01" if number == 0 || number > 12 => Some("month"),
                "15" if number > 23 => Some("hour"),
                "04" if number > 59 => Some("minute"),
                "05" if number > 59 => Some("second"),
                _ => None,
            };
            if let Some(range) = range {
                return Err(Failure::new(
                    Reason::MalformedUpstreamResponse,
                    format!("parsing time {value:?}: {range} out of range"),
                )
                .classify("Post"));
            }
        }
        remaining = &remaining[width..];
    }
    let message = if let Some((rest, token)) = failed {
        format!("parsing time {value:?} as {layout:?}: cannot parse {rest:?} as {token:?}")
    } else {
        format!("parsing time {value:?} as {layout:?}: cannot parse {remaining:?} as \"Z07:00\"")
    };
    Err(Failure::new(Reason::MalformedUpstreamResponse, message).classify("Post"))
}
