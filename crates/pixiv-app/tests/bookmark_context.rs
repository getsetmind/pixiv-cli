use pixiv_app::search_filter::{bookmark_context, combine_contexts};
use serde::Deserialize;
#[derive(Deserialize)]
struct Case {
    min: Option<i64>,
    max: Option<i64>,
    strategy: String,
    context: String,
    bookmark: String,
    combined: String,
}
#[test]
fn bookmark_contexts_match_go_nil_zero_strategy_and_ordered_combination() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/bookmark-context.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 192);
    for case in cases {
        let context = bookmark_context(case.min, case.max, &case.strategy);
        assert_eq!(context, case.bookmark);
        assert_eq!(combine_contexts(&[&case.context, &context]), case.combined);
    }
}
