use crate::{IllustReference, illust_detail, illust_detail_tool};
use futures_util::{
    StreamExt,
    future::{AbortHandle, Abortable},
    stream::FuturesUnordered,
};
use pixiv_sdk::{Client, transport::Transport};
use serde_json::{Value, json};
use std::{collections::BTreeMap, future::Future, io, pin::Pin};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

type ResponseFuture<'a> = Pin<Box<dyn Future<Output = Value> + 'a>>;
type ToolFuture<'a> = Pin<Box<dyn Future<Output = Value> + 'a>>;

enum ToolInput {
    BookmarkRead(crate::BookmarkRead, crate::BookmarkReadInput),
    BookmarkList(crate::BookmarkList, crate::BookmarkListInput),
    UserRelationship(crate::UserRelationship, crate::UserRelationshipInput),
    UserArtworks(crate::UserArtworksInput),
    UserNovels(crate::UserNovelsInput),
    Mutation(crate::MutationAction, crate::MutationInput),
    Detail(IllustReference),
    Novel(crate::NovelDetailInput),
    NovelSeries(crate::NovelSeriesInput),
    IllustSeries(crate::IllustSeriesInput),
    Related(crate::IllustRelatedInput),
    Recommended(crate::IllustRecommendedInput),
    MixedRecommended(crate::RecommendedInput),
    NovelContent(crate::NovelContentInput),
    User(crate::UserDetailInput),
    UserSearch(crate::SearchUserInput),
    NovelSearch(crate::SearchNovelInput),
    Trending,
    Ranking(Box<crate::IllustRankingInput>),
    Search(Box<crate::SearchIllustInput>),
}

#[derive(Default)]
struct Session {
    initialized: bool,
    log_level: String,
}

pub async fn serve<T: Transport, R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    client: &Client<T>,
    input: R,
    output: &mut W,
) -> io::Result<()> {
    let invoke = |input| -> ToolFuture<'_> {
        Box::pin(async move {
            match input {
                ToolInput::BookmarkRead(kind, input) => {
                    serde_json::to_value(crate::bookmark_read(client, kind, input).await)
                        .expect("tool result is serializable")
                }
                ToolInput::BookmarkList(kind, input) => {
                    serde_json::to_value(crate::bookmark_list(client, kind, input).await)
                        .expect("tool result is serializable")
                }
                ToolInput::UserRelationship(kind, input) => {
                    serde_json::to_value(crate::user_relationship(client, kind, input).await)
                        .expect("tool result is serializable")
                }
                ToolInput::Mutation(action, input) => {
                    serde_json::to_value(crate::mutate(client, action, input).await)
                        .expect("tool result is serializable")
                }
                ToolInput::Detail(input) => {
                    serde_json::to_value(illust_detail(client, input).await)
                        .expect("tool result is serializable")
                }
                ToolInput::UserArtworks(input) => {
                    serde_json::to_value(crate::user_artworks(client, input).await)
                        .expect("tool result is serializable")
                }
                ToolInput::UserNovels(input) => {
                    serde_json::to_value(crate::user_novels(client, input).await)
                        .expect("tool result is serializable")
                }
                ToolInput::User(input) => {
                    serde_json::to_value(crate::user_detail(client, input).await)
                        .expect("tool result is serializable")
                }
                ToolInput::Related(input) => {
                    serde_json::to_value(crate::illust_related(client, input).await)
                        .expect("tool result is serializable")
                }
                ToolInput::MixedRecommended(input) => {
                    serde_json::to_value(crate::recommended(client, input).await)
                        .expect("tool result is serializable")
                }
                ToolInput::Recommended(input) => {
                    serde_json::to_value(crate::illust_recommended(client, input).await)
                        .expect("tool result is serializable")
                }
                ToolInput::IllustSeries(input) => {
                    serde_json::to_value(crate::illust_series(client, input).await)
                        .expect("tool result is serializable")
                }
                ToolInput::NovelSeries(input) => {
                    serde_json::to_value(crate::novel_series(client, input).await)
                        .expect("tool result is serializable")
                }
                ToolInput::NovelContent(input) => {
                    serde_json::to_value(crate::novel_content(client, input).await)
                        .expect("tool result is serializable")
                }
                ToolInput::Novel(input) => {
                    serde_json::to_value(crate::novel_detail(client, input).await)
                        .expect("tool result is serializable")
                }
                ToolInput::UserSearch(input) => {
                    serde_json::to_value(crate::search_user(client, input).await)
                        .expect("tool result is serializable")
                }
                ToolInput::NovelSearch(input) => {
                    serde_json::to_value(crate::search_novel(client, input).await)
                        .expect("tool result is serializable")
                }
                ToolInput::Trending => {
                    serde_json::to_value(crate::trending_tags_illust(client).await)
                        .expect("tool result is serializable")
                }
                ToolInput::Ranking(input) => {
                    serde_json::to_value(crate::illust_ranking(client, *input).await)
                        .expect("tool result is serializable")
                }
                ToolInput::Search(input) => {
                    serde_json::to_value(crate::search_illust(client, *input).await)
                        .expect("tool result is serializable")
                }
            }
        })
    };
    serve_with(&invoke, input, output).await
}

pub async fn serve_saved<T: Transport + 'static, R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    execution: &pixiv_app::execution::Execution<T>,
    input: R,
    output: &mut W,
) -> io::Result<()> {
    serve_saved_with_proxy(execution, None, input, output).await
}

pub async fn serve_saved_with_proxy<
    T: Transport + 'static,
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
>(
    execution: &pixiv_app::execution::Execution<T>,
    proxy: Option<&str>,
    input: R,
    output: &mut W,
) -> io::Result<()> {
    let invoke = |input| -> ToolFuture<'_> {
        Box::pin(async move {
            let context = pixiv_app::lifecycle::Context::new();
            match input {
                ToolInput::BookmarkRead(kind, input) => serde_json::to_value(
                    crate::bookmark_reads::saved_read(execution, &context, kind, input, proxy)
                        .await,
                )
                .expect("tool result is serializable"),
                ToolInput::BookmarkList(kind, input) => serde_json::to_value(
                    crate::bookmark_lists::saved_list(execution, &context, kind, input, proxy)
                        .await,
                )
                .expect("tool result is serializable"),
                ToolInput::UserRelationship(kind, input) => serde_json::to_value(
                    crate::user_relationships::saved_relationship(
                        execution, &context, kind, input, proxy,
                    )
                    .await,
                )
                .expect("tool result is serializable"),
                ToolInput::Mutation(action, input) => serde_json::to_value(
                    crate::mutation::saved_mutate(execution, &context, action, input, proxy).await,
                )
                .expect("tool result is serializable"),
                ToolInput::Detail(input) => serde_json::to_value(
                    crate::saved_illust_detail_with_proxy(execution, &context, input, proxy).await,
                )
                .expect("tool result is serializable"),
                ToolInput::UserArtworks(input) => serde_json::to_value(
                    crate::user_works::saved_artworks(execution, &context, input, proxy).await,
                )
                .expect("tool result is serializable"),
                ToolInput::UserNovels(input) => serde_json::to_value(
                    crate::user_works::saved_novels(execution, &context, input, proxy).await,
                )
                .expect("tool result is serializable"),
                ToolInput::User(input) => serde_json::to_value(
                    crate::user::saved_user_detail(execution, &context, input, proxy).await,
                )
                .expect("tool result is serializable"),
                ToolInput::Related(input) => serde_json::to_value(
                    crate::artwork_feed::saved_related(execution, &context, input, proxy).await,
                )
                .expect("tool result is serializable"),
                ToolInput::MixedRecommended(input) => serde_json::to_value(
                    crate::recommended::saved_recommended(execution, &context, input, proxy).await,
                )
                .expect("tool result is serializable"),
                ToolInput::Recommended(input) => serde_json::to_value(
                    crate::artwork_feed::saved_recommended(execution, &context, input, proxy).await,
                )
                .expect("tool result is serializable"),
                ToolInput::IllustSeries(input) => serde_json::to_value(
                    crate::illust_series::saved_illust_series(execution, &context, input, proxy)
                        .await,
                )
                .expect("tool result is serializable"),
                ToolInput::NovelSeries(input) => serde_json::to_value(
                    crate::novel_series::saved_novel_series(execution, &context, input, proxy)
                        .await,
                )
                .expect("tool result is serializable"),
                ToolInput::NovelContent(input) => serde_json::to_value(
                    crate::novel_content::saved_novel_content(execution, &context, input, proxy)
                        .await,
                )
                .expect("tool result is serializable"),
                ToolInput::Novel(input) => serde_json::to_value(
                    crate::novel::saved_novel_detail(execution, &context, input, proxy).await,
                )
                .expect("tool result is serializable"),
                ToolInput::UserSearch(input) => serde_json::to_value(
                    crate::user_search::saved_search_user(execution, &context, input, proxy).await,
                )
                .expect("tool result is serializable"),
                ToolInput::NovelSearch(input) => serde_json::to_value(
                    crate::novel_search::saved_search_novel(execution, &context, input, proxy)
                        .await,
                )
                .expect("tool result is serializable"),
                ToolInput::Trending => serde_json::to_value(
                    crate::trending::saved_trending_tags_illust(execution, &context, proxy).await,
                )
                .expect("tool result is serializable"),
                ToolInput::Ranking(input) => serde_json::to_value(
                    crate::ranking::saved_illust_ranking(execution, &context, *input, proxy).await,
                )
                .expect("tool result is serializable"),
                ToolInput::Search(input) => serde_json::to_value(
                    crate::search::saved_search_illust(execution, &context, *input, proxy).await,
                )
                .expect("tool result is serializable"),
            }
        })
    };
    serve_with(&invoke, input, output).await
}

async fn serve_with<'a, R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    invoke: &impl Fn(ToolInput) -> ToolFuture<'a>,
    input: R,
    output: &mut W,
) -> io::Result<()> {
    let mut lines = BufReader::new(input).lines();
    let mut session = Session::default();
    let mut pending: FuturesUnordered<ResponseFuture<'_>> = FuturesUnordered::new();
    let mut eof = false;
    let mut inflight = BTreeMap::new();
    loop {
        tokio::select! {
                   line = lines.next_line(), if !eof => {
                       let Some(line) = line? else { eof = true; continue; };
                       if line.trim().is_empty() { continue; }
                       let message: Value = serde_json::from_str(&line).map_err(|error| io::Error::new(io::ErrorKind::InvalidData,error))?;
                       if message.get("id").is_none() {
                           if message["method"] == "notifications/cancelled"
                               && let Some(id) = message["params"].get("requestId")
                               && let Some(handle) = inflight.get(&id.to_string())
                           {
                               AbortHandle::abort(handle);
                           }
                           continue;
                       }
                       let id = message["id"].clone();
                       let method = message["method"].as_str().unwrap_or_default();
                       let params = message.get("params");
                       let response = match method {
                           "initialize" => {
                               if params.is_none() || params.is_some_and(Value::is_null) {
                                   write_response(output,protocol_error(id,-32600,"invalid request: missing required \"params\"".into())).await?;
                                   continue;
                               }
                               session.initialized = true;
                               let version = params.and_then(|params| params["protocolVersion"].as_str()).unwrap_or_default();
                               let version = match version { "2025-06-18" | "2025-03-26" | "2024-11-05" => version, _ => "2025-06-18" };
                               success(id, json!({"protocolVersion":version,"capabilities":{"logging":{},"tools":{"listChanged":true}},"instructions":"Pixiv MCP server for searching, browsing, and downloading Pixiv content.","serverInfo":{"name":"pixiv-cli","version":"3.0.0"}}))
                           }
                           "ping" => success(id, json!({})),
                           _ if !session.initialized => protocol_error(id,0,format!("method {method:?} is invalid during session initialization")),
                           "tools/list" => success(id,json!({"tools":[illust_detail_tool(),crate::search_illust_tool(),crate::trending_tags_illust_tool(),crate::illust_ranking_tool(),crate::novel_detail_tool(),crate::search_novel_tool(),crate::user_detail_tool(),crate::search_user_tool(),crate::mutation_tool(crate::MutationAction::AddBookmark),crate::mutation_tool(crate::MutationAction::RemoveBookmark),crate::mutation_tool(crate::MutationAction::AddNovelBookmark),crate::mutation_tool(crate::MutationAction::RemoveNovelBookmark),crate::mutation_tool(crate::MutationAction::FollowUser),crate::mutation_tool(crate::MutationAction::UnfollowUser),crate::novel_series_tool(),crate::novel_content_tool(),crate::illust_series_tool(),crate::illust_related_tool(),crate::illust_recommended_tool(),crate::recommended_tool(),crate::user_artworks_tool(),crate::user_novels_tool(),crate::user_relationship_tool(crate::UserRelationship::Following),crate::user_relationship_tool(crate::UserRelationship::Followers),crate::user_relationship_tool(crate::UserRelationship::Related),crate::user_relationship_tool(crate::UserRelationship::Blocked),crate::bookmark_list_tool(crate::BookmarkList::Artwork),crate::bookmark_list_tool(crate::BookmarkList::Novel),crate::bookmark_list_tool(crate::BookmarkList::All),crate::bookmark_read_tool(crate::BookmarkRead::ArtworkDetail),crate::bookmark_read_tool(crate::BookmarkRead::NovelDetail),crate::bookmark_read_tool(crate::BookmarkRead::ArtworkTags),crate::bookmark_read_tool(crate::BookmarkRead::NovelTags),crate::bookmark_read_tool(crate::BookmarkRead::AllTags)]})),
                           "logging/setLevel" => {
                               if params.is_none() || params.is_some_and(Value::is_null) {
                                   protocol_error(id,-32600,"invalid request: missing required \"params\"".into())
                               } else {
                                   session.log_level = params.and_then(|params|params["level"].as_str()).unwrap_or_default().into();
                                   success(id,json!({}))
                               }
                           }
                           "tools/call" => {
                               if let Some(params) = params.filter(|value| !value.is_null()) {
                                   let name = params["name"].as_str().unwrap_or_default();
                                   if crate::BookmarkRead::from_name(name).is_none() && crate::BookmarkList::from_name(name).is_none() && crate::UserRelationship::from_name(name).is_none() && crate::MutationAction::from_name(name).is_none() && !matches!(name, "user_artworks" | "user_novels" | "recommended" | "illust_related" | "illust_recommended" | "illust_detail" | "search_illust" | "trending_tags_illust" | "illust_ranking" | "novel_detail" | "illust_series" | "novel_series" | "novel_content" | "search_novel" | "user_detail" | "search_user") {
                                       protocol_error(id,-32602,format!("unknown tool {name:?}"))
                                   } else {
                                       let input = if let Some(kind)=crate::BookmarkRead::from_name(name) {crate::bookmark_reads::decode(kind,params.get("arguments")).map(|input|ToolInput::BookmarkRead(kind,input))} else if let Some(kind)=crate::BookmarkList::from_name(name) {crate::bookmark_lists::decode(kind,params.get("arguments")).map(|input|ToolInput::BookmarkList(kind,input))} else if let Some(kind)=crate::UserRelationship::from_name(name) {crate::user_relationships::decode(kind,params.get("arguments")).map(|input|ToolInput::UserRelationship(kind,input))} else if let Some(action) = crate::MutationAction::from_name(name) {crate::mutation::decode(action,params.get("arguments")).map(|input|ToolInput::Mutation(action,input))} else if name=="user_artworks" {crate::user_works::decode_artworks(params.get("arguments")).map(ToolInput::UserArtworks)} else if name=="user_novels" {crate::user_works::decode_novels(params.get("arguments")).map(ToolInput::UserNovels)} else if name=="recommended" {crate::recommended::decode(params.get("arguments")).map(ToolInput::MixedRecommended)} else if name=="illust_related" {crate::artwork_feed::decode_related(params.get("arguments")).map(ToolInput::Related)} else if name=="illust_recommended" {crate::artwork_feed::decode_recommended(params.get("arguments")).map(ToolInput::Recommended)} else if name=="search_user" {crate::user_search::decode(params.get("arguments")).map(ToolInput::UserSearch)} else if name=="user_detail" {crate::user::decode(params.get("arguments")).map(ToolInput::User)} else if name=="search_novel" {crate::novel_search::decode(params.get("arguments")).map(ToolInput::NovelSearch)} else if name == "illust_series" {crate::illust_series::decode(params.get("arguments")).map(ToolInput::IllustSeries)} else if name == "novel_series" {crate::novel_series::decode(params.get("arguments")).map(ToolInput::NovelSeries)} else if name == "novel_content" {crate::novel_content::decode(params.get("arguments")).map(ToolInput::NovelContent)} else if name == "novel_detail" { crate::novel::decode(params.get("arguments")).map(ToolInput::Novel) } else if name == "search_illust" {
                                           crate::search::decode(params.get("arguments")).map(|input| ToolInput::Search(Box::new(input)))
                                       } else if name == "illust_ranking" { crate::ranking::decode(params.get("arguments")).map(|input|ToolInput::Ranking(Box::new(input))) } else if name == "trending_tags_illust" { crate::trending::decode(params.get("arguments")).map(|()|ToolInput::Trending) } else { decode_reference(params.get("arguments")).map(ToolInput::Detail) };
                                       match input {
                                           Ok(input) => {
                                               let (handle, registration) = AbortHandle::new_pair();
                                               inflight.insert(id.to_string(), handle);
                                               pending.push(Box::pin(async move {
                                                   let mutation = match &input {ToolInput::Mutation(action,input)=>Some((*action,input.clone())),_=>None};
                                                   let bookmark_read=if let ToolInput::BookmarkRead(kind,_) = &input {Some(*kind)} else {None};
                                                   let bookmark_list=if let ToolInput::BookmarkList(kind,_) = &input {Some(*kind)} else {None};
                                                   let is_user_artworks=matches!(&input,ToolInput::UserArtworks(_));
        let relationship=if let ToolInput::UserRelationship(kind,_) = &input {Some(*kind)}else{None};
        let is_user_novels=matches!(&input,ToolInput::UserNovels(_));
        let is_related=matches!(&input,ToolInput::Related(_));
        let is_mixed_recommended=matches!(&input,ToolInput::MixedRecommended(_));
        let is_recommended=matches!(&input,ToolInput::Recommended(_));
        let is_search = matches!(&input, ToolInput::Search(_));
                                                   let is_ranking = matches!(&input, ToolInput::Ranking(_));
                                                   let is_trending = matches!(&input, ToolInput::Trending);
                                                   let is_illust_series=matches!(&input,ToolInput::IllustSeries(_));
                                                   let is_novel_series=matches!(&input,ToolInput::NovelSeries(_));
                                                   let is_novel_content=matches!(&input,ToolInput::NovelContent(_));
                                                   let is_novel = matches!(&input, ToolInput::Novel(_));
                                                   let is_user=matches!(&input,ToolInput::User(_));
                                                   let is_novel_search=matches!(&input,ToolInput::NovelSearch(_));
                                                   let is_user_search=matches!(&input,ToolInput::UserSearch(_));
                                                   let result = match Abortable::new(invoke(input), registration).await {
                                                       Ok(result) => result,
                                                       Err(_) => {
                                                           if let Some((action,input))=mutation {let message=pixiv_sdk::Error::new(pixiv_sdk::Reason::UpstreamUnavailable,action.operation()).with_detail("pixiv upstream transport failed").to_string();serde_json::to_value(crate::mutation::result_for(action,&input,Err(message))).expect("tool result is serializable")} else {
                                                           let message = pixiv_sdk::Error::new(pixiv_sdk::Reason::UpstreamUnavailable,if let Some(kind)=bookmark_read {kind.operation()} else if let Some(kind)=bookmark_list {kind.operation()} else if let Some(kind)=relationship {kind.operation()} else if is_user_artworks { "UserArtworks" } else if is_user_novels { "UserNovels" } else if is_mixed_recommended { "Recommended" } else if is_related { "RelatedArtworks" } else if is_recommended { "RecommendedArtworks" } else if is_search { "SearchArtworks" } else if is_ranking { "ArtworkRanking" } else if is_trending { "TrendingArtworkTags" } else if is_illust_series { "ArtworkSeries" } else if is_novel_series { "NovelSeries" } else if is_novel_content { "NovelContent" } else if is_novel { "Novel" } else if is_novel_search { "SearchNovels" } else if is_user_search { "SearchUsers" } else if is_user { "User" } else { "Artwork" }).with_detail("pixiv upstream transport failed").to_string();
                                                           if let Some(kind)=bookmark_read {serde_json::to_value(crate::bookmark_reads::failure(kind,message))} else if is_mixed_recommended {serde_json::to_value(crate::recommended::failure(message))} else if is_novel_series {serde_json::to_value(crate::novel_series::failure(message))} else if is_novel_content {serde_json::to_value(crate::novel_content::failure(message))} else if bookmark_list.is_some() || relationship.is_some() || is_user_artworks || is_user_novels || is_related || is_recommended || is_search || is_ranking || is_illust_series || is_novel_search || is_user_search { serde_json::to_value(crate::search::failure(message)) } else if is_trending { serde_json::to_value(crate::trending::failure(message)) } else { serde_json::to_value(crate::failure(message)) }.expect("tool result is serializable")}
                                                       },
                                                   };
                                                   if bookmark_read==Some(crate::BookmarkRead::ArtworkDetail) && result["structuredContent"]["tags"].is_null() { protocol_error(id,0,"validating tool output: validating root: validating /properties/tags: type: <invalid reflect.Value> has type \"null\", want \"array\"".into()) } else {success(id,result)}
                                               }));
                                               continue;
                                           }
                                           Err(message) => protocol_error(id,-32602,message),
                                       }
                                   }
                               } else {
                                   protocol_error(id,-32600,"invalid request: missing required \"params\"".into())
                               }
                           }
                           _ => protocol_error(id,0,format!("JSON RPC not handled: {method:?} unsupported")),
                       };
                       write_response(output,response).await?;
                   }
                   Some(response) = pending.next(), if !pending.is_empty() => {
                       inflight.remove(&response["id"].to_string());
                       write_response(output,response).await?;
                   }
                   else => break,
               }
    }
    output.flush().await
}

async fn write_response<W: AsyncWrite + Unpin>(output: &mut W, response: Value) -> io::Result<()> {
    let mut encoded = serde_json::to_vec(&response)?;
    encoded.push(b'\n');
    output.write_all(&encoded).await?;
    output.flush().await
}
fn success(id: Value, result: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":result})
}
fn protocol_error(id: Value, code: i32, message: String) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}

fn decode_reference(arguments: Option<&Value>) -> Result<IllustReference, String> {
    let arguments = arguments.unwrap_or(&Value::Null);
    if arguments.is_null() {
        return Ok(IllustReference::default());
    }
    let Some(arguments) = arguments.as_object() else {
        return Err(format!(
            "invalid params: validating \"arguments\": unmarshaling arguments: json: cannot unmarshal {} into Go value of type map[string]interface {{}}",
            value_type(arguments)
        ));
    };
    let extra = arguments
        .keys()
        .filter(|key| !matches!(key.as_str(), "illust_id" | "url"))
        .cloned()
        .collect::<Vec<_>>();
    if !extra.is_empty() {
        let extra = extra
            .iter()
            .map(|key| format!("{key:?}"))
            .collect::<Vec<_>>()
            .join(" ");
        return Err(format!(
            "invalid params: validating \"arguments\": validating root: unexpected additional properties [{extra}]"
        ));
    }
    let mut input = IllustReference::default();
    if let Some(value) = arguments.get("illust_id") {
        if value.as_f64().is_none_or(|number| number.fract() != 0.0) {
            return Err(type_error("illust_id", value, "integer"));
        }
        let decimal = value
            .as_f64()
            .expect("integer argument fits float64")
            .to_string();
        input.illust_id=decimal.parse().map_err(|_|format!("invalid params: json: cannot unmarshal number {decimal} into Go struct field illustReferenceIn.illust_id of type int64"))?;
    }
    if let Some(value) = arguments.get("url") {
        input.url = value
            .as_str()
            .ok_or_else(|| type_error("url", value, "string"))?
            .into();
    }
    Ok(input)
}
pub(crate) fn value_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
        Value::Number(number) if number.as_f64().is_some_and(|number| number.fract() == 0.0) => {
            "integer"
        }
        Value::Number(_) => "number",
    }
}
fn type_error(field: &str, value: &Value, expected: &str) -> String {
    let display = match value {
        Value::Null => "<invalid reflect.Value>".into(),
        Value::String(value) => value.clone(),
        _ => value.to_string(),
    };
    format!(
        "invalid params: validating \"arguments\": validating root: validating /properties/{field}: type: {display} has type {:?}, want {expected:?}",
        value_type(value)
    )
}
