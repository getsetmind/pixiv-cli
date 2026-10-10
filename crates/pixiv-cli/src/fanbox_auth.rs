pub use crate::fanbox_browser::{
    BrowserCookiesFuture, BrowserProfilesFuture, CookieProfile, CookieProvider,
    SystemBrowserProvider,
};
use crate::{CommandError, auth_accounts::AccountPrompts};
use pixiv_app::{
    fanbox_account_service::{AccountService, AccountSummary},
    lifecycle::Context,
};
use serde::Serialize;
use std::{
    future::Future,
    io::{Read, Write},
    pin::Pin,
    sync::Arc,
};
pub type BrowserFuture<'a> =
    Pin<Box<dyn Future<Output = Result<String, CommandError>> + Send + 'a>>;
pub trait BrowserProvider: Send + Sync {
    fn read_session<'a>(
        &'a self,
        context: &'a Context,
        browser: &'a str,
        profile: &'a str,
    ) -> BrowserFuture<'a>;
}
pub struct Data<'a, R, W, F> {
    pub service_factory: F,
    pub reader: &'a mut R,
    pub writer: &'a mut W,
    pub prompts: &'a mut dyn AccountPrompts,
    pub browser: &'a dyn BrowserProvider,
}
pub struct UpdateRuntime {
    pub enabled: bool,
    pub https_proxy: String,
}
pub trait AutomaticUpdateHooks {
    fn runtime(&self) -> Result<UpdateRuntime, CommandError>;
    fn check(&self, context: &Context, proxy: &str) -> Result<(), CommandError>;
}
pub struct AuthCommand {
    leaf: String,
    values: Vec<String>,
    json: bool,
    machine: bool,
    auto: bool,
    yes: bool,
    default: bool,
    browser: String,
    profile: String,
    proxy: Option<String>,
    no_proxy: Option<bool>,
    help: Option<String>,
    help_changed: bool,
}
impl AuthCommand {
    pub fn parse(args: &[String]) -> Result<Self, CommandError> {
        let normalized = if crate::fanbox::is_fanbox_route(args) {
            args.to_vec()
        } else {
            std::iter::once("fanbox".into())
                .chain(std::iter::once("auth".into()))
                .chain(args.iter().cloned())
                .collect()
        };
        let route = crate::fanbox::command_route(&normalized);
        let leaf = route
            .path
            .last()
            .filter(|s| s.as_str() != "auth")
            .cloned()
            .unwrap_or_default();
        let mut command = Self {
            leaf,
            values: vec![],
            json: false,
            machine: false,
            auto: false,
            yes: false,
            default: false,
            browser: String::new(),
            profile: String::new(),
            proxy: None,
            no_proxy: None,
            help: None,
            help_changed: false,
        };
        let mut index = 0;
        let mut positional = false;
        let mut help = false;
        while index < route.args.len() {
            let arg = &route.args[index];
            index += 1;
            if arg == "--" && !positional {
                positional = true;
                continue;
            }
            if positional || !arg.starts_with('-') || arg == "-" {
                command.values.push(arg.clone());
                continue;
            }
            let (flag, attached) = arg
                .split_once('=')
                .map_or((arg.as_str(), None), |(a, b)| (a, Some(b)));
            let boolean = matches!(
                flag,
                "--help" | "-h" | "--json" | "--auto" | "--yes" | "--default" | "--no-proxy"
            );
            let known = matches!(flag, "--help" | "-h" | "--proxy" | "--no-proxy")
                || flag == "--json" && !command.leaf.is_empty()
                || matches!(flag, "--default" | "--from-browser" | "--profile")
                    && command.leaf == "import"
                || flag == "--auto" && command.leaf == "use"
                || flag == "--yes" && command.leaf == "remove";
            if !known {
                return Err(CommandError::Usage(format!("unknown option '{flag}'")));
            }
            let value = if boolean {
                attached.unwrap_or("true")
            } else if let Some(v) = attached {
                v
            } else {
                let v = route.args.get(index).ok_or_else(|| {
                    CommandError::MessageText(format!("flag needs an argument: {flag}"))
                })?;
                index += 1;
                v
            };
            match flag {
                "--help" | "-h" => {
                    command.help_changed = true;
                    help = crate::fanbox::parse_bool(value, flag)?;
                }
                "--json" => {
                    command.machine = true;
                    command.json = crate::fanbox::parse_bool(value, flag)?;
                }
                "--auto" => command.auto = crate::fanbox::parse_bool(value, flag)?,
                "--yes" => command.yes = crate::fanbox::parse_bool(value, flag)?,
                "--default" => command.default = crate::fanbox::parse_bool(value, flag)?,
                "--no-proxy" => command.no_proxy = Some(crate::fanbox::parse_bool(value, flag)?),
                "--proxy" => command.proxy = Some(value.into()),
                "--from-browser" => command.browser = value.into(),
                "--profile" => command.profile = value.into(),
                _ => unreachable!(),
            }
        }
        if help || command.leaf.is_empty() && command.values.is_empty() {
            command.help = crate::fanbox::help_route(&normalized)?;
        }
        Ok(command)
    }
    pub fn machine_requested(&self) -> bool {
        self.machine
    }
    pub fn requires_startup(&self) -> bool {
        self.help.is_none() && !self.leaf.is_empty()
    }
    fn usage(&self) -> &'static str {
        match self.leaf.as_str() {
            "import" => {
                "usage: pixiv fanbox auth import [--from-browser BROWSER] [--profile ID] [--default]"
            }
            "list" => "usage: pixiv fanbox auth list [--json]",
            "status" => "usage: pixiv fanbox auth status [UID]",
            "use" => "usage: pixiv fanbox auth use [UID] | --auto",
            "remove" => "usage: pixiv fanbox auth remove UID",
            _ => "usage: pixiv fanbox auth <command>",
        }
    }
    pub fn resolve_input<R: Read>(
        &mut self,
        input: &mut R,
        terminal: bool,
    ) -> Result<(), CommandError> {
        if self.help.is_some() {
            return Ok(());
        }
        let max = usize::from(matches!(self.leaf.as_str(), "status" | "use" | "remove"));
        if self.values.len() > max {
            return Err(CommandError::Message(self.usage()));
        }
        if max == 1 && self.values.is_empty() && !terminal {
            match crate::search::read_text_value(
                input,
                false,
                "uid is required",
                "stdin value is not valid UTF-8",
            ) {
                Ok(v) => self.values.push(v),
                Err(CommandError::Message("uid is required")) => {}
                Err(e) => return Err(e),
            }
        }
        if self.leaf == "remove" && self.values.len() != 1 {
            return Err(CommandError::Message(self.usage()));
        }
        Ok(())
    }
    fn proxy_override(&self) -> Result<Option<&str>, CommandError> {
        if self.proxy.is_some() && self.no_proxy.is_some() {
            return Err(CommandError::Message(
                "use either --proxy or --no-proxy, not both",
            ));
        }
        Ok(if self.no_proxy == Some(true) {
            Some("")
        } else {
            self.proxy.as_deref()
        })
    }
    pub async fn execute<R: Read, W: Write, F>(
        &self,
        context: &Context,
        data: Data<'_, R, W, F>,
    ) -> Result<(), CommandError>
    where
        F: FnOnce() -> Result<Option<Arc<AccountService>>, CommandError>,
    {
        if let Some(help) = &self.help {
            if let Some((description, rest)) = help.split_once("\n\n") {
                let _ = data.writer.write(format!("{description}\n").as_bytes());
                let _ = data.writer.write(b"\n");
                let _ = data.writer.write(rest.as_bytes());
            } else {
                let _ = data.writer.write(help.as_bytes());
            }
            return Ok(());
        }
        let background = Context::background();
        let factory = || {
            (data.service_factory)()?.ok_or(CommandError::Message(
                "fanbox is not available: cannot open the local account store",
            ))
        };
        match self.leaf.as_str() {
            "import" => {
                let value = if !self.browser.is_empty() {
                    data.browser
                        .read_session(context, &self.browser, &self.profile)
                        .await?
                        .into_bytes()
                } else if data.prompts.can_prompt() {
                    data.prompts.secret("FANBOXSESSID")?.into_bytes()
                } else {
                    read_session(data.reader)?
                };
                let service = factory()?;
                let proxy = self.proxy_override()?;
                let summary = service
                    .import_session_bytes_with_proxy(context, &value, self.default, proxy)
                    .await
                    .map_err(CommandError::App)?;
                if self.json {
                    json(data.writer, &SafeAccount::from(&summary))?;
                } else {
                    line(data.writer, format!("imported uid:{}\n", summary.user_id));
                    identity(data.writer, &summary);
                }
                Ok(())
            }
            "list" => {
                self.proxy_override()?;
                let service = factory()?;
                let accounts = service
                    .list_accounts(&background)
                    .map_err(CommandError::App)?;
                if self.json {
                    let accounts = accounts.iter().map(SafeAccount::from).collect::<Vec<_>>();
                    let default_user_id =
                        accounts.iter().find(|s| s.default).map_or(0, |s| s.user_id);
                    json(
                        data.writer,
                        &AccountList {
                            default_user_id,
                            accounts,
                        },
                    )?;
                } else if accounts.is_empty() {
                    line(data.writer, "no accounts\n".into());
                } else {
                    for s in accounts {
                        line(
                            data.writer,
                            format!("{} uid:{}", if s.default { "*" } else { " " }, s.user_id),
                        );
                        if !s.display_name.is_empty() {
                            line(data.writer, format!(" display:{}", s.display_name));
                        }
                        if !s.creator_id.is_empty() {
                            line(data.writer, format!(" creator:{}", s.creator_id));
                        }
                        line(data.writer, "\n".into());
                    }
                }
                Ok(())
            }
            "status" => {
                self.proxy_override()?;
                let service = factory()?;
                let summary = if let Some(value) = self.values.first() {
                    let id = uid(value)?;
                    service
                        .list_accounts(&background)
                        .map_err(CommandError::App)?
                        .into_iter()
                        .find(|s| s.user_id == id)
                        .ok_or_else(|| {
                            CommandError::MessageText(format!("fanbox account uid:{id} not found"))
                        })?
                } else {
                    service.status(&background).map_err(CommandError::App)?
                };
                if self.json {
                    json(data.writer, &SafeAccount::from(&summary))?;
                } else {
                    line(data.writer, format!("uid:{}\n", summary.user_id));
                    identity(data.writer, &summary);
                    line(
                        data.writer,
                        format!("default:{}\n", if summary.default { "yes" } else { "no" }),
                    );
                }
                Ok(())
            }
            "use" => {
                if self.auto && !self.values.is_empty() {
                    return Err(CommandError::Message(
                        "--auto cannot be combined with a UID",
                    ));
                }
                let service = factory()?;
                let id = if self.auto {
                    service.use_auto().map_err(CommandError::App)?;
                    0
                } else {
                    let value = self.values.first().ok_or(CommandError::Message(
                        "usage: pixiv fanbox auth use UID | --auto",
                    ))?;
                    let id = uid(value)?;
                    service
                        .use_account(&background, id)
                        .map_err(CommandError::App)?;
                    id
                };
                if self.json {
                    json(
                        data.writer,
                        &serde_json::json!({"default_user_id":id,"auto":self.auto}),
                    )?;
                } else {
                    line(
                        data.writer,
                        if self.auto {
                            "default uid: auto\n".into()
                        } else {
                            format!("default uid: {id}\n")
                        },
                    );
                }
                Ok(())
            }
            "remove" => {
                let id = uid(&self.values[0])?;
                let service = factory()?;
                if data.prompts.can_prompt()
                    && !self.yes
                    && !data
                        .prompts
                        .confirm(&format!("Remove fanbox uid {id}?"), false)?
                {
                    return Err(CommandError::Message("fanbox account removal canceled"));
                }
                service
                    .remove_account(&background, id)
                    .map_err(CommandError::App)?;
                if self.json {
                    json(data.writer, &serde_json::json!({"removed_user_id":id}))?;
                } else {
                    line(data.writer, format!("account uid:{id} removed\n"));
                }
                Ok(())
            }
            _ => Err(CommandError::Message(self.usage())),
        }
    }
    pub fn post_success<W: Write>(
        &self,
        context: &Context,
        release: bool,
        hooks: &dyn AutomaticUpdateHooks,
        diagnostics: &mut W,
    ) {
        if !release || self.help_changed || self.help.is_some() || self.leaf.is_empty() {
            return;
        }
        run_automatic_update(
            context,
            self.proxy.as_deref(),
            self.no_proxy,
            hooks,
            diagnostics,
        );
    }
}
pub(crate) fn run_automatic_update<W: Write>(
    context: &Context,
    proxy: Option<&str>,
    no_proxy: Option<bool>,
    hooks: &dyn AutomaticUpdateHooks,
    diagnostics: &mut W,
) {
    let warning = |out: &mut W, action: &str, error: CommandError| {
        let _ = out.write(format!("warning: {action}: {error}\n").as_bytes());
    };
    let runtime = match hooks.runtime() {
        Ok(v) => v,
        Err(e) => {
            warning(diagnostics, "load automatic update configuration", e);
            return;
        }
    };
    if !runtime.enabled {
        return;
    }
    if proxy.is_some() && no_proxy.is_some() {
        warning(
            diagnostics,
            "read automatic update proxy override",
            CommandError::Message("use either --proxy or --no-proxy, not both"),
        );
        return;
    }
    let proxy = if no_proxy.is_some() {
        ""
    } else {
        proxy.unwrap_or(&runtime.https_proxy)
    };
    if let Err(e) = hooks.check(context, proxy) {
        warning(diagnostics, "create automatic update checker", e);
    }
}

fn uid(value: &str) -> Result<i64, CommandError> {
    value
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or(CommandError::Message(
            "fanbox uid must be a positive integer",
        ))
}
fn read_session<R: Read>(input: &mut R) -> Result<Vec<u8>, CommandError> {
    let mut bytes = Vec::with_capacity(512);
    input.read_to_end(&mut bytes)?;
    if bytes.ends_with(b"\r\n") {
        bytes.truncate(bytes.len() - 2);
    } else if bytes.ends_with(b"\n") {
        bytes.pop();
    }
    if bytes.iter().any(|b| matches!(b, b'\r' | b'\n')) {
        return Err(CommandError::Message(
            "FANBOX session input must contain exactly one line",
        ));
    }
    if bytes.is_empty() {
        return Err(CommandError::Message(
            "FANBOX session value cannot be empty",
        ));
    }
    Ok(bytes)
}
fn line<W: Write>(out: &mut W, value: String) {
    let _ = out.write(value.as_bytes());
}
fn identity<W: Write>(out: &mut W, s: &AccountSummary) {
    if !s.display_name.is_empty() {
        line(out, format!("display:{}\n", s.display_name));
    }
    if !s.creator_id.is_empty() {
        line(out, format!("creator:{}\n", s.creator_id));
    }
}
#[derive(Serialize)]
struct SafeAccount {
    user_id: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    display_name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    creator_id: String,
    default: bool,
}
impl From<&AccountSummary> for SafeAccount {
    fn from(s: &AccountSummary) -> Self {
        Self {
            user_id: s.user_id,
            display_name: s.display_name.clone(),
            creator_id: s.creator_id.clone(),
            default: s.default,
        }
    }
}
#[derive(Serialize)]
struct AccountList {
    #[serde(skip_serializing_if = "zero")]
    default_user_id: i64,
    accounts: Vec<SafeAccount>,
}
fn zero(id: &i64) -> bool {
    *id == 0
}
fn json<W: Write, T: Serialize>(out: &mut W, value: &T) -> Result<(), CommandError> {
    let encoded = serde_json::to_string_pretty(value).map_err(std::io::Error::other)?;
    let _written = out.write(format!("{}\n", crate::go_json_escape(encoded)).as_bytes())?;
    Ok(())
}
pub fn prepare_root<R: Read, W: Write>(
    context: &Context,
    args: &[String],
    reader: &mut R,
    terminal: bool,
    hooks: &dyn crate::startup::StartupHooks,
    diagnostics: &mut W,
) -> Result<AuthCommand, CommandError> {
    let mut command = AuthCommand::parse(args)?;
    command.resolve_input(reader, terminal)?;
    if command.requires_startup() {
        crate::startup::run_startup(context, hooks, diagnostics)?;
    }
    Ok(command)
}
