use pixiv_app::auth_bundle::{self, AuthExportAccount, AuthExportBundle};

#[test]
fn compact_export_preserves_go_field_order_html_escaping_and_omission() {
    let bundle = AuthExportBundle {
        schema: auth_bundle::SCHEMA.into(),
        version: 1,
        default_user_id: 0,
        accounts: vec![AuthExportAccount {
            user_id: 7,
            username: "<&>\u{2028}\u{2029}".into(),
            refresh_token: "synthetic".into(),
        }],
    };
    let output = auth_bundle::encode(&bundle).unwrap();
    assert_eq!(output,br#"{"schema":"pixiv-cli.auth-export","version":1,"accounts":[{"user_id":7,"username":"\u003c\u0026\u003e\u2028\u2029","refresh_token":"synthetic"}]}"#);
}
#[test]
fn bundle_validation_retains_go_order_and_null_semantics() {
    let prefix = r#"{"schema":"pixiv-cli.auth-export","version":1,"#;
    let cases = [
        (
            format!("{prefix}\"accounts\":[{{\"user_id\":7,\"refresh_token\":\"synthetic\"}}]}}"),
            None,
        ),
        (
            format!(
                "{prefix}\"VERSION\":null,\"ACCOUNTS\":[{{\"user_id\":1,\"USER_ID\":7,\"refresh_token\":\"synthetic\",\"USERNAME\":null}}]}}"
            ),
            None,
        ),
        (
            format!("{prefix}\"accounts\":[{{\"user_id\":7,\"refresh_token\":\" \"}}]}}"),
            None,
        ),
        (
            format!("{prefix}\"accounts\":[],\"accounts\":[]}}"),
            Some(
                r#"invalid auth export bundle: auth export bundle JSON has duplicate object key "accounts""#,
            ),
        ),
        (
            format!("{prefix}\"unknown\":1,\"accounts\":[{{\"user_id\":7,\"user_id\":8}}]}}"),
            Some(
                r#"invalid auth export bundle: auth export bundle JSON has duplicate object key "user_id""#,
            ),
        ),
        (
            format!("{prefix}\"unknown\":1,\"accounts\":[]}}"),
            Some(r#"invalid auth export bundle: json: unknown field "unknown""#),
        ),
        (
            "null".into(),
            Some("unsupported auth export bundle schema or version"),
        ),
        (
            format!("{prefix}\"accounts\":null}}"),
            Some("auth export bundle has no accounts"),
        ),
        (
            format!("{prefix}\"accounts\":[null]}}"),
            Some("auth export bundle contains an invalid account"),
        ),
        (
            format!(
                "{prefix}\"accounts\":[{{\"user_id\":7,\"refresh_token\":\"s\"}},{{\"user_id\":7,\"refresh_token\":\"s\"}}]}}"
            ),
            Some("auth export bundle contains duplicate account 7"),
        ),
        (
            format!(
                "{prefix}\"default_user_id\":8,\"accounts\":[{{\"user_id\":7,\"refresh_token\":\"s\"}}]}}"
            ),
            Some("auth export bundle default does not name an included account"),
        ),
        (
            format!("{prefix}\"accounts\":[{{\"user_id\":7,\"refresh_token\":\"s\"}}]}} {{}}"),
            Some("auth export bundle has trailing JSON"),
        ),
    ];
    for (body, want) in cases {
        let result = auth_bundle::decode(body.as_bytes());
        match want {
            None => assert_eq!(result.unwrap().accounts[0].user_id, 7),
            Some(e) => assert_eq!(result.err().unwrap().to_string(), e),
        }
    }
}
#[test]
fn source_scanner_panic_is_rejected_without_exposing_secret() {
    let body =
        br#"{"schema":"pixiv-cli.auth-export","version":1,"unknown":[{},"synthetic-secret"]}"#;
    let error = auth_bundle::decode(body).err().unwrap().to_string();
    assert_eq!(
        error,
        r#"invalid auth export bundle: json: unknown field "unknown""#
    );
}

#[test]
fn typed_diagnostics_preserve_number_lexemes_and_scan_precedence() {
    let cases = [
        (
            r#"{"schema":"pixiv-cli.auth-export","version":1,"accounts":[{"user_id":"synthetic-secret","refresh_token":"s"}]}"#,
            r#"invalid auth export bundle: json: cannot unmarshal string into Go struct field authExportBundle.accounts.0.user_id of type int64"#,
        ),
        (
            r#"{"schema":"pixiv-cli.auth-export","version":1,"accounts":[{"user_id":7.0,"refresh_token":"s"}]}"#,
            r#"invalid auth export bundle: json: cannot unmarshal number 7.0 into Go struct field authExportBundle.accounts.0.user_id of type int64"#,
        ),
        (
            r#"{"schema":"pixiv-cli.auth-export","version":1,"accounts":[{"user_id":9223372036854775808,"refresh_token":"s"}]}"#,
            r#"invalid auth export bundle: json: cannot unmarshal number 9223372036854775808 into Go struct field authExportBundle.accounts.0.user_id of type int64"#,
        ),
        (
            r#"{"schema":"pixiv-cli.auth-export","version":1,"accounts":[{"user_id":7,"refresh_token":"s"}],"ACCOUNTS":null}"#,
            r#"auth export bundle has no accounts"#,
        ),
        (
            r#"{"schema":"pixiv-cli.auth-export","version":1,"schema":"#,
            r#"invalid auth export bundle: auth export bundle JSON has duplicate object key "schema""#,
        ),
    ];
    for (body, want) in cases {
        assert_eq!(
            auth_bundle::decode(body.as_bytes())
                .err()
                .unwrap()
                .to_string(),
            want
        )
    }
}
#[test]
fn go_string_decoding_retains_unpaired_surrogates_and_each_invalid_utf8_byte() {
    let bundle=auth_bundle::decode(br#"{"schema":"pixiv-cli.auth-export","version":1,"accounts":[{"user_id":7,"username":"\ud800","refresh_token":"s"}]}"#).unwrap();
    assert_eq!(bundle.accounts[0].username, "\u{fffd}");
    let bundle=auth_bundle::decode(b"{\"schema\":\"pixiv-cli.auth-export\",\"version\":1,\"accounts\":[{\"user_id\":7,\"username\":\"\xff\xff\",\"refresh_token\":\"s\"}]}").unwrap();
    assert_eq!(bundle.accounts[0].username, "\u{fffd}\u{fffd}");
}

#[test]
fn duplicate_scan_rejects_huge_numbers_before_schema_validation() {
    assert_eq!(
        auth_bundle::decode(br#"{"version":1e999}"#)
            .err()
            .unwrap()
            .to_string(),
        "invalid auth export bundle: json: cannot unmarshal number 1e999 into Go value of type float64"
    );
}

#[test]
fn key_quoting_matches_go_for_html_controls_and_unicode_printability() {
    let cases = [
        ("<&>", r#""<&>""#),
        (
            "\0\u{7}\u{8}\t\n\u{b}\u{c}\r\u{1f}\u{7f}",
            r#""\x00\a\b\t\n\v\f\r\x1f\x7f""#,
        ),
        ("é日😀", r#""é日😀""#),
        (
            "\u{a0}\u{200b}\u{2028}\u{e000}",
            r#""\u00a0\u200b\u2028\ue000""#,
        ),
        ("\u{f0000}", r#""\U000f0000""#),
        ("\u{323b0}", "\"\u{323b0}\""),
    ];
    for (key, quoted) in cases {
        let encoded = serde_json::to_string(key).unwrap();
        for duplicate in [false, true] {
            let body = if duplicate {
                format!("{{{encoded}:0,{encoded}:1}}")
            } else {
                format!("{{{encoded}:0}}")
            };
            let label = if duplicate {
                "auth export bundle JSON has duplicate object key"
            } else {
                "json: unknown field"
            };
            assert_eq!(
                auth_bundle::decode(body.as_bytes())
                    .err()
                    .unwrap()
                    .to_string(),
                format!("invalid auth export bundle: {label} {quoted}")
            );
        }
    }
}
#[test]
fn all_valid_scalar_key_diagnostics_match_frozen_go_1_27_1_sha256() {
    use sha2::{Digest, Sha256};
    let key = (0..=0x10ffff)
        .filter_map(char::from_u32)
        .collect::<String>();
    assert_eq!(key.chars().count(), 1_112_064);
    let encoded = serde_json::to_string(&key).unwrap();
    for duplicate in [false, true] {
        let body = if duplicate {
            format!("{{{encoded}:0,{encoded}:1}}")
        } else {
            format!("{{{encoded}:0}}")
        };
        let error = auth_bundle::decode(body.as_bytes())
            .err()
            .unwrap()
            .to_string();
        let actual = format!("{:x}", Sha256::digest(error.as_bytes()));
        let expected = if duplicate {
            "83e09deef6235851859d4605380d173ffdea2a250d4c9d95f25140eb38e77db4"
        } else {
            "78264dd9ca8a15047dba309785b941535e68935901c6f344be0ce65f13cab314"
        };
        assert_eq!(actual, expected);
    }
}
