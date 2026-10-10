#[allow(dead_code)]
mod fanbox_solver_support;
#[allow(dead_code)]
mod fanbox_support;
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use fanbox_solver_support::{Control, error, expected_control, expected_error};
use fanbox_support::{FixtureTransport, SESSION, context_for};
use pixiv_sdk::fanbox::transport::{
    ExternalError, RawRequest, RawResponse, RawTransport, TransportFuture,
};
use pixiv_sdk::{
    context::RequestContext, cursor::Cursor, dto::ResourceDto, fanbox::*, resource::Resource,
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, atomic::Ordering};
struct RecordingTransport {
    inner: Arc<FixtureTransport>,
    control: Option<Arc<Mutex<Vec<Value>>>>,
    trace: Mutex<Vec<String>>,
    controls: Mutex<usize>,
}
impl RecordingTransport {
    fn flush(&self) {
        if let Some(control) = &self.control {
            let requests = control.lock().unwrap();
            let mut count = self.controls.lock().unwrap();
            for request in &requests[*count..] {
                self.trace.lock().unwrap().push(format!(
                    "solver_control:{} {}",
                    request["method"].as_str().unwrap(),
                    request["path"].as_str().unwrap()
                ));
            }
            *count = requests.len();
        }
    }
}
impl RawTransport for RecordingTransport {
    fn send(
        &self,
        request: RawRequest,
    ) -> TransportFuture<'_, std::result::Result<Option<RawResponse>, ExternalError>> {
        Box::pin(async move {
            self.flush();
            self.trace
                .lock()
                .unwrap()
                .push(format!("api_or_identity:{}", request.url));
            self.inner.send(request).await
        })
    }
    fn close_idle_connections(&self) {
        self.inner.close_idle_connections();
    }
}
fn text(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or("").into()
}
fn resource(value: &Resource) -> Value {
    json!({"ref":value.reference.as_str(),"payload":if value.reference.is_zero(){String::new()}else{String::from_utf8(value.reference.payload().unwrap()).unwrap()},"url":value.url,"request_headers":if value.reference.is_zero(){Value::Null}else{json!(value.request_headers)},"requires_credentials":value.requires_credentials,"expires_at":value.expires_at,"dto":ResourceDto::from_resource(value)})
}
fn post(value: &Post) -> Value {
    let mut out = json!({"dto":value.to_dto(),"runtime_cover":resource(&value.cover.resource),"runtime_body_nil":value.body.is_none()});
    if let Some(body) = &value.body {
        out["runtime_assets"]=json!(body.assets.as_deref().unwrap_or_default().iter().map(|v|json!({"resource":resource(&v.resource),"thumbnail":resource(&v.thumbnail.resource)})).collect::<Vec<_>>());
        out["runtime_blocks"] = json!(
            body.blocks
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|v| {
                    let mut out = json!({});
                    if let Some(image) = &v.image {
                        out["image"] = resource(&image.resource);
                    }
                    if let Some(file) = &v.file {
                        out["file"] = resource(&file.resource);
                    }
                    out
                })
                .collect::<Vec<_>>()
        );
        out["runtime_assets_nil"] = json!(body.assets.is_none());
        out["runtime_blocks_nil"] = json!(body.blocks.is_none());
    }
    out
}
fn cursor(cur: &Cursor) -> Value {
    if cur.is_zero() {
        return json!({"text":"","envelope":null,"payload":"","identity":"","identity_present":false});
    }
    json!({"text":cur.as_str(),"envelope":serde_json::from_slice::<Value>(&URL_SAFE_NO_PAD.decode(cur.as_str()).unwrap()).unwrap(),"payload":String::from_utf8(cur.payload().unwrap()).unwrap(),"identity":cur.identity().unwrap_or_default(),"identity_present":cur.identity().is_some()})
}
fn mutate(cur: Cursor, kind: &str) -> pixiv_sdk::Result<Cursor> {
    if kind.is_empty() {
        return Ok(cur);
    }
    let mut env: Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(cur.as_str()).unwrap()).unwrap();
    match kind {
        "product" => env["p"] = json!("pixiv"),
        "operation" => env["o"] = json!("Other"),
        "binding" => env["b"] = json!(2),
        "format" => env["v"] = json!(2),
        "query" => env["q"] = json!("wrong-query"),
        "identity" => env["id"] = json!("99"),
        "no_identity" => {
            env.as_object_mut().unwrap().remove("id");
        }
        "empty_identity" => env["id"] = json!(""),
        "empty_payload" => env["pl"] = json!(""),
        "unknown_fields" => {
            env["unknown"] = json!("ignored");
            env["e"] = json!(true);
            env["i"] = json!("non-secret-instance");
        }
        kind => {
            env["pl"] = json!(STANDARD.encode(match kind {
                "bad_payload" => "{",
                "empty_url" => r#"{"u":""}"#,
                "null_payload" => "null",
                "wrong_url_type" => r#"{"u":1}"#,
                "relative_url" => r#"{"u":"/next"}"#,
                "unsafe_url" => r#"{"u":"https://evil.example/next"}"#,
                _ => panic!("unknown mutation {kind}"),
            }))
        }
    }
    Cursor::parse(&URL_SAFE_NO_PAD.encode(serde_json::to_vec(&env).unwrap()))
}
fn dto_conversion() -> Value {
    let stamp = chrono::DateTime::parse_from_rfc3339("2026-01-02T03:04:05.006Z")
        .unwrap()
        .to_utc();
    let resource = Resource {
        reference: pixiv_sdk::resource::ResourceRef::new(
            "fanbox",
            br#"{"k":"post_image","c":"dto-creator","p":"dto-post","a":"dto-asset"}"#,
        )
        .unwrap(),
        url: "https://downloads.fanbox.cc/dto-private".into(),
        request_headers: std::collections::BTreeMap::from([(
            "Referer".into(),
            "https://www.fanbox.cc/".into(),
        )]),
        expires_at: Some(stamp),
        requires_credentials: true,
    };
    let image = ImageResource {
        resource: resource.clone(),
        variant: "original".into(),
        width: 17,
        height: 23,
    };
    let block = PostBlock {
        kind: PostBlockKind::Unknown,
        image: Some(PostImageBlock {
            resource: resource.clone(),
            caption: "image caption".into(),
        }),
        file: Some(PostFileBlock {
            resource: resource.clone(),
            name: "file name".into(),
            caption: "file caption".into(),
        }),
        article: Some(PostArticleBlock {
            text: "article".into(),
        }),
        video: Some(PostVideoEmbed {
            provider: "provider".into(),
            content_id: "content".into(),
            canonical_url: "https://example.invalid/video".into(),
            title: "title".into(),
            thumbnail_url: "https://example.invalid/thumb".into(),
            video_id: "video".into(),
            embedded_data: std::collections::BTreeMap::from([("key".into(), "original".into())]),
        }),
        unknown: Some(PostUnknownBlock {
            raw_type: "raw".into(),
            payload: std::collections::BTreeMap::from([("key".into(), "original".into())]),
        }),
    };
    let mut body = PostBody {
        text: "body".into(),
        blocks: Some(vec![block]),
        assets: Some(vec![Asset {
            id: "asset".into(),
            kind: AssetKind::Image,
            name: "name".into(),
            resource: resource.clone(),
            thumbnail: image.clone(),
        }]),
    };
    let dto = body.to_dto();
    body.blocks.as_mut().unwrap()[0]
        .video
        .as_mut()
        .unwrap()
        .embedded_data
        .insert("key".into(), "mutated".into());
    body.blocks.as_mut().unwrap()[0]
        .unknown
        .as_mut()
        .unwrap()
        .payload
        .insert("key".into(), "mutated".into());
    body.assets.as_mut().unwrap()[0].name = "mutated".into();
    let creator = Creator {
        id: "creator".into(),
        name: "name".into(),
        icon: image.clone(),
        has_adult_content: true,
        is_following: true,
        cover: image,
        plan_fee: 500,
        has_supporting_plan: true,
    };
    json!({"body":dto,"empty_body":PostBody::default().to_dto(),"nil_video_map":PostVideoEmbed::default().to_dto(),"nil_unknown_map":PostUnknownBlock::default().to_dto(),"creator":creator.to_dto(),"summary":creator.summary().to_dto(),"file_resource":FileResource{resource,name:"name".into()}.to_dto(),"nil_variants":PostBlock{kind:PostBlockKind::Unknown,..Default::default()}.to_dto(),"tag":CreatorTag{name:"tag".into(),url:"https://example.invalid/tag".into()}.to_dto(),"maps_independent":dto.blocks[0].video.as_ref().unwrap().embedded_data["key"]=="original" && dto.blocks[0].unknown.as_ref().unwrap().payload["key"]=="original","assets_independent":dto.assets[0].name=="name"})
}
#[tokio::test]
async fn public_content_reads_match_all_416_frozen_go_rows() {
    let rows: Value =
        serde_json::from_str(include_str!("fixtures/fanbox-content-reads.json")).unwrap();
    assert_eq!(rows["cases"].as_array().unwrap().len(), 416);
    for row in rows["cases"].as_array().unwrap() {
        let name = row["name"].as_str().unwrap();
        let input = &row["input"];
        let expected = &row["observation"];
        let base = context_for(&json!({}));
        let transport = Arc::new(FixtureTransport::new(
            input["steps"].as_array().unwrap().clone(),
            base.clone(),
        ));
        let mut control = if input["solver_steps"]
            .as_array()
            .is_some_and(|v| !v.is_empty())
        {
            Some(Control::new(&json!({"control_steps":input["solver_steps"],"mode":""})).await)
        } else {
            None
        };
        let recording = Arc::new(RecordingTransport {
            inner: transport.clone(),
            control: control.as_ref().map(|c| c.requests.clone()),
            trace: Mutex::new(vec![]),
            controls: Mutex::new(0),
        });
        let options = Options {
            http_client: Some(recording.clone()),
            flare_solverr: control.as_ref().map(|c| FlareSolverrOptions {
                url: format!("{}/", c.url),
                proxy_url: text(input, "solver_proxy"),
            }),
            ..Default::default()
        };
        let open = || {
            Client::open_with(
                SessionCredentials {
                    fanbox_sessid: SESSION.into(),
                },
                options.clone(),
            )
            .unwrap()
        };
        let mut clients = vec![open()];
        let mut previous = Cursor::default();
        let mut outcomes = vec![];
        for call in input["calls"].as_array().unwrap() {
            if call["fresh_client"] == true {
                clients.push(open());
            }
            let client = clients.last().unwrap();
            let context: Arc<dyn RequestContext> =
                if call["context"].as_str().is_some_and(|s| !s.is_empty()) {
                    Arc::new(context_for(call))
                } else {
                    Arc::new(base.clone())
                };
            let operation = call["operation"].as_str().unwrap();
            let cur = if call["continue"] == true {
                previous.clone()
            } else {
                Cursor::default()
            };
            let parsed = if text(call, "cursor").is_empty() {
                Ok(cur)
            } else {
                Cursor::parse(&text(call, "cursor"))
            }
            .and_then(|cur| mutate(cur, &text(call, "mutation")));
            let mut result = json!({"operation":operation});
            let mut failure = None;
            match parsed {
                Err(e) => {
                    result["cursor_parse_error"] = error(Some(&e));
                    failure = Some(e);
                }
                Ok(cur) => match operation {
                    "DTO" => result["conversion"] = dto_conversion(),
                    "Creator" => {
                        let v = client
                            .creator(
                                context,
                                CreatorRequest {
                                    creator_id: text(call, "creator_id"),
                                },
                            )
                            .await;
                        failure = v.as_ref().err().cloned();
                        let v = v.unwrap_or_default();
                        result["dto"] = json!(v.to_dto());
                        result["runtime_icon"] = resource(&v.icon.resource);
                        result["runtime_cover"] = resource(&v.cover.resource);
                    }
                    "Creators" => {
                        let v = client
                            .creators(
                                context,
                                CreatorsRequest {
                                    kind: text(call, "kind"),
                                    cursor: cur,
                                },
                            )
                            .await;
                        result["runtime_items_nil"] = json!(v.is_err());
                        failure = v.as_ref().err().cloned();
                        let v = v.unwrap_or_default();
                        result["items"] = json!(
                            v.items
                                .iter()
                                .map(CreatorSummary::to_dto)
                                .collect::<Vec<_>>()
                        );
                        previous = v.next;
                        result["next"] = cursor(&previous);
                    }
                    "CreatorTags" => {
                        let v = client
                            .creator_tags(
                                context,
                                CreatorTagsRequest {
                                    creator_id: text(call, "creator_id"),
                                },
                            )
                            .await;
                        result["runtime_items_nil"] = json!(v.is_err());
                        failure = v.as_ref().err().cloned();
                        result["items"] = if let Ok(v) = v {
                            json!(v.iter().map(CreatorTag::to_dto).collect::<Vec<_>>())
                        } else {
                            Value::Null
                        };
                    }
                    "Post" => {
                        let v = client
                            .post(
                                context,
                                PostRequest {
                                    post_id: text(call, "post_id"),
                                },
                            )
                            .await;
                        failure = v.as_ref().err().cloned();
                        result["post"] = post(&v.unwrap_or_default());
                    }
                    "ResolveURL" => {
                        let v = client.resolve_url(
                            context,
                            ResolveURLRequest {
                                raw_url: text(call, "raw_url"),
                            },
                        );
                        failure = v.as_ref().err().cloned();
                        result["reference"] = json!(v.unwrap_or_default());
                    }
                    "CreatorPosts" | "TaggedPosts" | "Home" | "Supporting" => {
                        let v = match operation {
                            "CreatorPosts" => {
                                client
                                    .creator_posts(
                                        context,
                                        CreatorPostsRequest {
                                            creator_id: text(call, "creator_id"),
                                            cursor: cur,
                                        },
                                    )
                                    .await
                            }
                            "TaggedPosts" => {
                                client
                                    .tagged_posts(
                                        context,
                                        TaggedPostsRequest {
                                            creator_id: text(call, "creator_id"),
                                            tag: text(call, "tag"),
                                            cursor: cur,
                                        },
                                    )
                                    .await
                            }
                            "Home" => client.home(context, HomeRequest { cursor: cur }).await,
                            _ => {
                                client
                                    .supporting(context, SupportingRequest { cursor: cur })
                                    .await
                            }
                        };
                        result["runtime_items_nil"] = json!(v.is_err());
                        failure = v.as_ref().err().cloned();
                        let v = v.unwrap_or_default();
                        result["items"] = json!(v.items.iter().map(post).collect::<Vec<_>>());
                        previous = v.next;
                        result["next"] = cursor(&previous);
                    }
                    _ => panic!("{operation}"),
                },
            }
            result["error"] = error(failure.as_ref());
            result["requests_completed"] = json!(transport.requests.lock().unwrap().len());
            outcomes.push(result);
        }
        let expected_outcomes: Vec<_> = expected["outcomes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| {
                let mut v = v.clone();
                v["error"] = expected_error(&v["error"]);
                if v.get("cursor_parse_error").is_some() {
                    v["cursor_parse_error"] = expected_error(&v["cursor_parse_error"]);
                }
                v
            })
            .collect();
        recording.flush();
        assert_eq!(
            *recording.trace.lock().unwrap(),
            expected["request_order"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_owned())
                .collect::<Vec<_>>(),
            "{name} request order"
        );
        assert_eq!(outcomes, expected_outcomes, "{name} outcomes");
        assert_eq!(
            *transport.requests.lock().unwrap(),
            expected["requests"].as_array().unwrap().clone(),
            "{name} requests"
        );
        let bodies=Value::Array(expected["bodies"].as_array().unwrap().iter().map(|v|json!({"injected_body":v["injected_body"],"bytes_read":v["bytes_read"],"close_calls":v["close_calls"]})).collect());
        assert_eq!(transport.body_projection(), bodies, "{name} bodies");
        for client in clients {
            client.close_idle_connections();
            client.close_idle_connections();
        }
        assert_eq!(
            transport.idle_calls.load(Ordering::SeqCst) as u64,
            expected["close_idle_calls"].as_u64().unwrap(),
            "{name} idle"
        );
        if let Some(control) = &mut control {
            assert_eq!(
                json!(*control.requests.lock().unwrap()),
                expected_control(&expected["solver_requests"]),
                "{name} solver requests"
            );
            control.stop().await;
        }
    }
}
