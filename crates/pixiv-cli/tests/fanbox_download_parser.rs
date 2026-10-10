use pixiv_cli_rs::{fanbox_download::DownloadCommand, finish_command};
use serde_json::Value;
use std::io::Cursor;

fn cases() -> Vec<Value> {
    let fixture: Value =
        serde_json::from_slice(include_bytes!("fixtures/fanbox-download.json")).unwrap();
    fixture["cases"].as_array().unwrap().clone()
}

#[test]
fn download_rejects_every_captured_unsupported_flag_before_startup() {
    let mut compared = 0;
    for row in cases() {
        if !row["name"].as_str().unwrap().starts_with("flag-rejected-") {
            continue;
        }
        let args: Vec<String> = serde_json::from_value(row["input"]["args"].clone()).unwrap();
        let result = DownloadCommand::parse(&args).map(|_| ());
        assert!(result.is_err(), "{}", row["name"]);
        let mut diagnostics = Vec::new();
        let exit = finish_command(result, false, false, &mut diagnostics);
        assert_eq!(
            exit,
            row["observation"]["exits"][0].as_i64().unwrap() as i32
        );
        assert_eq!(
            String::from_utf8(diagnostics).unwrap(),
            row["observation"]["stderr"]
        );
        compared += 1;
    }
    assert_eq!(compared, 11);
}

#[test]
fn download_stdin_is_one_value_and_preserves_a_bare_carriage_return() {
    for (name, expected) in [
        ("stdin-lf", "123"),
        ("stdin-crlf", "123"),
        ("stdin-one-terminal-lf-only", "123\n"),
        ("stdin-bare-cr-preserved", "123\r"),
        ("stdin-spaces-no-split", "123 creator-one"),
        ("stdin-hyphen-not-sentinel", "-"),
    ] {
        let row = cases().into_iter().find(|row| row["name"] == name).unwrap();
        let args: Vec<String> = serde_json::from_value(row["input"]["args"].clone()).unwrap();
        let mut command = DownloadCommand::parse(&args).unwrap();
        let mut input = Cursor::new(row["input"]["stdin"].as_str().unwrap().as_bytes());
        command.resolve_source(&mut input, false).unwrap();
        assert_eq!(command.sources, [expected], "{name}");
    }
}

#[test]
fn bundled_short_help_preserves_the_captured_cobra_flag_order() {
    for name in [
        "help-short-cluster-hh",
        "help-short-cluster-hx",
        "help-short-empty-equals",
    ] {
        let row = cases().into_iter().find(|row| row["name"] == name).unwrap();
        let args: Vec<String> = serde_json::from_value(row["input"]["args"].clone()).unwrap();
        if name.ends_with("-hh") {
            let command = DownloadCommand::parse(&args).unwrap();
            assert!(!command.requires_startup());
            assert_eq!(
                pixiv_cli_rs::fanbox::help_route(&args).unwrap().unwrap(),
                row["observation"]["stdout"]
            );
        } else {
            let result = DownloadCommand::parse(&args).map(|_| ());
            let mut diagnostics = Vec::new();
            assert_eq!(finish_command(result, false, false, &mut diagnostics), 2);
            assert_eq!(
                String::from_utf8(diagnostics).unwrap(),
                row["observation"]["stderr"]
            );
        }
    }
}
