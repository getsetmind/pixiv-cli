#[path = "support/ugoira_workflow.rs"]
mod support;

use pixiv_app::{
    config::Store,
    database::{Database, PixivAccount},
    execution::Execution,
    lifecycle::Context,
};
use pixiv_cli_rs::{CommandError, finish_command, ugoira::saved_ugoira};
use pixiv_sdk::{
    error::{is_canceled, is_deadline_exceeded},
    transport::HttpTransport,
};
use sha2::{Digest, Sha256};
use std::{
    io,
    sync::{Arc, Mutex, atomic::AtomicBool},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use support::{
    ApiTransport, ErrorObservation, Fixture, Input, Observed, Outcome, Output, PendingServer, State,
};

fn error_observation(error: &CommandError) -> ErrorObservation {
    let classified = error.sdk_error();
    ErrorObservation {
        message: error.to_string(),
        product: classified
            .map(|error| error.product.clone())
            .unwrap_or_default(),
        operation: classified
            .map(|error| error.operation.clone())
            .unwrap_or_default(),
        reason: classified
            .map(|error| error.code.as_str().to_owned())
            .unwrap_or_default(),
        detail: classified
            .and_then(|error| error.detail.clone())
            .unwrap_or_default(),
        transport: classified
            .and_then(|error| error.transport)
            .map(|kind| {
                serde_json::to_value(kind)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .unwrap_or_default(),
        http_status: classified
            .and_then(|error| error.http_status)
            .unwrap_or_default(),
        retry_safe: classified.is_some_and(|error| error.retry.safe),
        retry_has_after: classified.is_some_and(|error| error.retry.after.is_some()),
        canceled: is_canceled(error),
        deadline: is_deadline_exceeded(error),
        broken_pipe: matches!(error, CommandError::Output(cause) if cause.kind() == io::ErrorKind::BrokenPipe),
    }
}
fn assert_outcome(
    input: &Input,
    expected: &Outcome,
    result: Result<(), CommandError>,
    observed: &Arc<Mutex<Observed>>,
    database: &Arc<Mutex<Database>>,
    path: &std::path::Path,
) {
    let actual_error = result.as_ref().err().map(error_observation);
    assert_eq!(actual_error, expected.error, "{} error", input.name);
    let mut diagnostics = vec![];
    assert_eq!(
        finish_command(
            result,
            false,
            matches!(input.mode.as_str(), "json" | "explicit_false"),
            &mut diagnostics
        ),
        expected.exit,
        "{} exit",
        input.name
    );
    assert_eq!(
        diagnostics,
        expected.stderr.as_bytes(),
        "{} stderr",
        input.name
    );
    let observation = observed.lock().unwrap();
    assert_eq!(
        observation.output,
        expected.stdout.as_bytes(),
        "{} stdout",
        input.name
    );
    assert_eq!(
        observation.requests, expected.requests,
        "{} request order/auth/persistence",
        input.name
    );
    assert_eq!(
        observation.closes,
        observation.opens.len(),
        "{} saved client opens/releases",
        input.name
    );
    assert_eq!(
        observation.active, 0,
        "{} client owner retained",
        input.name
    );
    if !observation.output.is_empty() {
        assert!(
            observation.all_writes_under_lease,
            "{} output escaped saved client lease",
            input.name
        );
        assert!(
            observation.output_visible_at_close,
            "{} writer output was deferred until after client close",
            input.name
        );
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let states: Vec<_> = [42, 43]
        .into_iter()
        .map(|id| {
            let account = database.lock().unwrap().get_pixiv(id).unwrap();
            State {
                id,
                revision: account.credential_revision,
                rotated: account.refresh_token_copy() == format!("fixture-rotated-{id}").as_bytes(),
                frozen: account.pool_frozen_until.is_some_and(|until| until > now),
                selected: account.pool_last_selected,
            }
        })
        .collect();
    assert_eq!(states, expected.states, "{} saved state", input.name);
    assert_eq!(
        std::fs::read_to_string(path).unwrap() == input.config,
        expected.config_unchanged,
        "{} config mutation",
        input.name
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn saved_ugoira_matches_frozen_reads_replay_writers_and_pending_http_bodies() {
    let fixture: Fixture =
        serde_json::from_str(include_str!("fixtures/cli-ugoira-workflow.json")).unwrap();
    assert_eq!(
        fixture.reference_commit,
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    assert_eq!(fixture.source_sha256.len(), 13);
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(include_bytes!(
                "../../../docs/migration/contracts/ugoira-metadata.json"
            ))
        ),
        fixture.metadata_fixture_sha256
    );
    assert_eq!(fixture.cases.len(), 74);
    let mut compared = 0;
    let mut go_only = 0;
    let mut pending_bodies = 0;
    for case in fixture.cases {
        // Execution's concrete production close is infallible; a new API solely for injected close failures would misrepresent that boundary.
        if matches!(
            case.input.scenario.as_str(),
            "close_failure" | "writer_close_failure"
        ) {
            go_only += 1;
            continue;
        }
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(&path, &case.input.config).unwrap();
        let mut database = Database::open(directory.path()).unwrap();
        for id in [42, 43] {
            database
                .save_pixiv_credential(&PixivAccount::new(
                    id,
                    "synthetic",
                    format!("fixture-refresh-{id}").as_bytes(),
                ))
                .unwrap();
        }
        database.set_all_pixiv_schedulable(true).unwrap();
        let database = Arc::new(Mutex::new(database));
        let observed = Arc::new(Mutex::new(Observed {
            all_writes_under_lease: true,
            ..Default::default()
        }));
        let pending =
            case.input.scenario.ends_with("_cancel") || case.input.scenario.ends_with("_deadline");
        let server = pending.then(PendingServer::start);
        let pending_url = server.as_ref().map(|server| server.url.clone());
        let factory_input = case.input.clone();
        let factory_database = database.clone();
        let factory_observed = observed.clone();
        let execution = Arc::new(Execution::new(
            Store::new(path.clone()),
            database.clone(),
            move |_| {
                Ok(ApiTransport {
                    input: factory_input.clone(),
                    database: factory_database.clone(),
                    observed: factory_observed.clone(),
                    opened: AtomicBool::new(false),
                    pending_url: pending_url.clone(),
                    http: HttpTransport::new(None).unwrap(),
                })
            },
        ));
        let context = if case.input.scenario.ends_with("_deadline") {
            Context::with_deadline(Instant::now() + Duration::from_millis(250))
        } else {
            Context::new()
        };
        let json = matches!(case.input.mode.as_str(), "json" | "configured_json");
        let result = if let Some(server) = server.as_ref() {
            pending_bodies += 1;
            let run = saved_ugoira(
                &execution,
                &context,
                42,
                Some(""),
                json,
                Output {
                    execution: execution.clone(),
                    scenario: case.input.scenario.clone(),
                    observed: observed.clone(),
                },
            );
            tokio::pin!(run);
            tokio::select! {
                early=&mut run=>panic!("{} returned before the real HTTP body became pending: {early:?}",case.input.name),
                ready=tokio::time::timeout(Duration::from_secs(5),server.started.notified())=>{ready.expect("owned partial HTTP response did not start");}
            }
            if case.input.scenario.ends_with("_cancel") {
                context.cancel();
            }
            let result = tokio::time::timeout(Duration::from_secs(5), &mut run)
                .await
                .expect("pending saved SDK request did not stop");
            server.assert_closed().await;
            result
        } else {
            saved_ugoira(
                &execution,
                &context,
                42,
                Some(""),
                json,
                Output {
                    execution: execution.clone(),
                    scenario: case.input.scenario.clone(),
                    observed: observed.clone(),
                },
            )
            .await
        };
        assert_outcome(
            &case.input,
            &case.outcome,
            result,
            &observed,
            &database,
            &path,
        );
        if let Some(reuse) = case.reuse {
            {
                let mut observation = observed.lock().unwrap();
                observation.reuse = true;
                observation.requests.clear();
                observation.output.clear();
                observation.output_visible_at_close = false;
            }
            let result = tokio::time::timeout(
                Duration::from_secs(5),
                saved_ugoira(
                    &execution,
                    &Context::new(),
                    42,
                    Some(""),
                    json,
                    Output {
                        execution: execution.clone(),
                        scenario: String::new(),
                        observed: observed.clone(),
                    },
                ),
            )
            .await
            .expect("same saved Execution was not reusable after body cancellation");
            assert_outcome(&case.input, &reuse, result, &observed, &database, &path);
        }
        compared += 1;
    }
    assert_eq!(compared, 70);
    assert_eq!(go_only, 4);
    assert_eq!(pending_bodies, 8);
}
