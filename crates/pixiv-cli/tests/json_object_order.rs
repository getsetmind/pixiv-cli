#[path = "support/json_object_order.rs"]
mod json_object_order;
use json_object_order::{canonicalize, canonicalize_ndjson};

#[test]
fn normalization_changes_only_object_entry_order() {
    assert_eq!(
        canonicalize(br#"{ "b": 2, "a": {"z":0,"x":[true,{"b":2,"a":1}]} }"#),
        canonicalize(br#"{ "a": {"x":[true,{"a":1,"b":2}],"z":0}, "b": 2 }"#)
    );
    for (left, right) in [
        (br#"{"a":"<"}"#.as_slice(), br#"{"a":"\u003c"}"#.as_slice()),
        (br#"{"a":1}"#.as_slice(), br#"{"a":1.0}"#.as_slice()),
        (br#"{"a":1}"#.as_slice(), br#"{ "a":1}"#.as_slice()),
        (br#"{"a":[1,2]}"#.as_slice(), br#"{"a":[2,1]}"#.as_slice()),
        (br#"{"a":null}"#.as_slice(), br#"{}"#.as_slice()),
    ] {
        assert_ne!(canonicalize(left), canonicalize(right));
    }
    assert_ne!(
        canonicalize_ndjson(b"{\"a\":1}\n"),
        canonicalize_ndjson(b"{\"a\":1}")
    );
    assert_eq!(canonicalize(br#"{}"#), b"{}");
    assert_eq!(canonicalize(b" {\n }\n"), b" {\n }\n");
}
