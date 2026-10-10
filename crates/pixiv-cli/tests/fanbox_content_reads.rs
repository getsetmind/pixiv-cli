use pixiv_cli_rs::fanbox::ReadCommand;
use serde_json::Value;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/fanbox-content-reads.json")).unwrap()
}

#[test]
fn fanbox_flags_preserve_frozen_go_validation_order() {
    let cases = fixture();
    for name in [
        "flags-negative-limit-before-page",
        "flags-explicit-zero-page",
        "flags-page-requires-positive-limit",
        "flags-json-false-still-conflicts-ndjson",
        "flags-json-true-conflicts-ndjson",
    ] {
        let case = cases["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["name"] == name)
            .unwrap();
        let args = case["input"]["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|arg| arg.as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        let command = ReadCommand::parse(&args).unwrap();
        let error = command.validate().unwrap_err();
        assert_eq!(
            error.to_string(),
            case["observation"]["errors"][0].as_str().unwrap(),
            "{name}"
        );
    }
}

#[test]
fn fanbox_does_not_implicitly_enable_ndjson_for_a_pipe() {
    let command = ReadCommand::parse(&["fanbox".into(), "post".into(), "123".into()]).unwrap();
    assert_eq!(
        command.output_mode().unwrap(),
        pixiv_cli_rs::DetailOutput::Human
    );
}

mod fanbox_cli_support;
#[tokio::test]
async fn six_fanbox_presenters_use_actual_sdk_responses() {
    use pixiv_sdk::{
        context::Context,
        fanbox::{Client, Options, SessionCredentials},
    };
    use std::sync::Arc;
    let document = fixture();
    for case in &document["cases"].as_array().unwrap()[..18] {
        let input = &case["input"];
        let args = input["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|arg| arg.as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        let command = ReadCommand::parse(&args).unwrap();
        let transport = Arc::new(fanbox_cli_support::Transport::new(
            input["replies"].as_array().unwrap().clone(),
        ));
        let client = Client::open_with(
            SessionCredentials {
                fanbox_sessid: "owned-session-42".into(),
            },
            Options {
                http_client: Some(transport.clone()),
                ..Default::default()
            },
        )
        .unwrap();
        let mut output = vec![];
        command
            .execute_with_client(&Context::background(), &client, &mut output)
            .await
            .unwrap();
        assert_eq!(
            String::from_utf8(output).unwrap(),
            case["observation"]["stdout"].as_str().unwrap(),
            "{}",
            case["name"]
        );
        assert_eq!(
            *transport.requests.lock().unwrap(),
            *case["observation"]["requests"].as_array().unwrap(),
            "{} requests",
            case["name"]
        );
        assert_eq!(
            *transport.closes.lock().unwrap(),
            case["observation"]["body_closes"].as_u64().unwrap() as usize,
            "{} closes",
            case["name"]
        );
    }
}
