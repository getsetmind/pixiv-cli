#[path = "support/json_object_order.rs"]
mod json_object_order;
#[path = "support/user_works_process.rs"]
mod user_works_process;
#[test]
fn mypixiv_process_preserves_go_config_proxy_auth_no_input_and_validation_order() {
    let cases: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/cli-mypixiv-startup.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 25);
    for mut case in cases {
        if case["args"][0] == "users" {
            assert_eq!(case["bytes"], 0);
            assert_eq!(case["read_calls"], 0);
        }
        if case["read_error"] == true && case["args"][0] == "works" {
            continue;
        }
        case["read_error"] = false.into();
        case["group"] = "mypixiv".into();
        user_works_process::assert_startup(&case);
    }
}

#[test]
fn mypixiv_root_flags_preserve_go_base_zero_integers_boolean_spellings_and_parser_diagnostics() {
    let cases: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/cli-mypixiv-flags.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 33);
    for mut case in cases {
        case["group"] = "mypixiv".into();
        user_works_process::assert_startup(&case);
    }
}
