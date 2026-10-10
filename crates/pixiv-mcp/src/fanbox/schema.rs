use serde_json::{Map, Value, json};

fn object(properties: Value, required: &[&str]) -> Value {
    let mut value = json!({"type":"object","additionalProperties":false});
    if !properties.as_object().is_none_or(Map::is_empty) {
        value["properties"] = properties;
    }
    if !required.is_empty() {
        value["required"] = json!(required);
    }
    value
}
fn resource() -> Value {
    let mut value = object(
        json!({"ref":{"type":"string"},"requires_credentials":{"type":"boolean"}}),
        &["ref"],
    );
    value["type"] = json!(["null", "object"]);
    value
}
fn post() -> Value {
    let asset = object(
        json!({"id":{"type":"string"},"kind":{"type":"string"},"name":{"type":"string"},"resource":resource(),"thumbnail":resource()}),
        &["id", "kind"],
    );
    object(
        json!({"id":{"type":"string"},"title":{"type":"string"},"published_at":{"type":"string"},"creator_id":{"type":"string"},"fee_required":{"type":"integer"},"is_restricted":{"type":"boolean"},"is_pinned":{"type":"boolean"},"restricted_for":{"type":"integer"},"comment_count":{"type":"integer"},"assets":{"type":"array","items":asset}}),
        &[
            "id",
            "title",
            "published_at",
            "creator_id",
            "is_restricted",
            "assets",
        ],
    )
}
fn pagination() -> Value {
    object(
        json!({"page":{"type":"integer"},"limit":{"type":["null","integer"]},"returned":{"type":"integer"},"has_more":{"type":"boolean"},"next_page":{"type":["null","integer"]}}),
        &["page", "limit", "returned", "has_more", "next_page"],
    )
}
fn posts() -> Value {
    object(
        json!({"posts":{"type":"array","items":post()},"pagination":pagination()}),
        &["posts", "pagination"],
    )
}
fn creator_id() -> Value {
    json!({"type":"string","description":"required FANBOX creator id"})
}
fn list_properties() -> Value {
    json!({"page":{"type":["null","integer"],"description":"1-based logical page; requires a positive limit"},"limit":{"type":["null","integer"],"description":"maximum items; 0 returns all items"}})
}
fn tool(name: &str, description: &str, input: Value, output: Value) -> Value {
    json!({"name":name,"description":description,"inputSchema":input,"outputSchema":output})
}

pub fn tools() -> Value {
    let mut creator_posts = list_properties();
    creator_posts["creator_id"] = creator_id();
    let mut tagged_posts = creator_posts.clone();
    tagged_posts["tag"] = json!({"type":"string","description":"required tag name"});
    let mut creators = list_properties();
    creators["kind"] = json!({"type":"string","description":"supporting or following"});
    json!({"tools":[
        tool("fanbox_creator","Get one FANBOX creator profile.",object(json!({"creator_id":creator_id()}), &["creator_id"]),object(json!({"id":{"type":"string"},"name":{"type":"string"},"has_adult_content":{"type":"boolean"},"is_following":{"type":"boolean"},"plan_fee":{"type":"integer"},"has_supporting_plan":{"type":"boolean"},"icon":resource(),"cover":resource()}), &["id","name"])),
        tool("fanbox_creator_posts","List posts from a FANBOX creator.",object(creator_posts,&["creator_id"]),posts()),
        tool("fanbox_creator_tags","List tags used by a FANBOX creator.",object(json!({"creator_id":creator_id()}),&["creator_id"]),object(json!({"tags":{"type":"array","items":object(json!({"name":{"type":"string"},"url":{"type":"string"}}),&["name"])}}),&["tags"])),
        tool("fanbox_creators","List supporting or following FANBOX creators.",object(creators,&[]),object(json!({"creators":{"type":"array","items":object(json!({"id":{"type":"string"},"name":{"type":"string"},"icon":resource()}),&["id"])},"pagination":pagination()}),&["creators","pagination"])),
        tool("fanbox_current_user","Show the current authenticated FANBOX user.",object(json!({}),&[]),object(json!({"user_id":{"type":"integer"},"display_name":{"type":"string"},"creator_id":{"type":"string"},"creator_status":{"type":"string"},"is_creator":{"type":"boolean"}}),&["user_id","display_name","creator_id","creator_status","is_creator"])),
        tool("fanbox_home","Browse the FANBOX home feed.",object(list_properties(),&[]),posts()),
        tool("fanbox_open_resource","Open a FANBOX media resource by ref and return its safe metadata and status without the bytes.",object(json!({"ref":{"type":"string","description":"opaque FANBOX media resource reference"},"method":{"type":"string","description":"GET or HEAD; default GET"}}),&["ref"]),object(json!({"ref":{"type":"string"},"status_code":{"type":"integer"},"content_type":{"type":"string"},"content_length":{"type":"integer"}}),&["ref","status_code"])),
        tool("fanbox_post","Get one FANBOX post.",object(json!({"post_id":{"type":"string","description":"required FANBOX post id"}}),&["post_id"]),post()),
        tool("fanbox_resolve_url","Resolve a FANBOX page URL into a typed reference.",object(json!({"url":{"type":"string","description":"required FANBOX page URL"}}),&["url"]),object(json!({"kind":{"type":"string"},"creator_id":{"type":"string"},"post_id":{"type":"string"},"tag":{"type":"string"}}),&["kind"])),
        tool("fanbox_supporting","Browse posts from supporting creators.",object(list_properties(),&[]),posts()),
        tool("fanbox_tagged_posts","List posts from a creator for one tag.",object(tagged_posts,&["creator_id","tag"]),posts())
    ]})
}

pub fn initialize(version: &str) -> Value {
    let version = match version {
        "2025-06-18" | "2025-03-26" | "2024-11-05" => version,
        _ => "2025-06-18",
    };
    json!({"protocolVersion":version,"capabilities":{"logging":{},"tools":{"listChanged":true}},"instructions":"FANBOX MCP server for browsing creators, posts, tags, and media resources.","serverInfo":{"name":"pixiv-cli-fanbox","version":"1.0.0"}})
}
