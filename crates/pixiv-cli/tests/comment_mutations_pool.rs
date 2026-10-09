use pixiv_app::{
    config::Store,
    database::{Database, PixivAccount},
    execution::Execution,
    lifecycle::Context,
};
use pixiv_cli_rs::{
    comment_mutations::{CommentMutation, Input, saved_mutation},
    finish_command,
};
use pixiv_sdk::transport::{Request, Response, Transport};
use serde::Deserialize;
use serde_json::json;
use std::{
    io::{self, Write},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

#[allow(dead_code)]
#[path = "support/json_object_order.rs"]
mod json_object_order;

#[derive(Deserialize)]
struct Case {
    scenario: String,
    mode: String,
    kind: String,
    operation: String,
    target: i64,
    opens: Vec<i64>,
    requests: Vec<Fetch>,
    closes: usize,
    under_lease: bool,
    states: Vec<State>,
    stdout: String,
    stderr: String,
    exit: i32,
}
#[derive(Debug, Deserialize, PartialEq)]
struct Fetch {
    id: i64,
    offset: String,
    path: String,
    target: String,
}
#[derive(Debug, Deserialize, PartialEq)]
struct State {
    id: i64,
    revision: i64,
    frozen: bool,
    selected: bool,
}
#[derive(Default)]
struct Observed {
    opens: Vec<i64>,
    requests: Vec<Fetch>,
    closes: usize,
    active: usize,
    under_lease: bool,
    output: Vec<u8>,
}
struct Fixture {
    scenario: String,

    database: Arc<Mutex<Database>>,
    observed: Arc<Mutex<Observed>>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let mut observed = self.observed.lock().unwrap();
        observed.closes += 1;
        observed.active -= 1;
    }
}
impl Transport for Fixture {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        let oauth = request.operation == "Open";
        let token = if oauth {
            &request
                .parameters
                .iter()
                .find(|(key, _)| key == "refresh_token")
                .unwrap()
                .1
        } else {
            &request
                .headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case("authorization"))
                .unwrap()
                .1
        };
        let id: i64 = token.rsplit('-').next().unwrap().parse().unwrap();
        let account = self.database.lock().unwrap().get_pixiv(id).unwrap();
        let mut observed = self.observed.lock().unwrap();
        if oauth {
            assert_eq!(token.as_bytes(), account.refresh_token_copy());
            observed.opens.push(id);
            observed.active += 1;
            return Ok(Response {
                status: 200,
                retry_after: None,
                body: json!({"access_token":format!("fixture-access-{id}"),"refresh_token":format!("fixture-rotated-{id}"),"expires_in":3600,"user":{"id":id}}),
            });
        }
        assert_eq!(request.method.as_str(), "POST");
        let path = request
            .url
            .strip_prefix("https://app-api.pixiv.net")
            .unwrap()
            .to_owned();

        assert!(account.credential_revision >= 2);
        assert_eq!(
            account.refresh_token_copy(),
            format!("fixture-rotated-{id}").as_bytes()
        );
        let offset = request
            .parameters
            .iter()
            .find(|(key, _)| key == "offset")
            .map(|(_, value)| value.clone())
            .unwrap_or_default();

        observed.requests.push(Fetch {
            id,
            offset: offset.clone(),
            path: path.clone(),
            target: request
                .parameters
                .iter()
                .find(|(key, _)| key == "illust_id" || key == "novel_id" || key == "comment_id")
                .map(|(_, value)| value.clone())
                .unwrap_or_default(),
        });
        let count = observed
            .requests
            .iter()
            .filter(|fetch| fetch.id == id && fetch.offset == offset && fetch.path == path)
            .count();
        let status = if self.scenario == "not_retryable" {
            401
        } else if self.scenario == "all_rate_limited"
            || id == 42
                && (self.scenario == "before_output"
                    || (self.scenario == "after_page" || self.scenario == "empty_before_output")
                        && (offset == "30" || path == "/v1/stamps"))
        {
            429
        } else {
            200
        };
        Ok(Response {
            status,
            retry_after: (status == 429)
                .then(|| chrono::TimeDelta::seconds(if count % 2 == 1 { 0 } else { 120 })),
            body: json!({"comment_id":123}),
        })
    }
}
struct Output {
    scenario: String,
    observed: Arc<Mutex<Observed>>,
}
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut observed = self.observed.lock().unwrap();
        observed.under_lease |= observed.active != 0;
        match self.scenario.as_str() {
            "other" => Err(io::Error::other("fixture write failed")),
            "broken" => Err(io::Error::new(io::ErrorKind::BrokenPipe, "broken pipe")),
            "short" => {
                let count = bytes
                    .len()
                    .min(10_usize.saturating_sub(observed.output.len()));
                observed.output.extend_from_slice(&bytes[..count]);
                Err(io::Error::other("short write"))
            }
            _ => {
                observed.output.extend_from_slice(bytes);
                Ok(bytes.len())
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn comment_mutations_pool_matches_go_commit_replay_writer_state_and_output_lease() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/cli-comment-mutations-pool.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 22);
    for case in cases {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(
            &path,
            "[account_pool]\nenabled = true\nstrategy = 'round_robin'\n",
        )
        .unwrap();
        let mut database = Database::open(directory.path()).unwrap();
        for id in [42, 43] {
            database
                .save_pixiv_credential(&PixivAccount::new(
                    id,
                    "fixture",
                    format!("fixture-refresh-{id}").as_bytes(),
                ))
                .unwrap();
        }
        database.set_all_pixiv_schedulable(true).unwrap();
        let database = Arc::new(Mutex::new(database));
        let observed = Arc::new(Mutex::new(Observed::default()));
        let factory_database = database.clone();
        let factory_observed = observed.clone();
        let scenario = case.scenario.clone();

        let execution = Execution::new(Store::new(path), database.clone(), move |_| {
            Ok(Fixture {
                scenario: scenario.clone(),

                database: factory_database.clone(),
                observed: factory_observed.clone(),
            })
        });
        let input = Input {
            sources: vec![case.target.to_string()],
            entity: Some(case.kind.clone()),
            ..Default::default()
        };
        let action = match case.operation.as_str() {
            "create" => CommentMutation::Create {
                input,
                comment: "body".into(),
            },
            "delete" => CommentMutation::Delete { input },
            "reply" => CommentMutation::Reply {
                input,
                comment: "body".into(),
                parent_comment_id: 9,
            },
            "stamp" => CommentMutation::Stamp {
                input,
                comment: String::new(),
                stamp_id: 7,
            },
            _ => panic!("unknown operation"),
        };
        let json = case.mode == "json";
        let mut output = Output {
            scenario: case.scenario.clone(),
            observed: observed.clone(),
        };
        let result = saved_mutation(&execution, &Context::new(), action, json, &mut output).await;
        let mut diagnostics = vec![];

        assert_eq!(
            finish_command(result, false, json, &mut diagnostics),
            case.exit,
            "{} {} {}",
            case.scenario,
            case.mode,
            case.kind
        );

        let observed = observed.lock().unwrap();
        assert_eq!(observed.opens, case.opens);
        assert_eq!(observed.requests, case.requests);
        assert_eq!(observed.closes, case.closes);
        assert_eq!(observed.active, 0);
        assert_eq!(
            observed.under_lease, case.under_lease,
            "{} {} {}",
            case.scenario, case.mode, case.kind
        );
        assert_eq!(observed.output, case.stdout.as_bytes());
        if json && !diagnostics.is_empty() {
            assert_eq!(
                json_object_order::canonicalize(&diagnostics),
                json_object_order::canonicalize(case.stderr.as_bytes())
            );
        } else {
            assert_eq!(diagnostics, case.stderr.as_bytes());
        }
        let states: Vec<_> = [42, 43]
            .into_iter()
            .map(|id| {
                let account = database.lock().unwrap().get_pixiv(id).unwrap();
                State {
                    id,
                    revision: account.credential_revision,
                    frozen: account.pool_frozen_until.is_some_and(|until| {
                        until
                            > SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap()
                                .as_secs() as i64
                    }),
                    selected: account.pool_last_selected,
                }
            })
            .collect();
        assert_eq!(states, case.states);
    }
}
