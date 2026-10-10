use pixiv_record::from_identity;
use serde_json::json;

#[test]
fn identity_records_keep_artwork_and_user_without_guessing_a_subtype() {
    assert_eq!(
        from_identity(17, "artwork", "https://www.pixiv.net/artworks/17").unwrap(),
        json!({"id":"17","type":"artwork","url":"https://www.pixiv.net/artworks/17"})
    );
    assert_eq!(
        from_identity(23, "user", "https://www.pixiv.net/users/23").unwrap(),
        json!({"id":"23","type":"user","url":"https://www.pixiv.net/users/23"})
    );
    assert!(from_identity(0, "artwork", "https://www.pixiv.net/artworks/0").is_err());
}

#[test]
fn identity_record_validation_keeps_safe_go_messages_and_exact_large_ids() {
    assert_eq!(
        from_identity(-1, "opaque", "owned")
            .unwrap_err()
            .to_string(),
        "record id must be positive"
    );
    assert_eq!(
        from_identity(1, "opaque", "owned").unwrap_err().to_string(),
        "identity record type must be artwork or user"
    );
    assert_eq!(
        from_identity(1, "artwork", "owned")
            .unwrap_err()
            .to_string(),
        "record url must be canonical for artwork identity"
    );
    assert_eq!(
        from_identity(1, "user", "owned").unwrap_err().to_string(),
        "record url must be canonical for user identity"
    );
    let url = format!("https://www.pixiv.net/artworks/{}", i64::MAX);
    assert_eq!(
        from_identity(i64::MAX, "artwork", &url).unwrap()["id"],
        i64::MAX.to_string()
    );
}
