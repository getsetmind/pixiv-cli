use pixiv_cli_rs::{
    CommandError,
    bookmark_lists::{BookmarkListOptions, BookmarkLists},
    bookmark_reads::{BookmarkDetailOptions, BookmarkTagsOptions},
};
use serde_json::Value;
use std::io::Cursor;

fn validate(command: &str, input: &mut Cursor<Vec<u8>>) -> Result<(), CommandError> {
    match command {
        "detail" => {
            let mut options = BookmarkDetailOptions::default();
            options.resolve_source(input, false)?;
            options.resolve_target(input)?;
            options.validate()
        }
        "list" => {
            let mut options = BookmarkLists::List(BookmarkListOptions::default());
            options.resolve_source(input, false)?;
            options.resolve_target(input)?;
            options.validate()
        }
        "tags" => {
            let mut options = BookmarkTagsOptions::default();
            options.resolve_source(input, false)?;
            options.resolve_target(input)?;
            options.validate()
        }
        _ => panic!("unexpected bookmark command"),
    }
}

#[test]
fn bookmark_whole_body_json_keeps_frozen_go_whitespace_before_the_pool_boundary() {
    let fixture: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/bookmark_record_whitespace.json")).unwrap();
    assert_eq!(fixture.len(), 9);
    let mut differences = Vec::new();
    for row in fixture {
        let bytes = row["input_hex"]
            .as_str()
            .unwrap()
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect();
        let result = validate(row["command"].as_str().unwrap(), &mut Cursor::new(bytes));
        let error = result
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();
        let expected_error = if row["pooled_called"] == true {
            ""
        } else {
            row["error"].as_str().unwrap()
        };
        if error != expected_error {
            differences.push(format!(
                "{} {}: input error {error:?}, frozen pre-pool error {expected_error:?}",
                row["command"], row["suffix"],
            ));
        }
    }
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}
