use pixiv_cli_rs::{fanbox, finish_command};
use serde_json::Value;
#[test]
fn owned_fanbox_help_and_flag_routing_matches_all_22_frozen_go_rows() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/fanbox-help-routing.json")).unwrap();
    for row in fixture["cases"].as_array().unwrap() {
        let args = row["input"]["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|arg| arg.as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        let mut stderr = vec![];
        let (stdout, result) = match fanbox::help_route(&args) {
            Ok(Some(help)) => (help, Ok(())),
            Ok(None) => (
                String::new(),
                Err(pixiv_cli_rs::CommandError::Message(
                    "usage: pixiv fanbox <command>",
                )),
            ),
            Err(error) => (String::new(), Err(error)),
        };
        let exit = finish_command(result, false, false, &mut stderr);
        assert_eq!(
            stdout,
            row["observation"]["stdout"].as_str().unwrap(),
            "{} stdout",
            row["name"]
        );
        assert_eq!(
            String::from_utf8(stderr).unwrap(),
            row["observation"]["stderr"].as_str().unwrap(),
            "{} stderr",
            row["name"]
        );
        assert_eq!(
            exit,
            row["observation"]["exit"].as_i64().unwrap() as i32,
            "{} exit",
            row["name"]
        );
    }
}
