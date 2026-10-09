#[path = "support/recommended_process.rs"]
mod recommended_process;

#[test]
fn recommended_process_matches_go_configuration_validation_proxy_and_auth_order() {
    let cases: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/cli-recommended-startup.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 216);
    for case in cases {
        if case["read_error"] == true {
            continue;
        }
        recommended_process::assert_startup(&case);
    }
}
