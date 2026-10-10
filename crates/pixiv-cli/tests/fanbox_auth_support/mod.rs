#[cfg(target_os = "linux")]
pub mod binary;
mod browser;
mod io;
mod ports;
mod runtime;
pub mod schema;
mod state;
mod transport;

use pixiv_app::{
    config::Store, database::Database, fanbox_account_service::AccountService, lifecycle::Context,
    scheduler::SchedulerError,
};
use pixiv_cli_rs::{
    CommandError,
    fanbox_auth::{self, AuthCommand, Data},
    finish_command, finish_with_cleanup,
};
use pixiv_sdk::context::{ContextKey, RequestContext};
use schema::{Case, Observation};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex},
};

pub type Shared = Arc<Mutex<Observation>>;
fn trace(observed: &Shared, value: impl Into<String>) {
    observed.lock().unwrap().trace.push(value.into());
}
fn caller_key() -> ContextKey {
    ContextKey::new("fanbox-auth-owned-command")
}
fn context_marker(context: &dyn RequestContext) -> String {
    let mut marker = context
        .value(&caller_key())
        .and_then(|value| value.downcast::<String>().ok())
        .map_or_else(|| "<nil>".into(), |value| (*value).clone());
    if let Some(error) = context.error() {
        marker.push('/');
        marker.push_str(&error.to_string());
    }
    marker
}

pub async fn observe(home: &Path, case: &Case) -> Vec<Observation> {
    let directory = home.join(".pixiv-cli");
    if directory.exists() {
        std::fs::remove_dir_all(&directory).unwrap();
    }
    let start = state::now();
    let mut created = BTreeMap::new();
    state::seed(home, case);
    let mut observations = Vec::new();
    for step in &case.steps {
        let observed = Arc::new(Mutex::new(Observation {
            before: state::snapshot(home, start, &mut created),
            ..Default::default()
        }));
        let context = Context::new().with_value(caller_key(), Arc::new(String::from("command")));
        if step.canceled {
            context.cancel();
        }
        let hooks = runtime::Hooks {
            step: step.clone(),
            observed: observed.clone(),
        };
        let mut reader = io::Reader::new(&step.input_hex, step.input_error, observed.clone());
        let mut output = io::Writer::new(&step.writer, "stdout", observed.clone());
        let mut diagnostics = io::Writer::new(&step.stderr_writer, "stderr", observed.clone());
        let mut prompts = runtime::Prompts {
            step: step.clone(),
            observed: observed.clone(),
        };
        let browser = browser::Browser {
            step: step.clone(),
            observed: observed.clone(),
        };
        let owned_database = Arc::new(Mutex::new(None::<Arc<Mutex<Database>>>));
        let machine = AuthCommand::parse(&step.args)
            .as_ref()
            .is_ok_and(AuthCommand::machine_requested);
        let prepared = fanbox_auth::prepare_root(
            &context,
            &step.args,
            &mut reader,
            false,
            &hooks,
            &mut diagnostics,
        );
        let mut result = match prepared {
            Err(error) => Err(error),
            Ok(command) => {
                let factory_observed = observed.clone();
                let factory_owned = owned_database.clone();
                let factory_directory = directory.clone();
                let factory_step = step.clone();
                let factory_context = context.clone();
                let result = command
                    .execute(
                        &context,
                        Data {
                            service_factory: move || {
                                trace(&factory_observed, "account.factory");
                                if factory_step.factory_error {
                                    return Err(CommandError::Message(
                                        "owned fixture account factory failure",
                                    ));
                                }
                                if factory_step.factory_nil {
                                    return Ok(None);
                                }
                                let database =
                                    Database::open(&factory_directory).map_err(|error| {
                                        CommandError::MessageText(error.to_string())
                                    })?;
                                trace(&factory_observed, "database.open");
                                let database = Arc::new(Mutex::new(database));
                                *factory_owned.lock().unwrap() = Some(database.clone());
                                let repository = Arc::new(ports::RepositoryPort {
                                    database,
                                    observed: factory_observed.clone(),
                                    failure: factory_step.repository_failure.clone(),
                                });
                                let store = Store::with_files(
                                    factory_directory.join("config.toml"),
                                    Some(Arc::new(ports::Files {
                                        path: factory_directory.join("config.toml"),
                                        failure: factory_step.file_failure.clone(),
                                        observed: factory_observed.clone(),
                                    })),
                                );
                                let defaults = Arc::new(ports::DefaultPort {
                                    store,
                                    step: factory_step.clone(),
                                    observed: factory_observed.clone(),
                                    reads: Mutex::new(0),
                                });
                                let mut service =
                                    AccountService::new(Some(repository), Some(defaults));
                                let options_observed = factory_observed.clone();
                                let options_step = factory_step.clone();
                                let options_context = factory_context.clone();
                                service.load_options = Some(Arc::new(move || {
                                    trace(&options_observed, "options.load");
                                    if options_step.options_error {
                                        return Err(SchedulerError::Message(
                                            "owned fixture options failure".into(),
                                        ));
                                    }
                                    Ok(pixiv_sdk::fanbox::Options {
                                        user_agent: "fixture-auth-agent".into(),
                                        http_client: Some(Arc::new(transport::Transport {
                                            step: options_step.clone(),
                                            observed: options_observed.clone(),
                                            context: options_context.clone(),
                                        })),
                                        ..Default::default()
                                    })
                                }));
                                if factory_step.inject_session {
                                    service.open_session = Some(Arc::new(move |session| {
                                        transport::injected_client(
                                            &factory_step,
                                            &factory_observed,
                                            &factory_context,
                                            session,
                                        )
                                    }));
                                }
                                Ok(Some(Arc::new(service)))
                            },
                            reader: &mut reader,
                            writer: &mut output,
                            prompts: &mut prompts,
                            browser: &browser,
                        },
                    )
                    .await;
                if result.is_ok() {
                    command.post_success(&context, step.release, &hooks, &mut diagnostics);
                }
                result
            }
        };
        if let Some(database) = owned_database.lock().unwrap().take() {
            trace(&observed, "database.close");
            let database = Arc::try_unwrap(database)
                .unwrap_or_else(|_| panic!("owned command retained database after completion"));
            let database = database.into_inner().unwrap();
            let close = database
                .close()
                .map_err(|error| CommandError::MessageText(error.to_string()));
            let close = if step.close_error {
                match close {
                    Ok(()) => Err(CommandError::Message(
                        "owned fixture database close failure",
                    )),
                    Err(error) => Err(CommandError::Joined(vec![
                        error,
                        CommandError::Message("owned fixture database close failure"),
                    ])),
                }
            } else {
                close
            };
            result = finish_with_cleanup(result, close);
        }
        let exit = finish_command(result, false, machine, &mut diagnostics);
        observed.lock().unwrap().exit = exit;
        output.record();
        diagnostics.record();
        observed.lock().unwrap().after = state::snapshot(home, start, &mut created);
        context.cancel();
        drop(reader);
        drop(prompts);
        drop(browser);
        drop(hooks);
        let observation = Arc::try_unwrap(observed)
            .unwrap_or_else(|_| panic!("owned observation retained after command"))
            .into_inner()
            .unwrap();
        observations.push(observation);
    }
    observations
}
