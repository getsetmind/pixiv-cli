use clap::Parser;
use pixiv_cli_rs::{
    bookmark_lists::{BookmarkListOptions, BookmarkLists, UserBookmarksOptions},
    finish_command,
};
#[path = "support/bookmark_lists_process.rs"]
mod bookmark_lists_process;
#[path = "support/json_object_order.rs"]
mod json_object_order;

#[derive(Parser)]
#[command(args_override_self = true)]
struct ListArgs {
    #[command(flatten)]
    options: BookmarkListOptions,
}
#[derive(Parser)]
#[command(args_override_self = true)]
struct UserArgs {
    #[command(flatten)]
    options: UserBookmarksOptions,
}
struct Failed;
impl std::io::Read for Failed {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("fixture read failed"))
    }
}
#[test]
fn bookmark_lists_startup_preserves_every_go_config_proxy_auth_and_input_order_row() {
    let cases: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/cli-bookmark-lists-startup.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 724);
    let mut processes = 0;
    let mut readers = 0;
    for case in cases {
        if case["read_error"] == true {
            readers += 1;
            let args = case["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|arg| arg.as_str().unwrap().to_owned());
            let mut options = if case["kind"] == "user" {
                BookmarkLists::User(
                    UserArgs::try_parse_from(std::iter::once("pixiv".to_owned()).chain(args))
                        .unwrap()
                        .options,
                )
            } else {
                BookmarkLists::List(
                    ListArgs::try_parse_from(
                        std::iter::once("pixiv".to_owned())
                            .chain([format!("--type={}", case["kind"].as_str().unwrap())])
                            .chain(args),
                    )
                    .unwrap()
                    .options,
                )
            };
            match options.resolve_source(&mut Failed, false) {
                Err(error) => {
                    let mut diagnostics = vec![];
                    assert_eq!(
                        finish_command(Err(error), false, false, &mut diagnostics),
                        case["exit"].as_i64().unwrap() as i32
                    );
                    assert_eq!(diagnostics, case["stderr"].as_str().unwrap().as_bytes());
                    assert_eq!(case["stdout"], "");
                    assert_eq!(case["config"], false);
                    assert_eq!(case["database"], false);
                    assert_eq!(case["after"], "");
                }
                Ok(()) => {
                    bookmark_lists_process::assert_startup(&case);
                    processes += 1;
                }
            }
        } else {
            bookmark_lists_process::assert_startup(&case);
            processes += 1;
        }
    }
    // A process pipe cannot inject an arbitrary Read error; the four consumed-reader failures use the same public boundary.
    assert_eq!(readers, 8);
    assert_eq!(processes, 720);
}
