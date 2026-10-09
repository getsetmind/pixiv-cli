use crate::{
    CommandError,
    auth_accounts::{AccountOut, boolean, print_json, uid},
};
use pixiv_app::{
    account_service::AccountService, config::Store, database::Database, lifecycle::Context,
};
use pixiv_sdk::transport::{HttpTransport, Transport};
use serde::Serialize;
use std::{
    io::{Read, Write},
    sync::{Arc, Mutex},
};

pub enum ValidationCommand {
    Run {
        refresh: bool,
        value: Option<String>,
        all: bool,
        proxy: Option<String>,
        no_proxy: Option<bool>,
        json: bool,
    },
    Help(String),
}
impl ValidationCommand {
    pub fn discover(args: &[String]) -> Option<&str> {
        crate::auth_accounts::discover_auth_operation(args)
            .filter(|op| matches!(*op, "check" | "refresh"))
    }
    pub fn parse<R: Read>(
        args: &[String],
        input: &mut R,
        terminal: bool,
    ) -> Result<Self, CommandError> {
        let refresh = Self::discover(args) == Some("refresh");
        let (mut json, mut all, mut help) = (false, false, false);
        let (mut proxy, mut no_proxy) = (None, None);
        let mut values = Vec::new();
        let (mut flags, mut command_seen, mut index) = (true, false, 0);
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
                if name == "--proxy" {
                    proxy = Some(if let Some(raw) = raw {
                        raw.to_owned()
                    } else {
                        let value = args.get(index).ok_or_else(|| {
                            CommandError::MessageText(format!("flag needs an argument: {name}"))
                        })?;
                        index += 1;
                        value.clone()
                    });
                    continue;
                }
                let target = match name {
                    "--help" => &mut help,
                    "--json" => &mut json,
                    "--all" if refresh => &mut all,
                    "--no-proxy" => {
                        no_proxy = Some(boolean(raw.unwrap_or("true"), name)?);
                        continue;
                    }
                    _ => return Err(CommandError::Usage(format!("unknown option '{name}'"))),
                };
                *target = boolean(
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
                        help = boolean(&chars.collect::<String>(), "-h, --help")?;
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
            return Ok(Self::Help(help_text(refresh)));
        }
        if !values.is_empty() {
            values.remove(0);
        }
        if values.len() > 1 {
            return Err(CommandError::Message(if refresh {
                "usage: pixiv auth refresh [UID] [--all]"
            } else {
                "usage: pixiv auth check [UID]"
            }));
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
        Ok(Self::Run {
            refresh,
            value: values.into_iter().next(),
            all,
            proxy,
            no_proxy,
            json,
        })
    }
    pub fn machine_output(&self) -> bool {
        matches!(self, Self::Run { json: true, .. })
    }
    pub fn requires_config(&self) -> bool {
        matches!(self, Self::Run { .. })
    }
    pub async fn execute_http<W: Write>(
        &self,
        store: &Store,
        context: &Context,
        out: &mut W,
    ) -> Result<(), CommandError> {
        self.execute_with_factory(store, context, out, |proxy| {
            HttpTransport::new(Some(proxy)).map_err(Into::into)
        })
        .await
    }
    pub async fn execute_with_transport<T: Transport + Clone, W: Write>(
        &self,
        store: &Store,
        context: &Context,
        out: &mut W,
        transport: T,
    ) -> Result<(), CommandError> {
        self.execute_with_factory(store, context, out, |_| Ok(transport.clone()))
            .await
    }
    pub async fn execute_with_factory<
        T: Transport,
        W: Write,
        F: FnMut(&str) -> Result<T, CommandError>,
    >(
        &self,
        store: &Store,
        context: &Context,
        out: &mut W,
        mut factory: F,
    ) -> Result<(), CommandError> {
        let Self::Run {
            refresh,
            value,
            all,
            proxy,
            no_proxy,
            json,
        } = self
        else {
            let Self::Help(text) = self else {
                unreachable!()
            };
            let _ = out.write_all(text.as_bytes());
            return Ok(());
        };
        store
            .ensure_defaults()
            .map_err(|e| CommandError::State(Box::new(e)))?;
        let startup_runtime = store
            .current()
            .and_then(|s| s.runtime())
            .map_err(pixiv_app::scheduler::SchedulerError::from)?;
        if *all && value.is_some() {
            return Err(CommandError::Message("--all cannot be combined with a UID"));
        }
        let requested_id = value.as_deref().map(uid).transpose()?.unwrap_or(0);
        let override_proxy = crate::auth_accounts::auth_proxy_override(proxy, *no_proxy)?;
        let db = Database::open(store.path().parent().unwrap())
            .map_err(|e| CommandError::State(Box::new(e)))?;
        let defaults = store.clone();
        let service = AccountService {
            repository: Arc::new(Mutex::new(db)),
            defaults: Some(Arc::new(move || {
                defaults.read_pixiv_default_user_id().map_err(Into::into)
            })),
        };
        let ids = if *all {
            let ids = service
                .list_accounts(&Context::new())?
                .into_iter()
                .map(|a| a.user_id)
                .collect::<Vec<_>>();
            if ids.is_empty() {
                return Err(CommandError::Message("no accounts"));
            }
            ids
        } else {
            vec![requested_id]
        };
        let mut results = Vec::new();
        let mut invalid_time = false;
        for id in ids {
            let id = if id == 0 {
                service.transfer(store).current_user(context)?.user_id
            } else {
                id
            };
            let runtime = if override_proxy.is_some() {
                startup_runtime.clone()
            } else {
                store
                    .current()
                    .and_then(|s| s.runtime())
                    .map_err(pixiv_app::scheduler::SchedulerError::from)?
            };
            let connection =
                pixiv_app::connection::CommandConnection::resolve(&runtime, override_proxy)
                    .map_err(|e| CommandError::State(Box::new(e)))?;
            let transport = factory(connection.proxy())?;
            let summary = if *refresh {
                service
                    .validation(store)
                    .refresh_account_with(context, id, transport)
                    .await?
            } else {
                service.check_account_with(context, id, transport).await?
            };
            if *json
                && summary
                    .pool_frozen_until
                    .is_some_and(|t| !(0..253402300800).contains(&t))
            {
                invalid_time = true;
            }
            results.push(AccountOut::from(summary));
        }
        if invalid_time {
            return Err(CommandError::Message(
                "json: error calling MarshalJSON for type *time.Time: year outside of range [0,9999]",
            ));
        }
        if *json {
            if *refresh {
                #[derive(Serialize)]
                struct Results<'a> {
                    accounts: &'a [AccountOut],
                }
                print_json(out, &Results { accounts: &results })
            } else {
                print_json(out, &results[0])
            }
        } else {
            for result in results {
                if *refresh {
                    let premium = result
                        .premium_status
                        .map_or("unknown", |p| if p { "yes" } else { "no" });
                    let _ = writeln!(out, "✓ refreshed uid:{} premium:{premium}", result.user_id);
                } else {
                    let _ = if requested_id == 0 {
                        writeln!(out, "token ok, uid:{}", result.user_id)
                    } else {
                        writeln!(out, "account uid:{} ok", result.user_id)
                    };
                    if !result.username.is_empty() {
                        let _ = writeln!(out, "username:{}", result.username);
                    }
                }
            }
            Ok(())
        }
    }
}
fn help_text(refresh: bool) -> String {
    let (op, title) = if refresh {
        (
            "refresh",
            "Refresh account credentials and membership status",
        )
    } else {
        ("check", "Validate an account token")
    };
    let all = if refresh {
        "      --all            refresh every stored account\n"
    } else {
        ""
    };
    format!(
        "{title}\n\nUsage:\n  pixiv auth {op} [UID] [flags]\n\nFlags:\n{all}  -h, --help           help for {op}\n      --json           print JSON\n      --no-proxy       clear the configured proxy for this command\n      --proxy string   proxy URL (http, https, socks5, or socks5h) for this command\n"
    )
}
pub fn machine_output_requested(args: &[String]) -> bool {
    let refresh = ValidationCommand::discover(args) == Some("refresh");
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
            if !(matches!(name, "--json" | "--help" | "--no-proxy") || refresh && name == "--all") {
                break;
            }
            if boolean(raw.unwrap_or("true"), name).is_err() {
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
                    valid = boolean(&chars.collect::<String>(), "-h, --help").is_ok();
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
