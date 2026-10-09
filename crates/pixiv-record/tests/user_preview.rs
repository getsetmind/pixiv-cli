use pixiv_record::{RecordError, from_user_preview};
use pixiv_sdk::models::{User, UserPreview};
use serde_json::json;

#[test]
fn user_preview_records_preserve_envelope_and_canonical_user_identity() {
    let preview = UserPreview {
        user: User {
            id: 31,
            name: "name\nfirst".into(),
            account: "account".into(),
            comment: "hello\tworld".into(),
            is_followed: true,
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(
        from_user_preview(&preview).unwrap(),
        json!({
            "id":"31", "type":"user", "url":"https://www.pixiv.net/users/31",
            "user": {
                "id":31, "name":"name\nfirst", "account":"account",
                "comment":"hello\tworld", "is_followed":true,
                "profile_image":{"resource":null,"variant":"","width":0,"height":0}
            },
            "illusts":[], "novels":[]
        })
    );
    for id in [0, -1] {
        let preview = UserPreview {
            user: User {
                id,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(matches!(
            from_user_preview(&preview),
            Err(RecordError::InvalidId)
        ));
    }
}
