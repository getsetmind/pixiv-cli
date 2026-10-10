use pixiv_app::{callback_handler::CallbackResult, lifecycle::Context};
use pixiv_cli_rs::{CommandError, fanbox, finish_command, startup::StartupHooks};
use serde_json::Value;
use std::io::{self, Read};
struct Hooks(bool);
impl StartupHooks for Hooks {
    fn cleanup_pending_update(&self) -> CallbackResult<()> {
        if self.0 {
            Err(Box::new(io::Error::other(
                "owned startup stop before external effect",
            )))
        } else {
            Ok(())
        }
    }
    fn automatic_supported(&self) -> bool {
        false
    }
    fn ensure_if_needed(&self, _: &Context) -> CallbackResult<()> {
        panic!("owned unsupported platform hook")
    }
}
struct Input(bool);
impl Read for Input {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        if self.0 {
            Err(io::Error::other("owned stdin failure"))
        } else {
            Ok(0)
        }
    }
}
#[tokio::test]
async fn actual_root_read_composition_preserves_all_five_frozen_go_stop_boundaries() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/fanbox-content-reads.json")).unwrap();
    for row in fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["input"]["boundary"] != "composition")
    {
        let input = &row["input"];
        let context = Context::background();
        let args = input["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|arg| arg.as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        let mut stdout = vec![];
        let mut stderr = vec![];
        let mut reader = Input(input["stdin_error"] == true);
        let hooks = Hooks(input["boundary"] == "root-startup");
        let prepared = fanbox::prepare_root(
            &context,
            &args,
            &mut reader,
            false,
            &hooks,
            &mut stderr,
            || {
                pixiv_app::config::Snapshot::parse(input["config"].as_str().unwrap(), [])
                    .map(|_| ())
                    .map_err(|error| CommandError::App(error.into()))
            },
        );
        let result = match prepared {
            Ok(command) => {
                command
                    .execute(
                        &context,
                        || {
                            Err(CommandError::Message(
                                "owned stop at FANBOX service composition",
                            ))
                        },
                        &mut stdout,
                    )
                    .await
            }
            Err(error) => Err(error),
        };
        let exit = finish_command(
            result,
            false,
            args.iter().any(|arg| arg == "--json"),
            &mut stderr,
        );
        assert_eq!(
            String::from_utf8(stdout).unwrap(),
            row["observation"]["stdout"].as_str().unwrap(),
            "{} stdout",
            row["name"]
        );
        assert_eq!(
            String::from_utf8(stderr).unwrap(),
            row["observation"]["stderr"].as_str().unwrap(),
            "{} stderr",
            row["name"]
        );
        assert_eq!(
            exit,
            row["observation"]["exits"][0].as_i64().unwrap() as i32,
            "{} exit",
            row["name"]
        );
    }
}
