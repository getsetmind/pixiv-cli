use crate::{CommandError, auth_accounts::AccountPrompts};
use pixiv_app::{
    account_service::AccountService,
    account_transfer::RestoreAccountInput,
    auth_bundle::{self, AuthExportAccount, AuthExportBundle},
    config::Store,
    database::Database,
    lifecycle::Context,
};
use pixiv_sdk::transport::Transport;
use serde::Serialize;
use std::{
    io::{Read, Write},
    path::Path,
    sync::{Arc, Mutex},
};

pub enum ImportInput {
    Token(String),
    Bundle(Vec<u8>),
    Prompt,
}
pub enum TransferCommand {
    Import {
        input: ImportInput,
        proxy: Option<String>,
        no_proxy: Option<bool>,
        json: bool,
    },
    Export {
        value: Option<String>,
        all: bool,
        output: Option<String>,
        force: bool,
    },
    Help(String),
}
impl TransferCommand {
    pub fn discover(args: &[String]) -> Option<&str> {
        let mut index = 0;
        while index < args.len() {
            let arg = &args[index];
            if arg == "--" {
                return None;
            }
            if arg.starts_with('-') && arg != "-" {
                index += if arg.contains('=') { 1 } else { 2 };
            } else {
                return matches!(arg.as_str(), "import" | "export").then_some(arg.as_str());
            }
        }
        None
    }
    pub fn parse<R: Read>(
        args: &[String],
        input: &mut R,
        terminal: bool,
    ) -> Result<Self, CommandError> {
        let op = Self::discover(args).unwrap_or("");
        let import = op == "import";
        let mut json = false;
        let mut all = false;
        let mut force = false;
        let mut help = false;
        let mut proxy = None;
        let mut no_proxy = None;
        let mut output = None;
        let mut values = Vec::new();
        let mut flags = true;
        let mut command_seen = false;
        let mut index = 0;
        while index < args.len() {
            let arg = &args[index];
            index += 1;
            if flags && arg == "--" {
                flags = false;
                continue;
            }
            if flags && arg.starts_with("--") {
                let (name, raw) = arg
                    .split_once('=')
                    .map_or((arg.as_str(), None), |(a, b)| (a, Some(b)));
                if !command_seen && name != "--help" {
                    return Err(CommandError::Usage(format!("unknown option '{name}'")));
                }
                if matches!(name, "--proxy" | "--output")
                    && ((name == "--proxy" && import) || (name == "--output" && !import))
                {
                    let value = if let Some(raw) = raw {
                        raw.to_owned()
                    } else {
                        let value = args.get(index).ok_or_else(|| {
                            CommandError::MessageText(format!("flag needs an argument: {name}"))
                        })?;
                        index += 1;
                        value.clone()
                    };
                    if name == "--proxy" {
                        proxy = Some(value);
                    } else {
                        output = Some(value);
                    }
                    continue;
                }
                let target = match name {
                    "--help" => &mut help,
                    "--json" if import => &mut json,
                    "--no-proxy" if import => {
                        no_proxy =
                            Some(crate::auth_accounts::boolean(raw.unwrap_or("true"), name)?);
                        continue;
                    }
                    "--all" if !import => &mut all,
                    "--force" if !import => &mut force,
                    _ => return Err(CommandError::Usage(format!("unknown option '{name}'"))),
                };
                *target = crate::auth_accounts::boolean(
                    raw.unwrap_or("true"),
                    if name == "--help" { "-h, --help" } else { name },
                )?;
                continue;
            }
            if flags && arg.starts_with('-') && arg != "-" {
                let mut chars = arg[1..].chars().peekable();
                while let Some(ch) = chars.next() {
                    if ch != 'h' {
                        return Err(CommandError::Usage(format!("unknown option '-{ch}'")));
                    }
                    if chars.peek() == Some(&'=') {
                        chars.next();
                        help = crate::auth_accounts::boolean(
                            &chars.collect::<String>(),
                            "-h, --help",
                        )?;
                        break;
                    }
                    help = true;
                }
                continue;
            }
            command_seen = true;
            values.push(arg.clone());
        }
        if help {
            return Ok(Self::Help(help_text(import).into()));
        }
        if !values.is_empty() {
            values.remove(0);
        }
        if import {
            let classified = if values.is_empty() && !terminal {
                check_proxy(&proxy, no_proxy)?;
                let mut body = Vec::new();
                input.read_to_end(&mut body).map_err(|e| {
                    CommandError::MessageText(format!("read auth import stdin: {e}"))
                })?;
                if body
                    .iter()
                    .copied()
                    .find(|c| !matches!(*c, b' ' | b'\t' | b'\r' | b'\n'))
                    == Some(b'{')
                {
                    if !auth_bundle::is_json(&body) {
                        return Err(CommandError::Message("invalid auth export bundle JSON"));
                    }
                    if proxy.is_some() || no_proxy.is_some() {
                        return Err(CommandError::Message(
                            "bundle import cannot be combined with --proxy or --no-proxy",
                        ));
                    }
                    Some(ImportInput::Bundle(body))
                } else {
                    Some(ImportInput::Token(read_token(&body)?))
                }
            } else {
                None
            };
            if values.len() > 1 {
                return Err(CommandError::Message(
                    "usage: pixiv auth import [REFRESH_TOKEN]",
                ));
            }
            Ok(Self::Import {
                input: classified.unwrap_or_else(|| {
                    values
                        .into_iter()
                        .next()
                        .map_or(ImportInput::Prompt, ImportInput::Token)
                }),
                proxy,
                no_proxy,
                json,
            })
        } else {
            if values.len() > 1 {
                return Err(CommandError::Message("usage: pixiv auth export [UID]"));
            }
            if values.is_empty() && !all && !terminal {
                match crate::search::read_text_value(
                    input,
                    false,
                    "uid is required",
                    "stdin value is not valid UTF-8",
                ) {
                    Ok(value) => values.push(value),
                    Err(CommandError::Message("uid is required")) => {}
                    Err(e) => return Err(e),
                }
            }
            Ok(Self::Export {
                value: values.into_iter().next(),
                all,
                output,
                force,
            })
        }
    }
    pub fn machine_output(&self) -> bool {
        matches!(self, Self::Import { json: true, .. })
    }
    pub fn requires_config(&self) -> bool {
        !matches!(self, Self::Help(_))
    }
    fn service(&self, store: &Store) -> Result<AccountService, CommandError> {
        if matches!(self, Self::Import { .. }) {
            store
                .ensure_defaults()
                .map_err(|e| CommandError::State(Box::new(e)))?;
            store
                .current()
                .and_then(|s| s.runtime())
                .map_err(pixiv_app::scheduler::SchedulerError::from)?;
        }
        let database = Database::open(store.path().parent().unwrap())
            .map_err(|e| CommandError::State(Box::new(e)))?;
        let defaults = store.clone();
        Ok(AccountService {
            repository: Arc::new(Mutex::new(database)),
            defaults: Some(Arc::new(move || {
                defaults.read_pixiv_default_user_id().map_err(Into::into)
            })),
        })
    }
    pub fn execute_offline<W: Write>(
        &self,
        store: &Store,
        out: &mut W,
    ) -> Result<(), CommandError> {
        match self {
            Self::Help(text) => {
                let _ = out.write_all(text.as_bytes());
                Ok(())
            }
            Self::Import {
                input: ImportInput::Bundle(body),
                json,
                ..
            } => {
                let service = self.service(store)?;
                let bundle =
                    auth_bundle::decode(body).map_err(|e| CommandError::State(Box::new(e)))?;
                let inputs = bundle
                    .accounts
                    .into_iter()
                    .map(|a| RestoreAccountInput {
                        account: pixiv_app::account_service::AccountSummary {
                            user_id: a.user_id,
                            username: a.username,
                            default: false,
                            premium: None,
                            schedulable: false,
                            pool_frozen_until: None,
                            pool_last_selected: false,
                            eligible: false,
                            pool_status_known: false,
                        },
                        refresh_token: a.refresh_token,
                        is_bundle_default: a.user_id == bundle.default_user_id,
                    })
                    .collect::<Vec<_>>();
                let result = service
                    .transfer(store)
                    .restore_accounts(&Context::new(), &inputs)?;
                let accounts = result
                    .accounts
                    .iter()
                    .map(|a| ImportOut {
                        user_id: a.account.user_id,
                        username: a.account.username.clone(),
                        status: if a.is_replacement { "updated" } else { "added" },
                    })
                    .collect::<Vec<_>>();
                if *json {
                    #[derive(Serialize)]
                    struct BundleOut {
                        accounts: Vec<ImportOut>,
                        default_user_id: i64,
                    }
                    return print_json(
                        out,
                        &BundleOut {
                            accounts,
                            default_user_id: result.resulting_default,
                        },
                    );
                }
                for a in accounts {
                    let _ = writeln!(out, "{} uid:{}", a.status, a.user_id);
                }
                let _ = writeln!(out, "default uid: {}", result.resulting_default);
                Ok(())
            }
            Self::Export {
                value,
                all,
                output,
                force,
            } => {
                if *force && output.is_none() {
                    return Err(CommandError::Message("--force requires --output"));
                }
                if *all && value.is_some() {
                    return Err(CommandError::Message("--all cannot be combined with a UID"));
                }
                let id = value.as_deref().map(export_uid).transpose()?;
                let service = self.service(store)?;
                let transfer = service.transfer(store);
                let background = Context::new();
                let accounts = transfer.accounts_with_tokens(&background)?;
                let id = if !*all {
                    Some(match id {
                        Some(id) => id,
                        None => transfer.current_user(&background)?.user_id,
                    })
                } else {
                    None
                };
                let selected = accounts
                    .iter()
                    .filter(|a| id.is_none_or(|id| id == a.user_id))
                    .collect::<Vec<_>>();
                if !*all && selected.is_empty() {
                    return Err(CommandError::MessageText(format!(
                        "account uid {} not found",
                        id.unwrap()
                    )));
                }
                let bundle = AuthExportBundle {
                    schema: "pixiv-cli.auth-export".into(),
                    version: 1,
                    default_user_id: selected
                        .iter()
                        .filter(|a| a.default)
                        .map(|a| a.user_id)
                        .next_back()
                        .unwrap_or(0),
                    accounts: selected
                        .iter()
                        .map(|a| AuthExportAccount {
                            user_id: a.user_id,
                            username: a.username.clone(),
                            refresh_token: a.refresh_token().to_owned(),
                        })
                        .collect(),
                };
                let body =
                    auth_bundle::encode(&bundle).map_err(|e| CommandError::State(Box::new(e)))?;
                if let Some(path) = output {
                    if path.trim().is_empty() {
                        return Err(CommandError::Message("--output requires a path"));
                    }
                    pixiv_app::secret_file::write(Path::new(path), &body, *force)
                        .map_err(|e| CommandError::State(Box::new(e)))?;
                    write_export_stdout(
                        out,
                        format!("output: {path}\naccounts: {}\n", bundle.accounts.len()).as_bytes(),
                    )
                } else if *all {
                    write_export_stdout(out, &body)
                } else {
                    let mut raw = selected[0].refresh_token_bytes().to_vec();
                    raw.push(b'\n');
                    write_export_stdout(out, &raw)
                }
            }
            _ => Err(CommandError::Message(
                "token import requires an OAuth transport",
            )),
        }
    }
    pub async fn execute_http<W: Write>(
        &self,
        store: &Store,
        context: &Context,
        out: &mut W,
        prompts: &mut dyn AccountPrompts,
    ) -> Result<(), CommandError> {
        if !matches!(
            self,
            Self::Import {
                input: ImportInput::Token(_) | ImportInput::Prompt,
                ..
            }
        ) {
            return self.execute_offline(store, out);
        }
        let prepared = self.prepare_token(store, context, prompts)?;
        let runtime = store
            .current()
            .and_then(|s| s.runtime())
            .map_err(pixiv_app::scheduler::SchedulerError::from)?;
        let connection =
            pixiv_app::connection::CommandConnection::resolve(&runtime, prepared.proxy.as_deref())
                .map_err(|e| CommandError::State(Box::new(e)))?;
        let transport = pixiv_sdk::transport::HttpTransport::new(Some(connection.proxy()))?;
        self.import_token(store, context, out, prepared, transport)
            .await
    }
    pub async fn execute_with_transport<T: Transport, W: Write>(
        &self,
        store: &Store,
        context: &Context,
        out: &mut W,
        prompts: &mut dyn AccountPrompts,
        transport: T,
    ) -> Result<(), CommandError> {
        if !matches!(
            self,
            Self::Import {
                input: ImportInput::Token(_) | ImportInput::Prompt,
                ..
            }
        ) {
            return self.execute_offline(store, out);
        }
        let prepared = self.prepare_token(store, context, prompts)?;
        let runtime = store
            .current()
            .and_then(|s| s.runtime())
            .map_err(pixiv_app::scheduler::SchedulerError::from)?;
        pixiv_app::connection::CommandConnection::resolve(&runtime, prepared.proxy.as_deref())
            .map_err(|e| CommandError::State(Box::new(e)))?;
        self.import_token(store, context, out, prepared, transport)
            .await
    }
    fn prepare_token(
        &self,
        store: &Store,
        context: &Context,
        prompts: &mut dyn AccountPrompts,
    ) -> Result<PreparedImport, CommandError> {
        let Self::Import {
            input,
            proxy,
            no_proxy,
            ..
        } = self
        else {
            unreachable!()
        };
        // Startup validates runtime before command flags, while the import service lists before constructing the OAuth client.
        store
            .ensure_defaults()
            .map_err(|e| CommandError::State(Box::new(e)))?;
        store
            .current()
            .and_then(|s| s.runtime())
            .map_err(pixiv_app::scheduler::SchedulerError::from)?;
        check_proxy(proxy, *no_proxy)?;
        let token = match input {
            ImportInput::Token(token) => token.clone(),
            ImportInput::Prompt => prompts.secret("Refresh token")?,
            ImportInput::Bundle(_) => unreachable!(),
        };
        let service = self.service(store)?;
        let token = token.trim().to_owned();
        if token.is_empty() {
            return Err(CommandError::Message("refresh token cannot be empty"));
        }
        let before = service
            .list_accounts(context)?
            .into_iter()
            .map(|a| a.user_id)
            .collect();
        Ok(PreparedImport {
            service,
            token,
            before,
            proxy: if *no_proxy == Some(true) {
                Some(String::new())
            } else {
                proxy.clone()
            },
        })
    }
    async fn import_token<T: Transport, W: Write>(
        &self,
        store: &Store,
        context: &Context,
        out: &mut W,
        prepared: PreparedImport,
        transport: T,
    ) -> Result<(), CommandError> {
        let account = prepared
            .service
            .transfer(store)
            .import_account_with(context, &prepared.token, false, transport)
            .await?;
        let result = ImportOut {
            user_id: account.user_id,
            username: account.username,
            status: if prepared.before.contains(&account.user_id) {
                "updated"
            } else {
                "added"
            },
        };
        if self.machine_output() {
            return print_json(out, &result);
        }
        let _ = writeln!(out, "{} uid:{}", result.status, result.user_id);
        if !result.username.is_empty() {
            let _ = writeln!(out, "username:{}", result.username);
        }
        Ok(())
    }
}
struct PreparedImport {
    service: AccountService,
    token: String,
    before: Vec<i64>,
    proxy: Option<String>,
}
#[derive(Serialize)]
struct ImportOut {
    user_id: i64,
    username: String,
    status: &'static str,
}
fn check_proxy(proxy: &Option<String>, no_proxy: Option<bool>) -> Result<(), CommandError> {
    if proxy.is_some() && no_proxy.is_some() {
        Err(CommandError::Message(
            "use either --proxy or --no-proxy, not both",
        ))
    } else {
        Ok(())
    }
}
fn read_token(body: &[u8]) -> Result<String, CommandError> {
    let text = String::from_utf8_lossy(body);
    let token = text
        .strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(&text);
    if token.contains(['\r', '\n']) {
        return Err(CommandError::Message(
            "refresh token input must contain exactly one line",
        ));
    }
    if token.is_empty() {
        return Err(CommandError::Message("refresh token cannot be empty"));
    }
    Ok(token.into())
}
fn export_uid(value: &str) -> Result<i64, CommandError> {
    value
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or(CommandError::Message("uid must be a positive integer"))
}
fn print_json<W: Write, T: Serialize>(out: &mut W, value: &T) -> Result<(), CommandError> {
    let body = crate::go_json_escape(
        serde_json::to_string_pretty(value).map_err(|e| CommandError::State(Box::new(e)))?,
    );
    let _ = out.write(format!("{body}\n").as_bytes())?;
    Ok(())
}
#[derive(Debug)]
struct ExportStdoutError(std::io::Error);
impl std::fmt::Display for ExportStdoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("write stdout failed")
    }
}
impl std::error::Error for ExportStdoutError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}
pub fn write_export_stdout<W: Write>(out: &mut W, body: &[u8]) -> Result<(), CommandError> {
    let error = match out.write(body) {
        Ok(n) if n == body.len() => return Ok(()),
        Ok(_) => std::io::Error::new(std::io::ErrorKind::WriteZero, "short write"),
        Err(error) => error,
    };
    Err(CommandError::State(Box::new(ExportStdoutError(error))))
}

fn help_text(import: bool) -> &'static str {
    if import {
        "Import or replace an account\n\nUsage:\n  pixiv auth import [REFRESH_TOKEN] [flags]\n\nExamples:\npixiv auth import YOUR_REFRESH_TOKEN\n\nFlags:\n  -h, --help           help for import\n      --json           print JSON\n      --no-proxy       clear the configured proxy for this command\n      --proxy string   proxy URL (http, https, socks5, or socks5h) for this command\n"
    } else {
        "Export stored authentication\n\nUsage:\n  pixiv auth export [UID] [flags]\n\nFlags:\n      --all             export all stored accounts; cannot be combined with UID\n      --force           replace an existing output file; requires --output\n  -h, --help            help for export\n      --output string   write a versioned authentication bundle to PATH\n"
    }
}

pub fn machine_output_requested(args: &[String]) -> bool {
    let mut changed = false;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        index += 1;
        if arg == "--" {
            break;
        }
        if arg.starts_with("--") {
            let (name, raw) = arg
                .split_once('=')
                .map_or((arg.as_str(), None), |(a, b)| (a, Some(b)));
            if name == "--proxy" {
                if raw.is_none() {
                    if index == args.len() {
                        break;
                    }
                    index += 1;
                }
                continue;
            }
            if !matches!(name, "--json" | "--help" | "--no-proxy") {
                break;
            }
            if crate::auth_accounts::boolean(raw.unwrap_or("true"), name).is_err() {
                break;
            }
            if name == "--json" {
                changed = true;
            }
        } else if arg.starts_with('-') && arg != "-" {
            let mut chars = arg[1..].chars().peekable();
            let mut valid = true;
            while let Some(ch) = chars.next() {
                if ch != 'h' {
                    valid = false;
                    break;
                }
                if chars.peek() == Some(&'=') {
                    chars.next();
                    valid = crate::auth_accounts::boolean(&chars.collect::<String>(), "-h, --help")
                        .is_ok();
                    break;
                }
            }
            if !valid {
                break;
            }
        }
    }
    changed
}
