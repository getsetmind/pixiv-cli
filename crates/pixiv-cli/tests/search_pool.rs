#[path = "support/pool_deadline.rs"]
mod pool_deadline;
use pixiv_app::{
    config::Store,
    database::{Database, PixivAccount},
    execution::Execution,
    lifecycle::Context,
};
use pixiv_cli_rs::{
    DetailOutput, finish_command,
    search::{SearchOptions, saved_artwork_search},
};
use pixiv_sdk::transport::{Request, Response, Transport};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    io::{self, Write},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Deserialize)]
struct Case {
    scenario: String,
    mode: String,
    filter: String,
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
    freeze_starts: Vec<(i64, i64)>,
}
struct Fixture {
    scenario: String,
    bodies: Vec<Value>,
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
        assert_eq!(request.url, "https://app-api.pixiv.net/v1/search/illust");
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
        let index = match offset.as_str() {
            "30" => 1,
            "60" => 2,
            _ => 0,
        };
        observed.requests.push(Fetch {
            id,
            offset: offset.clone(),
        });
        let count = observed
            .requests
            .iter()
            .filter(|fetch| fetch.id == id && fetch.offset == offset)
            .count();
        let status = if self.scenario == "not_retryable" {
            401
        } else if self.scenario == "all_rate_limited"
            || id == 42
                && (self.scenario == "before_output"
                    || self.scenario == "after_page" && offset == "30")
        {
            429
        } else {
            200
        };
        if status == 429 && count % 2 == 0 {
            observed
                .freeze_starts
                .push((id, chrono::Utc::now().timestamp()));
        }
        Ok(Response {
            status,
            retry_after: (status == 429)
                .then(|| chrono::TimeDelta::seconds(if count % 2 == 1 { 0 } else { 120 })),
            body: self.bodies[index].clone(),
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
async fn search_pool_matches_go_replay_state_commit_and_release_before_collected_output() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/search-pool.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 63);
    let sources: Value = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/search-pages.json"
    ))
    .unwrap();
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
        let bodies: Vec<Value> = serde_json::from_value(sources[0]["bodies"].clone()).unwrap();
        let execution = Execution::new(Store::new(path), database.clone(), move |_| {
            Ok(Fixture {
                scenario: scenario.clone(),
                bodies: bodies.clone(),
                database: factory_database.clone(),
                observed: factory_observed.clone(),
            })
        });
        let mut options = SearchOptions {
            limit: Some(0),
            ..Default::default()
        };
        if case.filter == "sfw" {
            options.rating = "sfw".into();
        }
        if case.filter == "bookmark" {
            options.bookmark_min = Some(0);
            options.bookmark_strategy = Some("local".into());
        }
        let request = options
            .request("cat", chrono::Utc::now().fixed_offset())
            .unwrap();
        let mode = match case.mode.as_str() {
            "json" => DetailOutput::Json,
            "ndjson" => DetailOutput::Ndjson,
            _ => DetailOutput::Human,
        };
        let invocation_start = chrono::Utc::now();
        let result = saved_artwork_search(
            &execution,
            &Context::new(),
            request,
            options,
            None,
            mode,
            Output {
                scenario: case.scenario.clone(),
                observed: observed.clone(),
            },
        )
        .await;
        let mut diagnostics = vec![];
        let diagnostic_start = chrono::Utc::now();
        assert_eq!(
            finish_command(
                result,
                mode == DetailOutput::Ndjson,
                mode != DetailOutput::Human,
                &mut diagnostics
            ),
            case.exit,
            "{} {} {}",
            case.scenario,
            case.mode,
            case.filter
        );
        let diagnostic_end = chrono::Utc::now();
        let observed = observed.lock().unwrap();
        assert_eq!(observed.opens, case.opens);
        assert_eq!(observed.requests, case.requests);
        assert_eq!(observed.closes, case.closes);
        assert_eq!(observed.active, 0);
        assert_eq!(
            observed.under_lease, case.under_lease,
            "{} {} {}",
            case.scenario, case.mode, case.filter
        );
        let actual = String::from_utf8(observed.output.clone()).unwrap();
        if mode == DetailOutput::Json && !actual.is_empty() && case.scenario != "short" {
            assert_eq!(
                serde_json::from_str::<Value>(&actual).unwrap(),
                serde_json::from_str::<Value>(&case.stdout).unwrap()
            );
        } else if mode == DetailOutput::Ndjson && case.scenario != "short" {
            let parse = |text: &str| {
                text.lines()
                    .map(|line| serde_json::from_str::<Value>(line).unwrap())
                    .collect::<Vec<_>>()
            };
            assert_eq!(parse(&actual), parse(&case.stdout));
        } else {
            assert_eq!(actual, case.stdout);
        }
        if mode != DetailOutput::Human && !diagnostics.is_empty() {
            let actual: Value = serde_json::from_slice(&diagnostics).unwrap();
            let mut expected: Value = serde_json::from_str(&case.stderr).unwrap();
            if case.scenario == "all_rate_limited" {
                for (id, start) in &observed.freeze_starts {
                    let until = database
                        .lock()
                        .unwrap()
                        .get_pixiv(*id)
                        .unwrap()
                        .pool_frozen_until
                        .unwrap();
                    pool_deadline::assert_freeze_deadline(
                        until,
                        *start,
                        invocation_start,
                        diagnostic_start,
                        120,
                    );
                }
                let deadline = [42, 43]
                    .into_iter()
                    .filter_map(|id| {
                        database
                            .lock()
                            .unwrap()
                            .get_pixiv(id)
                            .unwrap()
                            .pool_frozen_until
                    })
                    .min()
                    .unwrap();
                let deadline = chrono::DateTime::from_timestamp(deadline, 0).unwrap();
                let seconds = actual["error"]["retry_after_seconds"].as_i64().unwrap();
                let remaining = |now| {
                    deadline
                        .signed_duration_since(now)
                        .to_std()
                        .unwrap()
                        .as_secs_f64()
                        .ceil() as i64
                };
                assert!(
                    seconds >= remaining(diagnostic_end) && seconds <= remaining(diagnostic_start)
                );
                // A persisted epoch deadline loses elapsed seconds during real SQLite work.
                expected["error"]["retry_after_seconds"] = seconds.into();
            }
            assert_eq!(actual, expected);
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
