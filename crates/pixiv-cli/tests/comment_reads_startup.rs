#[path = "support/comment_reads_process.rs"]
mod comment_reads_process;
#[path = "support/json_object_order.rs"]
mod json_object_order;
#[test]
fn comment_reads_startup_preserves_go_configuration_auth_and_validation_order() {
    let cases: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/cli-comment-reads-startup.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 30);
    for case in cases {
        comment_reads_process::assert_startup(&case);
    }
}
