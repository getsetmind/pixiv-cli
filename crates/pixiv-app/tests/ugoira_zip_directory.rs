#[path = "../src/download/zip_directory.rs"]
mod zip_directory;

use serde::Deserialize;
use std::{fs, path::Path};

#[derive(Deserialize)]
struct Fixture {
    reference: String,
    go_version: String,
    cases: Vec<Case>,
}
#[derive(Deserialize)]
struct Case {
    name: String,
    zip_hex: String,
    expected: Expected,
}
#[derive(Deserialize)]
struct Expected {
    names_hex: Vec<String>,
    reader_error: String,
}

#[test]
fn raw_directory_entries_match_the_frozen_go_reader() {
    let fixture: Fixture = serde_json::from_str(include_str!("fixtures/ugoira_zip_directory.json"))
        .expect("frozen ZIP fixture");
    assert_eq!(
        fixture.reference,
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    assert_eq!(fixture.go_version, "go1.27.1");
    assert_eq!(fixture.cases.len(), 73);
    let temp = tempfile::tempdir().expect("owned archive directory");
    for case in fixture.cases {
        let path = temp.path().join("archive.zip");
        fs::write(&path, decode_hex(&case.zip_hex)).expect("synthetic archive");
        match zip_directory::read_names(Path::new(&path)) {
            Ok(names) => {
                assert!(case.expected.reader_error.is_empty(), "{}", case.name);
                let expected: Vec<Vec<u8>> = case
                    .expected
                    .names_hex
                    .iter()
                    .map(|v| decode_hex(v))
                    .collect();
                assert_eq!(names, expected, "{}", case.name);
            }
            Err(error) => assert_eq!(
                error.to_string(),
                case.expected.reader_error,
                "{}",
                case.name
            ),
        }
    }
}

fn decode_hex(value: &str) -> Vec<u8> {
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = (pair[0] as char).to_digit(16).expect("hex byte");
            let low = (pair[1] as char).to_digit(16).expect("hex byte");
            ((high << 4) | low) as u8
        })
        .collect()
}
