#[path = "support/json_object_order.rs"]
mod json_object_order;
#[path = "support/user_works_process.rs"]
mod user_works_process;
#[test]
fn user_relationships_process_preserves_go_config_proxy_auth_and_optional_stdin_order() {
    let cases: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/cli-user-relationships-startup.json"
    ))
    .unwrap();
    for case in cases {
        if case["read_error"] == true {
            use pixiv_cli_rs::{
                user_relationships::{UserFollowingOptions, UserRelationships},
                user_works::UserWorksOptions,
            };
            struct Failed;
            impl std::io::Read for Failed {
                fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                    Err(std::io::Error::other("fixture read failed"))
                }
            }
            let args = case["args"].as_array().unwrap();
            let listing = UserWorksOptions {
                sources: args
                    .iter()
                    .skip(1)
                    .map(|arg| arg.as_str().unwrap().to_owned())
                    .collect(),
                ..Default::default()
            };
            let mut options = match args[0].as_str().unwrap() {
                "following" => UserRelationships::Following(UserFollowingOptions {
                    listing,
                    restrict: "public".into(),
                }),
                "followers" => UserRelationships::Followers(UserFollowingOptions {
                    listing,
                    restrict: "public".into(),
                }),
                "related" => UserRelationships::Related(listing),
                _ => UserRelationships::Blocked(listing),
            };
            let result = options.resolve_source(&mut Failed, false);
            if args.len() == 1 {
                let mut diagnostics = vec![];
                assert_eq!(
                    pixiv_cli_rs::finish_command(result, false, false, &mut diagnostics),
                    case["exit"].as_i64().unwrap() as i32
                );
                assert_eq!(diagnostics, case["stderr"].as_str().unwrap().as_bytes());
                assert_eq!(case["stdout"], "");
                assert_eq!(case["config"], false);
                assert_eq!(case["database"], false);
            } else {
                result.unwrap();
                let mut process_case = case.clone();
                process_case["read_error"] = false.into();
                user_works_process::assert_startup(&process_case);
            }
        } else {
            user_works_process::assert_startup(&case);
        }
    }
}

#[test]
fn relationship_input_preserves_reader_failures_and_utf8_validation_order() {
    use pixiv_cli_rs::{
        user_relationships::{UserFollowingOptions, UserRelationships},
        user_works::UserWorksOptions,
    };
    struct Failed;
    impl std::io::Read for Failed {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("fixture read failed"))
        }
    }
    for kind in ["following", "followers", "related", "blocked"] {
        let make = |listing: UserWorksOptions, restrict: &str| match kind {
            "following" => UserRelationships::Following(UserFollowingOptions {
                listing,
                restrict: restrict.into(),
            }),
            "followers" => UserRelationships::Followers(UserFollowingOptions {
                listing,
                restrict: restrict.into(),
            }),
            "related" => UserRelationships::Related(listing),
            _ => UserRelationships::Blocked(listing),
        };
        for explicit in [false, true] {
            let mut options = make(
                UserWorksOptions {
                    sources: if explicit { vec!["42".into()] } else { vec![] },
                    ..Default::default()
                },
                "public",
            );
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
        for input in [vec![0xff], vec![b'4', b'2', 0xff, b'\n']] {
            for invalid_restrict in [false, true] {
                if invalid_restrict && !matches!(kind, "following" | "followers") {
                    continue;
                }
                let mut options = make(
                    UserWorksOptions::default(),
                    if invalid_restrict {
                        "invalid"
                    } else {
                        "public"
                    },
                );
                options
                    .resolve_source(&mut input.as_slice(), false)
                    .unwrap();
                let expected = if invalid_restrict {
                    "pixiv:user relationships: invalid_argument: restrict must be public or private"
                        .to_owned()
                } else {
                    format!(
                        "user_id: pixiv:user {kind}: invalid_argument: input must be a positive ID or a supported Pixiv URL"
                    )
                };
                assert_eq!(options.validate().unwrap_err().to_string(), expected);
            }
        }
    }
}
