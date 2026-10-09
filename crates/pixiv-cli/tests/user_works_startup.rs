#[path = "support/json_object_order.rs"]
mod json_object_order;
#[path = "support/user_works_process.rs"]
mod user_works_process;
#[test]
fn user_works_process_preserves_go_config_proxy_auth_and_optional_stdin_order() {
    let cases: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/cli-user-works-startup.json"
    ))
    .unwrap();
    for case in cases {
        if case["read_error"] == true {
            continue;
        }
        user_works_process::assert_startup(&case);
    }
}
#[test]
fn user_works_optional_input_preserves_reader_failure_and_explicit_skipping() {
    use pixiv_cli_rs::user_works::{UserWorks, UserWorksOptions};
    struct Failed;
    impl std::io::Read for Failed {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("fixture read failed"))
        }
    }
    for explicit in [false, true] {
        let mut options = UserWorks::Novels(UserWorksOptions {
            sources: if explicit { vec!["42".into()] } else { vec![] },
            ..Default::default()
        });
        let result = options.resolve_source(&mut Failed, false);
        if explicit {
            result.unwrap();
        } else {
            assert_eq!(
                result.unwrap_err().to_string(),
                "read stdin value: fixture read failed"
            );
        }
    }
}
#[test]
fn user_works_invalid_utf8_stdin_preserves_go_id_and_type_validation() {
    use pixiv_cli_rs::user_works::{UserArtworksOptions, UserWorks, UserWorksOptions};
    for artwork in [false, true] {
        for input in [vec![0xff], vec![b'4', b'2', 0xff, b'\n']] {
            for invalid_type in [false, true] {
                if invalid_type && !artwork {
                    continue;
                }
                let mut options = if artwork {
                    UserWorks::Artworks(UserArtworksOptions {
                        listing: UserWorksOptions::default(),
                        kind: if invalid_type {
                            "invalid"
                        } else {
                            "illustration"
                        }
                        .into(),
                    })
                } else {
                    UserWorks::Novels(UserWorksOptions::default())
                };
                options
                    .resolve_source(&mut input.as_slice(), false)
                    .unwrap();
                let expected = if invalid_type {
                    "pixiv:user artworks: invalid_argument: type must be one of illustration, illust, manga, or ugoira".to_owned()
                } else {
                    format!(
                        "user_id: pixiv:user {}: invalid_argument: input must be a positive ID or a supported Pixiv URL",
                        options.name()
                    )
                };
                assert_eq!(options.validate().unwrap_err().to_string(), expected);
            }
        }
    }
}
