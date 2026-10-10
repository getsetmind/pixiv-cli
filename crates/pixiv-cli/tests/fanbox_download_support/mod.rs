pub mod binary;
mod io;
pub mod isolation;
pub mod schema;
mod state;
mod transport;

use pixiv_app::{
    callback_handler::CallbackResult,
    config::Store,
    database::Database,
    fanbox_account_service::AccountService,
    fanbox_facade::{AccountOpener, Facade},
    lifecycle::Context,
    scheduler::SchedulerError,
    sessions::ClientOpen,
};
use pixiv_cli_rs::{
    CommandError, fanbox_download, finish_command, finish_with_cleanup, startup::StartupHooks,
};
use pixiv_sdk::fanbox::Client;
use schema::{Input, Observation};
use serde_json::json;
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Instant,
};

type Shared = Arc<Mutex<Observation>>;
fn trace(observed: &Shared, value: impl Into<String>) {
    observed.lock().unwrap().trace.push(value.into());
}
struct Hooks {
    observed: Shared,
    fail: bool,
}
impl StartupHooks for Hooks {
    fn cleanup_pending_update(&self) -> CallbackResult<()> {
        trace(&self.observed, "startup.cleanup");
        if self.fail {
            Err(Box::new(std::io::Error::other(
                "owned startup stop before external effect",
            )))
        } else {
            Ok(())
        }
    }
    fn automatic_supported(&self) -> bool {
        trace(&self.observed, "startup.supported");
        false
    }
    fn ensure_if_needed(&self, _: &Context) -> CallbackResult<()> {
        panic!("forbidden native-handler")
    }
}
struct ForbiddenUpdate;
impl pixiv_cli_rs::fanbox_auth::AutomaticUpdateHooks for ForbiddenUpdate {
    fn runtime(&self) -> Result<pixiv_cli_rs::fanbox_auth::UpdateRuntime, CommandError> {
        panic!("forbidden automatic-update runtime in development build")
    }
    fn check(&self, _: &Context, _: &str) -> Result<(), CommandError> {
        panic!("forbidden automatic-update check in development build")
    }
}

#[derive(Default)]
struct OwnedDatabase {
    before: bool,
    database: Option<Arc<Mutex<Database>>>,
}
fn runtime(
    store: &Store,
    observed: &Shared,
) -> Result<pixiv_app::config::RuntimeConfig, CommandError> {
    trace(observed, "runtime.read");
    store.ensure_defaults().map_err(SchedulerError::from)?;
    store
        .current()
        .and_then(|snapshot| snapshot.runtime())
        .map_err(|error| SchedulerError::from(error).into())
}

pub async fn observe(home: &Path, input: Input, denied: (bool, bool)) -> Observation {
    let directory = home.join(".pixiv-cli");
    if directory.exists() {
        std::fs::remove_dir_all(&directory).unwrap();
    }
    let output_root = home.join(&input.output_root);
    if output_root.exists() {
        if output_root.is_dir() {
            std::fs::remove_dir_all(&output_root).unwrap();
        } else {
            std::fs::remove_file(&output_root).unwrap();
        }
    }
    state::seed(&directory, &input);
    state::seed_files(home, &input);
    let database_path = directory.join("pixiv-cli.db");
    let observed = Arc::new(Mutex::new(Observation {
        db_before: state::hash(&database_path),
        db_rows_before: state::rows(&database_path),
        files_before: state::files(home, &input.output_root),
        socket_denied: denied.0,
        exec_denied: denied.1,
        ..Default::default()
    }));
    let context = if input.cancel == "deadline" {
        Context::with_deadline(Instant::now())
    } else {
        Context::new()
    };
    if input.cancel == "before" {
        context.cancel();
    }
    let transport = Arc::new(transport::Transport {
        input: input.clone(),
        observed: observed.clone(),
        context: context.clone(),
        index: Mutex::new(0),
    });
    let clients = Arc::new(Mutex::new(Vec::<Arc<Client>>::new()));
    let mut reader = io::Reader::new(&input, observed.clone());
    let mut writer = io::Writer::new(&input, observed.clone());
    let hooks = Hooks {
        observed: observed.clone(),
        fail: input.startup_error,
    };
    let store = Arc::new(Store::new(directory.join("config.toml")));
    let mut diagnostics = vec![];
    for _ in 0..input.repeat.max(1) {
        let owned = Arc::new(Mutex::new(OwnedDatabase::default()));
        let prepared = fanbox_download::prepare_root(
            &context,
            &input.args,
            &mut reader,
            false,
            &hooks,
            &mut diagnostics,
            || runtime(&store, &observed).map(|_| ()),
        );
        let result = match prepared {
            Err(error) => Err(error),
            Ok(command) if !command.requires_startup() => {
                let result = command
                    .execute(
                        &context,
                        Path::new("."),
                        || panic!("help opened the service factory"),
                        &mut writer,
                    )
                    .await;
                if result.is_ok() {
                    command.post_success(&context, false, &ForbiddenUpdate, &mut diagnostics);
                }
                result
            }
            Ok(command) => match runtime(&store, &observed) {
                Err(error) => Err(error),
                Ok(config) => {
                    state::owned_relative(Path::new(&config.download_path));
                    let service_input = input.clone();
                    let service_observed = observed.clone();
                    let service_owned = owned.clone();
                    let service_directory = directory.clone();
                    let service_store = store.clone();
                    let service_transport = transport.clone();
                    let service_clients = clients.clone();
                    let result = command.execute(&context, Path::new(&config.download_path), move || {
                                trace(&service_observed, "service.open");
                                match service_input.factory.as_str() {
                                    "error" => return Err(CommandError::Message("owned service factory failure")),
                                    "nil" => return Ok(None),
                                    "" => (),
                                    other => panic!("unknown factory fault {other}"),
                                }
                                service_owned.lock().unwrap().before = true;
                                let database = Database::open(&service_directory).map_err(|error| CommandError::MessageText(error.to_string()))?;
                                let database = Arc::new(Mutex::new(database));
                                service_owned.lock().unwrap().database = Some(database.clone());
                                let mut accounts = AccountService::from_store(database, service_store);
                                let original = accounts.load_options.take().unwrap();
                                let options_observed = service_observed.clone();
                                let options_transport = service_transport.clone();
                                let options_error = service_input.options_error;
                                accounts.load_options = Some(Arc::new(move || {
                                    trace(&options_observed, "options.load");
                                    if options_error { return Err(SchedulerError::Message("owned options failure".into())); }
                                    trace(&options_observed, "runtime.read");
                                    let mut options = original()?;
                                    let solver = options.flare_solverr.as_ref().map(|solver| json!({"url":solver.url,"proxy_url":solver.proxy_url}));
                                    options_observed.lock().unwrap().options.push(json!({"proxy_url":options.proxy_url,"user_agent":options.user_agent,"solver":solver}));
                                    options.http_client = Some(options_transport.clone());
                                    Ok(options)
                                }));
                                let opener = Arc::new(Opener { accounts, observed: service_observed.clone(), clients: service_clients, mode: service_input.open });
                                let close_error = service_input.close_error;
                                Ok(Some(Arc::new(Facade::with_close_client(Some(opener), Some(Arc::new(move |client| {
                                    service_observed.lock().unwrap().lease_closes += 1;
                                    trace(&service_observed, "lease.close");
                                    client.close_idle_connections();
                                    if close_error { Err(SchedulerError::Message("owned lease close failure".into())) } else { Ok(()) }
                                }))))))
                            }, &mut writer).await;
                    if result.is_ok() {
                        command.post_success(&context, false, &ForbiddenUpdate, &mut diagnostics);
                    }
                    result
                }
            },
        };
        let mut result = result;
        let OwnedDatabase { before, database } = std::mem::take(&mut *owned.lock().unwrap());
        if let Some(database) = database {
            trace(&observed, "root.cleanup.after-database");
            let canary = if input.root_close_error {
                Err(CommandError::Message("owned root cleanup failure"))
            } else {
                Ok(())
            };
            let database = Arc::try_unwrap(database)
                .unwrap_or_else(|_| panic!("download retained owned database after execution"))
                .into_inner()
                .unwrap();
            let close = database
                .close()
                .map_err(|error| CommandError::MessageText(error.to_string()));
            result = finish_with_cleanup(result, finish_with_cleanup(canary, close));
        }
        if before {
            trace(&observed, "root.cleanup.before-database");
        }
        let exit = finish_command(result, false, false, &mut diagnostics);
        observed.lock().unwrap().exits.push(exit);
    }
    drop(reader);
    drop(hooks);
    drop(transport);
    drop(clients);
    let stdout = String::from_utf8(std::mem::take(&mut writer.bytes)).unwrap();
    drop(writer);
    let mut observed = Arc::try_unwrap(observed)
        .unwrap_or_else(|_| panic!("download retained owned observation after execution"))
        .into_inner()
        .unwrap();
    observed.stdout = stdout.replace(home.to_str().unwrap(), "<HOME>");
    observed.stderr = String::from_utf8(diagnostics)
        .unwrap()
        .replace(home.to_str().unwrap(), "<HOME>");
    for write in &mut observed.output_writes {
        write["bytes"] = write["bytes"]
            .as_str()
            .unwrap()
            .replace(home.to_str().unwrap(), "<HOME>")
            .into();
    }
    observed.db_after = state::hash(&database_path);
    observed.db_rows_after = state::rows(&database_path);
    observed.config_after = std::fs::read_to_string(directory.join("config.toml")).unwrap();
    observed.files_after = state::files(home, &input.output_root);
    observed.remaining_temps = observed
        .files_after
        .iter()
        .filter(|file| {
            Path::new(&file.path)
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with(".atomic-write-")
        })
        .map(|file| file.path.clone())
        .collect();
    observed.remaining_temps.extend(
        std::fs::read_dir(std::env::temp_dir())
            .unwrap()
            .map(|entry| format!("temp/{}", entry.unwrap().file_name().to_str().unwrap())),
    );
    observed.remaining_temps.sort();
    observed
}
struct Opener {
    accounts: AccountService,
    observed: Shared,
    clients: Arc<Mutex<Vec<Arc<Client>>>>,
    mode: String,
}
impl AccountOpener for Opener {
    fn open_client_with_proxy(
        &self,
        context: &Context,
        proxy_override: Option<&str>,
    ) -> ClientOpen<Client> {
        self.observed
            .lock()
            .unwrap()
            .proxy_overrides
            .push(proxy_override.map(str::to_owned));
        trace(&self.observed, "account.open");
        match self.mode.as_str() {
            "error" => {
                return ClientOpen {
                    client: None,
                    error: Some(SchedulerError::Message(
                        "owned account opener failure".into(),
                    )),
                };
            }
            "nil" => {
                return ClientOpen {
                    client: None,
                    error: None,
                };
            }
            "" | "client-error" => (),
            other => panic!("unknown opener fault {other}"),
        }
        let mut opened = self
            .accounts
            .open_client_with_proxy(context, proxy_override);
        if let Some(client) = &opened.client {
            let mut clients = self.clients.lock().unwrap();
            if !clients.iter().any(|other| Arc::ptr_eq(other, client)) {
                clients.push(client.clone());
            }
            self.observed.lock().unwrap().unique_clients = clients.len();
        }
        if opened.error.is_none() && self.mode == "client-error" {
            opened.error = Some(SchedulerError::Message(
                "owned account opener returned client and failure".into(),
            ));
        }
        opened
    }
}
