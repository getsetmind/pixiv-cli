use crate::CommandError;
use pixiv_app::{
    account_service::AccountService, config::Store, database::Database, lifecycle::Context,
};
use serde::Serialize;
use std::{
    io::{Read, Write},
    sync::{Arc, Mutex},
};

pub trait AccountPrompts {
    fn can_prompt(&self) -> bool;
    fn select(&mut self, message: &str, options: &[String]) -> Result<String, CommandError>;
    fn confirm(&mut self, message: &str, default: bool) -> Result<bool, CommandError>;
    fn secret(&mut self, _message: &str) -> Result<String, CommandError> {
        Err(CommandError::Message(
            "interactive prompt is only available on a TTY",
        ))
    }
}
struct Noninteractive;
impl AccountPrompts for Noninteractive {
    fn can_prompt(&self) -> bool {
        false
    }
    fn select(&mut self, _: &str, _: &[String]) -> Result<String, CommandError> {
        Err(CommandError::Message("uid is required"))
    }
    fn confirm(&mut self, _: &str, _: bool) -> Result<bool, CommandError> {
        Ok(false)
    }
}
pub enum AuthCommand {
    Transfer(crate::auth_transfer::TransferCommand),
    Validation(crate::auth_validation::ValidationCommand),
    Help(String),
    List {
        json: bool,
    },
    Status {
        json: bool,
    },
    Change {
        enabled: bool,
        all: bool,
        json: bool,
        values: Vec<String>,
    },
    Select {
        remove: bool,
        json: bool,
        yes: bool,
        value: Option<String>,
    },
    Pending(String),
}
impl AuthCommand {
    pub fn parse<R: Read>(
        args: &[String],
        input: &mut R,
        terminal: bool,
    ) -> Result<Self, CommandError> {
        if crate::auth_validation::ValidationCommand::discover(args).is_some() {
            return crate::auth_validation::ValidationCommand::parse(args, input, terminal)
                .map(Self::Validation);
        }
        if crate::auth_transfer::TransferCommand::discover(args).is_some() {
            return crate::auth_transfer::TransferCommand::parse(args, input, terminal)
                .map(Self::Transfer);
        }
        let mut values = Vec::new();
        let mut json = false;
        let mut all = false;
        let mut yes = false;
        let mut help = false;
        let mut flags = true;
        let mut discovered = Vec::new();
        let mut index = 0;
        while index < args.len() {
            let arg = &args[index];
            if arg == "--" {
                break;
            }
            if arg.starts_with('-') && arg != "-" {
                index += if arg.contains('=') { 1 } else { 2 };
            } else {
                discovered.push(arg.as_str());
                index += 1;
            }
        }
        let pool = discovered.first().is_some_and(|s| *s == "pool");
        let op = discovered.get(usize::from(pool)).copied().unwrap_or("");
        let selection = !pool && matches!(op, "use" | "remove");
        let change = pool && matches!(op, "enable" | "disable");
        let leaf = matches!(
            (pool, op),
            (false, "use")
                | (false, "remove")
                | (false, "list")
                | (true, "status")
                | (true, "enable")
                | (true, "disable")
        );
        for arg in args {
            if flags && arg == "--" {
                flags = false;
                continue;
            }
            if flags && arg.starts_with("--") {
                let (name, raw) = arg
                    .split_once('=')
                    .map_or((arg.as_str(), "true"), |(a, b)| (a, b));
                let target = match name {
                    "--help" => &mut help,
                    "--json" if leaf => &mut json,
                    "--all" if change => &mut all,
                    "--yes" if selection && op == "remove" => &mut yes,
                    _ => return Err(CommandError::Usage(format!("unknown option '{name}'"))),
                };
                *target = boolean(raw, if name == "--help" { "-h, --help" } else { name })?;
                continue;
            }
            if flags && arg.starts_with('-') && arg != "-" {
                let mut cluster = arg[1..].chars().peekable();
                while let Some(flag) = cluster.next() {
                    if flag != 'h' {
                        return Err(CommandError::Usage(format!("unknown option '-{flag}'")));
                    }
                    if cluster.peek() == Some(&'=') {
                        cluster.next();
                        help = boolean(&cluster.collect::<String>(), "-h, --help")?;
                        break;
                    }
                    help = true;
                }
                continue;
            }
            values.push(arg.clone());
        }
        let path = if pool {
            if leaf {
                format!("pool {op}")
            } else {
                "pool".into()
            }
        } else if leaf {
            op.into()
        } else {
            "".into()
        };
        if help {
            return Ok(Self::Help(help_text(&path)));
        }
        if !leaf {
            let expected = if pool { 1 } else { 0 };
            if values.len() > expected {
                if !pool
                    && matches!(
                        op,
                        "use" | "remove" | "import" | "export" | "check" | "refresh" | "login"
                    )
                {
                    return Ok(Self::Pending(op.into()));
                }
                return Err(CommandError::Message(if pool {
                    "usage: pixiv auth pool <command>"
                } else {
                    "usage: pixiv auth <command>"
                }));
            }
            return Ok(Self::Help(help_text(&path)));
        }
        let mut values = values
            .into_iter()
            .skip(if pool { 2 } else { 1 })
            .collect::<Vec<_>>();
        if selection {
            if values.len() > 1 {
                return Err(CommandError::Message(if op == "remove" {
                    "usage: pixiv auth remove [UID] [--yes]"
                } else {
                    "usage: pixiv auth use [UID]"
                }));
            }
            if values.is_empty() && !terminal {
                match crate::search::read_text_value(
                    input,
                    false,
                    "uid is required",
                    "stdin value is not valid UTF-8",
                ) {
                    Ok(value) => values.push(value),
                    Err(CommandError::Message("uid is required")) => {}
                    Err(error) => return Err(error),
                }
            }
            return Ok(Self::Select {
                remove: op == "remove",
                json,
                yes,
                value: values.into_iter().next(),
            });
        }
        if change {
            if values.is_empty() && !all && !terminal {
                match crate::search::read_text_value(
                    input,
                    false,
                    "at least one UID is required unless --all is used",
                    "stdin value is not valid UTF-8",
                ) {
                    Ok(value) => values.push(value),
                    Err(CommandError::Message(
                        "at least one UID is required unless --all is used",
                    )) => {}
                    Err(e) => return Err(e),
                }
            }
            if all && !values.is_empty() {
                return Err(CommandError::Message("--all cannot be combined with UIDs"));
            }
            if !all && values.is_empty() {
                return Err(CommandError::Message(
                    "at least one UID is required unless --all is used",
                ));
            }
            return Ok(Self::Change {
                enabled: op == "enable",
                all,
                json,
                values,
            });
        }
        if !values.is_empty() {
            return Err(CommandError::Message(if pool {
                "usage: pixiv auth pool status [--json]"
            } else {
                "usage: pixiv auth list [--json]"
            }));
        }
        Ok(if pool {
            Self::Status { json }
        } else {
            Self::List { json }
        })
    }
    pub fn machine_output(&self) -> bool {
        if let Self::Validation(command) = self {
            return command.machine_output();
        }
        if let Self::Transfer(command) = self {
            return command.machine_output();
        }
        matches!(
            self,
            Self::List { json: true }
                | Self::Status { json: true }
                | Self::Change { json: true, .. }
                | Self::Select { json: true, .. }
        )
    }
    pub fn requires_config(&self) -> bool {
        if let Self::Validation(command) = self {
            return command.requires_config();
        }
        if let Self::Transfer(command) = self {
            return command.requires_config();
        }
        matches!(
            self,
            Self::List { .. } | Self::Status { .. } | Self::Change { .. } | Self::Select { .. }
        )
    }
    pub fn execute<W: Write>(
        &self,
        store: &Store,
        context: &Context,
        out: &mut W,
    ) -> Result<(), CommandError> {
        self.execute_with_prompts(store, context, out, &mut Noninteractive)
    }
    pub fn execute_with_prompts<W: Write>(
        &self,
        store: &Store,
        context: &Context,
        out: &mut W,
        prompts: &mut dyn AccountPrompts,
    ) -> Result<(), CommandError> {
        match self {
            Self::Transfer(command) => return command.execute_offline(store, out),
            Self::Validation(crate::auth_validation::ValidationCommand::Help(text)) => {
                let _ = out.write_all(text.as_bytes());
                return Ok(());
            }
            Self::Validation(_) => {
                return Err(CommandError::Message(
                    "account validation requires an OAuth transport",
                ));
            }
            Self::Help(text) => {
                let _ = out.write_all(text.as_bytes());
                return Ok(());
            }
            Self::Pending(op) => {
                return Err(CommandError::MessageText(format!(
                    "auth {op} is not implemented in the Rust migration"
                )));
            }
            _ => {}
        }
        store
            .ensure_defaults()
            .map_err(|e| CommandError::State(Box::new(e)))?;
        let runtime = store
            .current()
            .and_then(|s| s.runtime())
            .map_err(pixiv_app::scheduler::SchedulerError::from)?;
        let database = Database::open(store.path().parent().unwrap())
            .map_err(|e| CommandError::State(Box::new(e)))?;
        let defaults = store.clone();
        let service = AccountService {
            repository: Arc::new(Mutex::new(database)),
            defaults: Some(Arc::new(move || {
                defaults.read_pixiv_default_user_id().map_err(Into::into)
            })),
        };
        if let Self::Select {
            remove,
            json,
            yes,
            value,
        } = self
        {
            return AccountSelection {
                service: &service,
                defaults: store,
            }
            .execute(
                value.as_deref(),
                SelectionOptions {
                    remove: *remove,
                    json: *json,
                    yes: *yes,
                },
                out,
                prompts,
            );
        }
        if let Self::Change {
            enabled,
            all,
            json,
            values,
        } = self
        {
            let ids = values
                .iter()
                .map(|s| uid(s))
                .collect::<Result<Vec<_>, _>>()?;
            if *all {
                service.set_all_pool_schedulable(context, *enabled)?;
            } else {
                service.set_pool_schedulable(context, &ids, *enabled)?;
            }
            if *json {
                #[derive(Serialize)]
                struct Change<'a> {
                    schedulable: bool,
                    all: bool,
                    #[serde(skip_serializing_if = "<[i64]>::is_empty")]
                    user_ids: &'a [i64],
                }
                return print_json(
                    out,
                    &Change {
                        schedulable: *enabled,
                        all: *all,
                        user_ids: &ids,
                    },
                );
            }
            if *all {
                let _ = writeln!(out, "schedulable:{enabled} all");
            } else {
                let _ = writeln!(
                    out,
                    "schedulable:{enabled} uid:{}",
                    ids.iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(",")
                );
            }
            return Ok(());
        }
        let background = Context::new();
        let summaries = service.list_accounts(if matches!(self, Self::List { .. }) {
            &background
        } else {
            context
        })?;
        // Go time.Time compares its wrapped internal epoch even when Unix seconds exceed it.
        let earliest_epoch = summaries
            .iter()
            .filter_map(|a| a.pool_frozen_until)
            .min_by_key(|t| t.wrapping_add(62135596800));
        if self.machine_output()
            && summaries
                .iter()
                .filter_map(|a| a.pool_frozen_until)
                .any(|t| !(0..253402300800).contains(&t))
        {
            return Err(CommandError::Message(
                "json: error calling MarshalJSON for type *time.Time: year outside of range [0,9999]",
            ));
        }
        let accounts = summaries
            .into_iter()
            .map(AccountOut::from)
            .collect::<Vec<_>>();
        if let Self::List { json } = self {
            if *json {
                #[derive(Serialize)]
                struct List<'a> {
                    #[serde(skip_serializing_if = "zero")]
                    default_user_id: i64,
                    accounts: Option<&'a [AccountOut]>,
                }
                return print_json(
                    out,
                    &List {
                        default_user_id: accounts
                            .iter()
                            .rfind(|a| a.default)
                            .map_or(0, |a| a.user_id),
                        accounts: if accounts.is_empty() {
                            None
                        } else {
                            Some(&accounts)
                        },
                    },
                );
            }
            if accounts.is_empty() {
                let _ = writeln!(out, "no accounts");
            }
            for a in accounts {
                let _ = write!(
                    out,
                    "{} ✓ uid:{}",
                    if a.default { "*" } else { " " },
                    a.user_id
                );
                if !a.username.is_empty() {
                    let _ = write!(out, " username:{}", a.username);
                }
                human_pool(out, &a);
                let _ = writeln!(out);
            }
        } else {
            let earliest = earliest_epoch.map(timestamp);
            if self.machine_output() {
                #[derive(Serialize)]
                struct Status<'a> {
                    enabled: bool,
                    strategy: &'a str,
                    #[serde(skip_serializing_if = "Option::is_none")]
                    earliest_frozen_until: Option<String>,
                    accounts: &'a [AccountOut],
                }
                return print_json(
                    out,
                    &Status {
                        enabled: runtime.account_pool.enabled,
                        strategy: &runtime.account_pool.strategy,
                        earliest_frozen_until: earliest,
                        accounts: &accounts,
                    },
                );
            }
            let _ = writeln!(
                out,
                "enabled:{} strategy:{}",
                runtime.account_pool.enabled, runtime.account_pool.strategy
            );
            if let Some(t) = earliest {
                let _ = writeln!(out, "earliest_frozen_until:{t}");
            }
            for a in accounts {
                let _ = write!(out, "uid:{}", a.user_id);
                human_pool(out, &a);
                let _ = writeln!(out);
            }
        }
        Ok(())
    }
}
pub struct SelectionOptions {
    pub remove: bool,
    pub json: bool,
    pub yes: bool,
}
pub struct AccountSelection<'a> {
    pub service: &'a AccountService,
    pub defaults: &'a dyn pixiv_app::account_management::AccountDefaultStore,
}
impl AccountSelection<'_> {
    pub fn execute<W: Write>(
        &self,
        value: Option<&str>,
        options: SelectionOptions,
        out: &mut W,
        prompts: &mut dyn AccountPrompts,
    ) -> Result<(), CommandError> {
        let background = Context::new();
        let SelectionOptions { remove, json, yes } = options;
        let service = self.service;
        let store = self.defaults;
        let accounts = service.list_accounts(&background)?;
        let user_id = if let Some(value) = value.filter(|v| !v.trim().is_empty()) {
            uid(value)?
        } else {
            if !prompts.can_prompt() {
                return Err(CommandError::Message("uid is required"));
            }
            if accounts.is_empty() {
                return Err(CommandError::Message("no accounts"));
            }
            let options = accounts
                .iter()
                .map(|a| {
                    if a.username.is_empty() {
                        a.user_id.to_string()
                    } else {
                        format!("{} {}", a.user_id, a.username)
                    }
                })
                .collect::<Vec<_>>();
            let selected = prompts.select(
                if remove {
                    "Select account to remove"
                } else {
                    "Select default account"
                },
                &options,
            )?;
            uid(selected.split_whitespace().next().unwrap_or(""))?
        };
        if !remove {
            service
                .management(store)
                .use_account(&background, user_id)?;
            if json {
                return print_json(out, &serde_json::json!({"default_user_id":user_id}));
            }
            let _ = writeln!(out, "default uid: {user_id}");
            return Ok(());
        }
        if prompts.can_prompt()
            && !yes
            && !prompts.confirm(&format!("Remove uid {user_id}?"), false)?
        {
            return Err(CommandError::Message("account removal canceled"));
        }
        let accounts = service.list_accounts(&background)?;
        if !accounts.iter().any(|a| a.user_id == user_id) {
            return Err(CommandError::MessageText(format!(
                "account uid {user_id} not found"
            )));
        }
        service
            .management(store)
            .remove_account(&background, user_id)?;
        let default_user_id = service
            .list_accounts(&background)?
            .iter()
            .rfind(|a| a.default)
            .map_or(0, |a| a.user_id);
        if json {
            return print_json(
                out,
                &serde_json::json!({"default_user_id":default_user_id,"removed_user_id":user_id}),
            );
        }
        let _ = writeln!(out, "account uid:{user_id} removed");
        if default_user_id != 0 {
            let _ = writeln!(out, "default uid: {default_user_id}");
        }
        Ok(())
    }
}

pub(crate) fn boolean(raw: &str, name: &str) -> Result<bool, CommandError> {
    match raw {
        "1" | "t" | "T" | "TRUE" | "true" | "True" => Ok(true),
        "0" | "f" | "F" | "FALSE" | "false" | "False" => Ok(false),
        _ => Err(CommandError::MessageText(format!(
            "invalid argument {} for {} flag: strconv.ParseBool: parsing {}: invalid syntax",
            crate::search::quote(raw),
            crate::search::quote(name),
            crate::search::quote(raw)
        ))),
    }
}
pub(crate) fn uid(raw: &str) -> Result<i64, CommandError> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(CommandError::Message("uid cannot be empty"));
    }
    s.parse::<i64>().ok().filter(|n| *n > 0).ok_or_else(|| {
        CommandError::MessageText(format!("invalid uid {}", crate::search::quote(s)))
    })
}
fn zero(value: &i64) -> bool {
    *value == 0
}
#[derive(Serialize)]
pub(crate) struct AccountOut {
    #[serde(skip_serializing_if = "zero")]
    pub(crate) user_id: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) username: String,
    default: bool,
    has_token: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) premium_status: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    schedulable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    eligible: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pool_frozen_until: Option<String>,
}
impl From<pixiv_app::account_service::AccountSummary> for AccountOut {
    fn from(a: pixiv_app::account_service::AccountSummary) -> Self {
        Self {
            user_id: a.user_id,
            username: a.username,
            default: a.default,
            has_token: true,
            premium_status: a.premium,
            schedulable: a.pool_status_known.then_some(a.schedulable),
            eligible: a.pool_status_known.then_some(a.eligible),
            pool_frozen_until: a.pool_frozen_until.map(timestamp),
        }
    }
}
fn timestamp(seconds: i64) -> String {
    let days = i128::from(seconds).div_euclid(86400);
    let remainder = i128::from(seconds).rem_euclid(86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let day_of_era = z - era * 146097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_position = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_position + 2) / 5 + 1;
    let month = month_position + if month_position < 10 { 3 } else { -9 };
    year += i128::from(month <= 2);
    let year_text = if year < 0 {
        format!("-{:04}", -year)
    } else {
        format!("{year:04}")
    };
    format!(
        "{year_text}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        remainder / 3600,
        remainder / 60 % 60,
        remainder % 60
    )
}
fn human_pool<W: Write>(out: &mut W, a: &AccountOut) {
    if let Some(s) = a.schedulable {
        let _ = write!(
            out,
            " schedulable:{s} eligible:{}",
            a.eligible.unwrap_or(false)
        );
        if let Some(t) = &a.pool_frozen_until {
            let _ = write!(out, " frozen_until:{t}");
        }
    }
}
pub(crate) fn print_json<W: Write, T: Serialize>(
    out: &mut W,
    value: &T,
) -> Result<(), CommandError> {
    let body = crate::go_json_escape(
        serde_json::to_string_pretty(value).map_err(|e| CommandError::State(Box::new(e)))?,
    );
    let _ = out.write(format!("{body}\n").as_bytes())?;
    Ok(())
}
fn help_text(path: &str) -> String {
    match path{
""=>"Manage local Pixiv authentication\n\nUsage:\n  pixiv auth [flags]\n  pixiv auth [command]\n\nAvailable Commands:\n  check            Validate an account token\n  export           Export stored authentication\n  import           Import or replace an account\n  list             List accounts\n  login            Login with the Pixiv browser OAuth flow\n  pool             Manage account pool scheduling\n  refresh          Refresh account credentials and membership status\n  remove           Remove an account\n  use              Set the default account\n\nFlags:\n  -h, --help   help for auth\n\nUse \"pixiv auth [command] --help\" for more information about a command.\n".into(),
"pool"=>"Manage account pool scheduling\n\nUsage:\n  pixiv auth pool [flags]\n  pixiv auth pool [command]\n\nAvailable Commands:\n  disable     Disable accounts in the pool\n  enable      Enable accounts in the pool\n  status      Show account pool scheduling status\n\nFlags:\n  -h, --help   help for pool\n\nUse \"pixiv auth pool [command] --help\" for more information about a command.\n".into(),
"use"=>"Set the default account\n\nUsage:\n  pixiv auth use [UID] [flags]\n\nFlags:\n  -h, --help   help for use\n      --json   print JSON\n".into(),
"remove"=>"Remove an account\n\nUsage:\n  pixiv auth remove [UID] [flags]\n\nFlags:\n  -h, --help   help for remove\n      --json   print JSON\n      --yes    skip confirmation in interactive terminals\n".into(),
"list"=>"List accounts\n\nUsage:\n  pixiv auth list [flags]\n\nFlags:\n  -h, --help   help for list\n      --json   print JSON\n".into(),
"pool status"=>"Show account pool scheduling status\n\nUsage:\n  pixiv auth pool status [flags]\n\nFlags:\n  -h, --help   help for status\n      --json   print JSON\n".into(),
"pool enable"=>"Enable accounts in the pool\n\nUsage:\n  pixiv auth pool enable [UID...] [flags]\n\nFlags:\n      --all    apply to every stored account\n  -h, --help   help for enable\n      --json   print JSON\n".into(),
"pool disable"=>"Disable accounts in the pool\n\nUsage:\n  pixiv auth pool disable [UID...] [flags]\n\nFlags:\n      --all    apply to every stored account\n  -h, --help   help for disable\n      --json   print JSON\n".into(),
_=>unreachable!(),}
}

pub fn machine_output_requested(args: &[String]) -> bool {
    if crate::auth_validation::ValidationCommand::discover(args).is_some() {
        return crate::auth_validation::machine_output_requested(args);
    }
    if let Some(op) = crate::auth_transfer::TransferCommand::discover(args) {
        return op == "import" && crate::auth_transfer::machine_output_requested(args);
    }
    let mut discovered = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            break;
        }
        if arg.starts_with('-') && arg != "-" {
            index += if arg.contains('=') { 1 } else { 2 };
        } else {
            discovered.push(arg.as_str());
            index += 1;
        }
    }
    let leaf = matches!(
        discovered.as_slice(),
        ["list" | "use" | "remove", ..] | ["pool", "status" | "enable" | "disable", ..]
    );
    if !leaf {
        return false;
    }
    let change = matches!(discovered.as_slice(), ["pool", "enable" | "disable", ..]);
    let mut changed = false;
    for arg in args {
        if arg == "--" {
            break;
        }
        if arg.starts_with("--") {
            let (name, raw) = arg
                .split_once('=')
                .map_or((arg.as_str(), "true"), |(a, b)| (a, b));
            if !(matches!(name, "--json" | "--help")
                || name == "--all" && change
                || name == "--yes" && discovered.first() == Some(&"remove"))
            {
                break;
            }
            if boolean(raw, name).is_err() {
                break;
            }
            if name == "--json" {
                changed = true;
            }
        } else if arg.starts_with('-') && arg != "-" {
            let mut cluster = arg[1..].chars().peekable();
            let mut valid = true;
            while let Some(flag) = cluster.next() {
                if flag != 'h' {
                    valid = false;
                    break;
                }
                if cluster.peek() == Some(&'=') {
                    cluster.next();
                    valid = boolean(&cluster.collect::<String>(), "-h, --help").is_ok();
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

pub(crate) fn discover_auth_operation(args: &[String]) -> Option<&str> {
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            return None;
        }
        if arg.starts_with('-') && arg != "-" {
            index += if arg.contains('=') { 1 } else { 2 };
        } else {
            return Some(arg.as_str());
        }
    }
    None
}
pub(crate) fn auth_proxy_override(
    proxy: &Option<String>,
    no_proxy: Option<bool>,
) -> Result<Option<&str>, CommandError> {
    if proxy.is_some() && no_proxy.is_some() {
        return Err(CommandError::Message(
            "use either --proxy or --no-proxy, not both",
        ));
    }
    Ok(if no_proxy == Some(true) {
        Some("")
    } else {
        proxy.as_deref()
    })
}
