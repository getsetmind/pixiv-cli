mod schema;
pub mod stdio;
pub use schema::{initialize, tools};

use chrono::{Datelike, Timelike};
use pixiv_app::{
    fanbox_facade::{Facade, OpenRequest},
    lifecycle::{Context, Lease},
    scheduler::SchedulerError,
};
use pixiv_sdk::{
    context::RequestContext,
    cursor::{Cursor, Page},
    diagnostics::Event,
    fanbox::{self as sdk, Client},
    resource::{OpenResourceRequest, Resource, ResourceRef},
};
use sdk::transport::RawBody;
use serde_json::{Map, Value, json};
use std::{
    collections::BTreeSet,
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

pub type ClientLease = Lease<Arc<Client>, SchedulerError>;
pub type OpenLeaseFuture =
    Pin<Box<dyn Future<Output = Result<Option<ClientLease>, SchedulerError>> + Send>>;
pub type OpenClientFuture =
    Pin<Box<dyn Future<Output = Result<Option<Arc<Client>>, SchedulerError>> + Send>>;
pub type OpenLease = dyn Fn(Context, Account) -> OpenLeaseFuture + Send + Sync;
pub type OpenClient = dyn Fn(Context, Account) -> OpenClientFuture + Send + Sync;

#[derive(Clone, Default)]
pub struct Account {
    pub https_proxy_override: Option<String>,
}
#[derive(Default)]
pub struct SdkPorts {
    pub open: Option<Arc<OpenClient>>,
    pub open_lease: Option<Arc<OpenLease>>,
}

struct OwnedLease(ClientLease);
impl Drop for OwnedLease {
    fn drop(&mut self) {
        let _ = self.0.close();
    }
}

pub struct Server {
    ports: SdkPorts,
    account: Account,
    request_counter: AtomicU64,
}
impl Server {
    pub fn new(ports: SdkPorts, proxy_override: Option<String>) -> Self {
        Self {
            ports,
            account: Account {
                https_proxy_override: proxy_override,
            },
            request_counter: AtomicU64::new(0),
        }
    }
    pub fn saved(facade: Arc<Facade>, proxy_override: Option<String>) -> Self {
        Self::new(
            SdkPorts {
                open_lease: Some(Arc::new(move |context, account| {
                    let facade = facade.clone();
                    Box::pin(async move {
                        facade
                            .open(
                                Some(&context),
                                OpenRequest {
                                    proxy_override: account.https_proxy_override,
                                },
                            )
                            .await
                            .map(Some)
                    })
                })),
                ..Default::default()
            },
            proxy_override,
        )
    }
    async fn open_client(&self, context: &Context) -> Result<ClientLease, SchedulerError> {
        if let Some(open) = &self.ports.open_lease {
            return open(context.clone(), self.account.clone())
                .await?
                .ok_or_else(|| {
                    SchedulerError::Message("lifecycle: open function returned a nil lease".into())
                });
        }
        if let Some(open) = &self.ports.open {
            let client = open(context.clone(), self.account.clone())
                .await?
                .ok_or_else(|| {
                    SchedulerError::Message("lifecycle: open function returned a nil lease".into())
                })?;
            let closing = client.clone();
            return Ok(Lease::new(
                client,
                Some(Box::new(move || {
                    closing.close_idle_connections();
                    Ok(())
                })),
            ));
        }
        Err(SchedulerError::Message(
            "fanbox service is not configured".into(),
        ))
    }
    pub async fn call(
        &self,
        context: &Context,
        name: &str,
        arguments: &Value,
    ) -> Result<Value, String> {
        let input = decode(name, arguments)?;
        let id = self.request_counter.fetch_add(1, Ordering::Relaxed) + 1;
        let scoped = context.with_child_scope("FANBOX MCP server", id);
        let operation = format!("tool {name}");
        scoped.emit(Event {
            kind: "started".into(),
            operation: operation.clone(),
            ..Default::default()
        });
        let started = Instant::now();
        let result = self.handle(&scoped, name, input).await;
        let failed = result
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        scoped.emit(Event {
            kind: if failed { "failed" } else { "completed" }.into(),
            operation,
            reason: if failed { "tool failed" } else { "" }.into(),
            duration_ns: started.elapsed().as_nanos().min(i64::MAX as u128) as i64,
            ..Default::default()
        });
        Ok(result)
    }
    async fn handle(&self, context: &Context, name: &str, input: Input) -> Value {
        let empty = empty_output(name, &input);
        let plan = if matches!(
            name,
            "fanbox_creators"
                | "fanbox_home"
                | "fanbox_supporting"
                | "fanbox_creator_posts"
                | "fanbox_tagged_posts"
        ) {
            match Plan::new(input.page, input.limit) {
                Ok(plan) => Some(plan),
                Err(error) => return failure(empty, error),
            }
        } else {
            None
        };
        let invalid = match name {
            "fanbox_creator" | "fanbox_creator_tags" | "fanbox_creator_posts"
                if input.creator_id.is_empty() =>
            {
                Some("creator_id is required")
            }
            "fanbox_post" if input.post_id.is_empty() => Some("post_id is required"),
            "fanbox_tagged_posts" if input.creator_id.is_empty() || input.tag.is_empty() => {
                Some("creator_id and tag are required")
            }
            "fanbox_resolve_url" if input.url.trim().is_empty() => Some("url is required"),
            "fanbox_creators"
                if !matches!(input.kind.as_str(), "" | "supporting" | "following") =>
            {
                Some("kind must be one of: supporting, following")
            }
            _ => None,
        };
        if let Some(error) = invalid {
            return failure(empty, error);
        }
        let reference = if name == "fanbox_open_resource" {
            let reference = match ResourceRef::parse(&input.reference) {
                Ok(reference) => reference,
                Err(error) => return failure(empty, error.to_string()),
            };
            if !matches!(input.method.to_uppercase().as_str(), "" | "GET" | "HEAD") {
                return failure(empty, "method supports only \"GET\" or \"HEAD\"");
            }
            Some(reference)
        } else {
            None
        };
        let lease = match self.open_client(context).await {
            Ok(lease) => OwnedLease(lease),
            Err(error) => return failure(empty, error.to_string()),
        };
        let client = lease.0.value();
        let ctx: Arc<dyn RequestContext> = Arc::new(context.clone());
        let result:Result<(Value,String),String>=async {
            match name {
                "fanbox_current_user"=>{
                    let user=client.current_user(ctx,sdk::CurrentUserRequest{}).await.map_err(|error|error.to_string())?;
                    Ok((serde_json::to_value(user.to_dto()).expect("user DTO serializes"),format!("Current FANBOX user {}.",user.user_id)))
                }
                "fanbox_creator"=>{
                    let creator=client.creator(ctx,sdk::CreatorRequest {creator_id:input.creator_id}).await.map_err(|error|error.to_string())?;
                    let mut out=json!({"id":creator.id,"name":creator.name});
                    add_nonzero(&mut out,"has_adult_content",creator.has_adult_content);
                    add_nonzero(&mut out,"is_following",creator.is_following);
                    add_integer(&mut out,"plan_fee",creator.plan_fee);
                    add_nonzero(&mut out,"has_supporting_plan",creator.has_supporting_plan);
                    add_resource(&mut out,"icon",&creator.icon.resource);
                    add_resource(&mut out,"cover",&creator.cover.resource);
                    Ok((out,format!("Retrieved creator {}.",creator.id)))
                }
                "fanbox_creator_tags"=>{
                    let tags=client.creator_tags(ctx,sdk::CreatorTagsRequest {creator_id:input.creator_id}).await.map_err(|error|error.to_string())?;
                    let tags=tags.into_iter().map(|tag|{let mut out=json!({"name":tag.name});if !tag.url.is_empty(){out["url"]=json!(tag.url);}out}).collect::<Vec<_>>();
                    let message=format!("Retrieved {} tags.",tags.len());
                    Ok((json!({"tags":tags}),message))
                }
                "fanbox_post"=>{
                    let post=client.post(ctx,sdk::PostRequest {post_id:input.post_id}).await.map_err(|error|error.to_string())?;
                    Ok((post_output(&post),format!("Retrieved post {}.",post.id)))
                }
                "fanbox_resolve_url"=>{
                    let reference=client.resolve_url(ctx,sdk::ResolveURLRequest {raw_url:input.url}).map_err(|error|error.to_string())?;
                    let mut out=json!({"kind":reference.kind.as_str()});
                    for (key,value) in [("creator_id",reference.creator_id),("post_id",reference.post_id),("tag",reference.tag)] {if !value.is_empty(){out[key]=json!(value);}}
                    Ok((out,format!("Resolved URL as {}.",reference.kind.as_str())))
                }
                "fanbox_open_resource"=>{
                    let mut response=client.open_resource(ctx,OpenResourceRequest {reference:reference.expect("resource ref parsed"),method:input.method.to_uppercase(),..Default::default()}).await.map_err(|error|error.to_string())?;
                    let mut out=json!({"ref":input.reference,"status_code":response.status_code});
                    if !response.content_type().is_empty(){out["content_type"]=json!(response.content_type());}
                    add_integer(&mut out,"content_length",response.content_length());
                    let _=response.body.close().await;
                    Ok((out,format!("Resource status {}.",response.status_code)))
                }
                "fanbox_creators"=>{
                    let kind=if input.kind.is_empty(){"supporting".into()}else{input.kind};
                    let (creators,more)=collect(context,plan.expect("list plan"),|cursor|client.creators(ctx.clone(),sdk::CreatorsRequest {kind:kind.clone(),cursor})).await?;
                    let creators=creators.iter().map(|creator|{let mut out=json!({"id":creator.id});if !creator.name.is_empty(){out["name"]=json!(creator.name);}add_resource(&mut out,"icon",&creator.icon.resource);out}).collect::<Vec<_>>();
                    let message=format!("Retrieved {} creators.",creators.len());
                    Ok((json!({"creators":creators,"pagination":pagination(plan.expect("list plan"),input.limit,creators.len(),more)}),message))
                }
                _=>{
                    let (posts,more)=collect(context,plan.expect("list plan"),|cursor|{
                        let ctx=ctx.clone(); let creator_id=input.creator_id.clone();let tag=input.tag.clone();
                        async move {
                            match name {
                                "fanbox_home"=>client.home(ctx,sdk::HomeRequest {cursor}).await,
                                "fanbox_supporting"=>client.supporting(ctx,sdk::SupportingRequest {cursor}).await,
                                "fanbox_creator_posts"=>client.creator_posts(ctx,sdk::CreatorPostsRequest {creator_id,cursor}).await,
                                _=>client.tagged_posts(ctx,sdk::TaggedPostsRequest {creator_id,tag,cursor}).await,
                            }
                        }
                    }).await?;
                    let message=format!("Retrieved {} posts.",posts.len());
                    Ok((json!({"posts":posts.iter().map(post_output).collect::<Vec<_>>(),"pagination":pagination(plan.expect("list plan"),input.limit,posts.len(),more)}),message))
                }
            }
        }.await;
        let _ = lease.0.close();
        match result {
            Ok((out, message)) => tool_result(out, false, message),
            Err(error) => failure(empty, error),
        }
    }
}

#[derive(Default)]
struct Input {
    creator_id: String,
    post_id: String,
    tag: String,
    kind: String,
    url: String,
    reference: String,
    method: String,
    page: Option<i64>,
    limit: Option<i64>,
}
fn decode(name: &str, arguments: &Value) -> Result<Input, String> {
    let catalog = tools();
    let tool = catalog["tools"]
        .as_array()
        .expect("catalog")
        .iter()
        .find(|tool| tool["name"] == name)
        .ok_or_else(|| format!("unknown tool {name:?}"))?;
    let arguments = if arguments.is_null() {
        Map::new()
    } else {
        arguments.as_object().cloned().ok_or_else(||format!("invalid params: validating \"arguments\": unmarshaling arguments: json: cannot unmarshal {} into Go value of type map[string]interface {{}}",match arguments {Value::Number(_)=>"number", _=>crate::stdio::value_type(arguments)}))?
    };
    let schema = &tool["inputSchema"];
    let missing = schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|key| !arguments.contains_key(*key))
        .map(|key| format!("{key:?}"))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!(
            "invalid params: validating \"arguments\": validating root: required: missing properties: [{}]",
            missing.join(" ")
        ));
    }
    let extra = arguments
        .keys()
        .filter(|key| schema["properties"].get(*key).is_none())
        .map(|key| format!("{key:?}"))
        .collect::<Vec<_>>();
    if !extra.is_empty() {
        return Err(format!(
            "invalid params: validating \"arguments\": validating root: unexpected additional properties [{}]",
            extra.join(" ")
        ));
    }
    let mut input = Input::default();
    for (field, value) in arguments {
        if matches!(field.as_str(), "page" | "limit") {
            if value.is_null() {
                continue;
            }
            let number = value
                .as_f64()
                .filter(|number| number.fract() == 0.0)
                .ok_or_else(|| type_error(&field, &value, "one of \"null, integer\""))?;
            let decimal = number.to_string();
            let value=decimal.parse::<i64>().map_err(|_|format!("invalid params: json: cannot unmarshal number {decimal} into Go struct field ListIn.{field} of type int"))?;
            if field == "page" {
                input.page = Some(value);
            } else {
                input.limit = Some(value);
            }
        } else {
            let value = value
                .as_str()
                .ok_or_else(|| type_error(&field, &value, "\"string\""))?
                .to_owned();
            match field.as_str() {
                "creator_id" => input.creator_id = value,
                "post_id" => input.post_id = value,
                "tag" => input.tag = value,
                "kind" => input.kind = value,
                "url" => input.url = value,
                "ref" => input.reference = value,
                "method" => input.method = value,
                _ => unreachable!("validated field"),
            }
        }
    }
    Ok(input)
}
fn type_error(field: &str, value: &Value, want: &str) -> String {
    let text = match value {
        Value::Null => "<invalid reflect.Value>".into(),
        Value::String(value) => value.clone(),
        _ => value.to_string(),
    };
    format!(
        "invalid params: validating \"arguments\": validating root: validating /properties/{field}: type: {text} has type {:?}, want {want}",
        crate::stdio::value_type(value)
    )
}
fn tool_result(mut out: Value, is_error: bool, message: String) -> Value {
    crate::structured_wire_numbers(&mut out);
    let mut result = json!({"content":[{"type":"text","text":message}],"structuredContent":out});
    if is_error {
        result["isError"] = json!(true);
    }
    result
}
fn failure(out: Value, error: impl ToString) -> Value {
    tool_result(out, true, format!("Error: {}", error.to_string()))
}
fn empty_output(name: &str, input: &Input) -> Value {
    match name {
        "fanbox_post" => {
            json!({"id":"","title":"","published_at":"","creator_id":"","is_restricted":false,"assets":[]})
        }
        "fanbox_creator" => json!({"id":"","name":""}),
        "fanbox_creator_tags" => json!({"tags":[]}),
        "fanbox_current_user" => {
            json!({"user_id":0,"display_name":"","creator_id":"","creator_status":"","is_creator":false})
        }
        "fanbox_resolve_url" => json!({"kind":""}),
        "fanbox_open_resource" => json!({"ref":input.reference,"status_code":0}),
        "fanbox_creators" => json!({"creators":[],"pagination":zero_pagination()}),
        _ => json!({"posts":[],"pagination":zero_pagination()}),
    }
}
fn zero_pagination() -> Value {
    json!({"page":0,"limit":null,"returned":0,"has_more":false,"next_page":null})
}
fn add_nonzero(out: &mut Value, key: &str, value: bool) {
    if value {
        out[key] = json!(value);
    }
}
fn add_integer(out: &mut Value, key: &str, value: i64) {
    if value != 0 {
        out[key] = json!(value);
    }
}
fn add_resource(out: &mut Value, key: &str, resource: &Resource) {
    if !resource.reference.is_zero() {
        let mut value = json!({"ref":resource.reference.as_str()});
        if resource.requires_credentials {
            value["requires_credentials"] = json!(true);
        }
        out[key] = value;
    }
}
fn post_output(post: &sdk::Post) -> Value {
    let published = if post.published_at.year() == 1
        && post.published_at.timestamp() == -62135596800
        && post.published_at.timestamp_subsec_nanos() == 0
    {
        String::new()
    } else {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            post.published_at.year(),
            post.published_at.month(),
            post.published_at.day(),
            post.published_at.hour(),
            post.published_at.minute(),
            post.published_at.second()
        )
    };
    let mut out = json!({"id":post.id,"title":post.title,"published_at":published,"creator_id":post.creator_id,"is_restricted":post.is_restricted,"assets":[]});
    add_integer(&mut out, "fee_required", post.fee_required);
    add_nonzero(&mut out, "is_pinned", post.is_pinned);
    add_integer(&mut out, "restricted_for", post.restricted_for);
    add_integer(&mut out, "comment_count", post.comment_count);
    if let Some(body) = &post.body {
        out["assets"] = json!(
            body.assets
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|asset| {
                    let mut out = json!({"id":asset.id,"kind":asset.kind.as_str()});
                    if !asset.name.is_empty() {
                        out["name"] = json!(asset.name);
                    }
                    add_resource(&mut out, "resource", &asset.resource);
                    add_resource(&mut out, "thumbnail", &asset.thumbnail.resource);
                    out
                })
                .collect::<Vec<_>>()
        );
    }
    out
}
#[derive(Clone, Copy)]
struct Plan {
    page: i64,
    limit: i64,
    one_batch: bool,
    skip: i64,
}
impl Plan {
    fn new(page: Option<i64>, limit: Option<i64>) -> Result<Self, String> {
        if page.is_some_and(|page| page <= 0) {
            return Err("page must be a positive integer".into());
        }
        if limit.is_some_and(|limit| limit < 0) {
            return Err("limit must be zero or a positive integer".into());
        }
        if let Some(page) = page {
            let limit = limit
                .filter(|limit| *limit > 0)
                .ok_or("page requires limit to be a positive integer")?;
            let skip = (page - 1)
                .checked_mul(limit)
                .ok_or("page and limit overflow the logical result offset")?;
            return Ok(Self {
                page,
                limit,
                one_batch: false,
                skip,
            });
        }
        Ok(Self {
            page: 1,
            limit: limit.unwrap_or(-1),
            one_batch: limit.is_none(),
            skip: 0,
        })
    }
}
fn pagination(plan: Plan, limit: Option<i64>, returned: usize, has_more: bool) -> Value {
    json!({"page":plan.page,"limit":limit,"returned":returned,"has_more":has_more,"next_page":if has_more && limit.is_some_and(|limit|limit>0){plan.page.checked_add(1)}else{None}})
}
async fn collect<T, F, Fut>(
    context: &Context,
    plan: Plan,
    fetch: F,
) -> Result<(Vec<T>, bool), String>
where
    F: Fn(Cursor) -> Fut,
    Fut: Future<Output = pixiv_sdk::Result<Page<T>>>,
{
    let mut cursor = Cursor::default();
    let mut seen = BTreeSet::new();
    let mut skip = plan.skip;
    let mut items = Vec::new();
    loop {
        if let Some(error) = context.error() {
            return Err(error.to_string());
        }
        if !seen.insert(cursor.as_str().to_owned()) {
            return Err("pagination cursor repeated".into());
        }
        let page = fetch(cursor).await.map_err(|error| error.to_string())?;
        let mut batch = page.items;
        if skip >= batch.len() as i64 {
            skip -= batch.len() as i64;
            batch.clear();
        } else if skip > 0 {
            batch.drain(..skip as usize);
            skip = 0;
        }
        let truncated = plan.limit > 0 && batch.len() as i64 > plan.limit - items.len() as i64;
        if truncated {
            batch.truncate((plan.limit - items.len() as i64) as usize);
        }
        items.extend(batch);
        let more = truncated || !page.next.is_zero();
        if plan.limit > 0 && items.len() as i64 >= plan.limit
            || plan.one_batch && (!items.is_empty() || page.next.is_zero())
            || page.next.is_zero()
        {
            return Ok((items, more));
        }
        cursor = page.next;
    }
}
