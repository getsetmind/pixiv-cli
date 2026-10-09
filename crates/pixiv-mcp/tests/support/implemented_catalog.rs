use serde_json::Value;

const IMPLEMENTED_NAMES: [&str; 38] = [
    "illust_detail",
    "search_illust",
    "trending_tags_illust",
    "illust_ranking",
    "novel_detail",
    "search_novel",
    "user_detail",
    "search_user",
    "add_bookmark",
    "remove_bookmark",
    "add_novel_bookmark",
    "remove_novel_bookmark",
    "follow_user",
    "unfollow_user",
    "novel_series",
    "novel_content",
    "illust_series",
    "illust_related",
    "illust_recommended",
    "recommended",
    "user_artworks",
    "user_novels",
    "user_following",
    "user_followers",
    "related_users",
    "blocked_users",
    "user_bookmarks",
    "user_novel_bookmarks",
    "bookmark_list_all",
    "bookmark_detail",
    "novel_bookmark_detail",
    "bookmark_tags",
    "novel_bookmark_tags",
    "bookmark_tags_all",
    "timeline_illust_following",
    "timeline_novel_following",
    "timeline_illust_latest",
    "timeline_novel_latest",
];

pub fn assert_catalog(tools: &[Value]) {
    let names = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        names, IMPLEMENTED_NAMES,
        "implemented catalog names and order"
    );
    let frozen: Vec<Value> = serde_json::from_str(include_str!(
        "../../../../docs/migration/reference/mcp-pixiv.json"
    ))
    .unwrap();
    for (index, name) in IMPLEMENTED_NAMES.iter().enumerate() {
        let expected = frozen.iter().find(|tool| tool["name"] == *name).unwrap();
        assert_eq!(
            &tools[index], expected,
            "{name} frozen Go metadata at index {index}"
        );
    }
}
