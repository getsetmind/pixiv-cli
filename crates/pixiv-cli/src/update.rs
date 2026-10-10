use crate::{
    CommandError,
    startup::{StartupHooks, run_startup},
};
use pixiv_app::{
    lifecycle::Context,
    update::{
        BuildInfo, CallerContext,
        coordinator::{
            AutomaticChecker, AutomaticRequest, Coordinator, UpdateRequest, UpdateResult,
        },
    },
};
use std::{error::Error, fmt, io::Write};

const USAGE: &str = "usage: pixiv update [--check] [--prerelease] [--proxy URL]";

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UpdateOptions {
    pub check: bool,
    pub include_prerelease: bool,
    pub json: bool,
    pub json_changed: bool,
    pub proxy: Option<String>,
    pub help_changed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UpdateCommand {
    Help(String),
    Run(UpdateOptions),
}
impl UpdateCommand {
    pub fn parse(args: &[String]) -> Result<Self, CommandError> {
        Self::parse_tracking_json(args, &mut false)
    }

    pub fn machine_output_requested(args: &[String]) -> bool {
        let mut changed = false;
        let _ = Self::parse_tracking_json(args, &mut changed);
        changed
    }

    fn parse_tracking_json(args: &[String], json_changed: &mut bool) -> Result<Self, CommandError> {
        let mut options = UpdateOptions::default();
        let (mut flags, mut help, mut index) = (true, false, 0);
        let mut positional = Vec::new();
        while index < args.len() {
            let argument = &args[index];
            index += 1;
            if flags && argument == "--" {
                flags = false;
                continue;
            }
            if flags && argument.starts_with("--") {
                let (name, raw) = argument
                    .split_once('=')
                    .map_or((argument.as_str(), None), |(name, raw)| (name, Some(raw)));
                if name == "--proxy" {
                    let value = if let Some(value) = raw {
                        value.to_owned()
                    } else {
                        let value = args.get(index).ok_or_else(|| {
                            CommandError::MessageText("flag needs an argument: --proxy".into())
                        })?;
                        index += 1;
                        value.clone()
                    };
                    options.proxy = Some(value);
                    continue;
                }
                let label = if name == "--help" { "-h, --help" } else { name };
                if !matches!(name, "--check" | "--prerelease" | "--json" | "--help") {
                    return Err(CommandError::Usage(format!("unknown option '{name}'")));
                }
                let value = boolean(raw.unwrap_or("true"), label)?;
                match name {
                    "--check" => options.check = value,
                    "--prerelease" => options.include_prerelease = value,
                    "--json" => {
                        options.json = value;
                        options.json_changed = true;
                        *json_changed = true;
                    }
                    "--help" => {
                        help = value;
                        options.help_changed = true;
                    }
                    _ => unreachable!(),
                }
                continue;
            }
            if flags && argument.starts_with('-') && argument != "-" {
                let mut cluster = argument[1..].chars().peekable();
                while let Some(flag) = cluster.next() {
                    if flag != 'h' {
                        return Err(CommandError::Usage(format!("unknown option '-{flag}'")));
                    }
                    help = if cluster.peek() == Some(&'=') {
                        cluster.next();
                        boolean(&cluster.collect::<String>(), "-h, --help")?
                    } else {
                        options.help_changed = true;
                        help = true;
                        continue;
                    };
                    options.help_changed = true;
                    break;
                }
                continue;
            }
            positional.push(argument);
        }
        if help {
            return Ok(Self::Help(help_text().into()));
        }
        if !positional.is_empty() {
            return Err(CommandError::Message(USAGE));
        }
        validate(&options)?;
        Ok(Self::Run(options))
    }

    pub fn requires_startup(&self) -> bool {
        matches!(self, Self::Run(_))
    }

    pub fn machine_output(&self) -> bool {
        matches!(self, Self::Run(options) if options.json_changed)
    }

    pub async fn execute<W: Write + ?Sized, E: Write + ?Sized>(
        &self,
        context: CallerContext,
        build_info: BuildInfo,
        host: &dyn UpdateHost,
        output: &mut W,
        _diagnostics: &mut E,
    ) -> Result<(), CommandError> {
        let Self::Run(options) = self else {
            if let Self::Help(text) = self {
                if let Some(index) = text.find("\n\n") {
                    let _ = output.write(&text.as_bytes()[..index + 1]);
                    let _ = output.write(b"\n");
                    let _ = output.write(&text.as_bytes()[index + 2..]);
                } else {
                    let _ = output.write(text.as_bytes());
                }
            }
            return Ok(());
        };
        validate(options)?;
        let runtime = host.load_update_runtime_config().map_err(|cause| {
            CommandError::State(Box::new(UpdateCommandError {
                action: "load update configuration",
                cause,
            }))
        })?;
        let proxy = options.proxy.as_deref().unwrap_or(&runtime.https_proxy);
        let coordinator = host.new_update_coordinator(proxy)?;
        let result = coordinator
            .execute(
                context,
                UpdateRequest {
                    build_info,
                    check: options.check,
                    include_prerelease: options.include_prerelease,
                },
            )
            .await
            .map_err(|error| CommandError::State(Box::new(error)))?;
        write_result(&result, options.json, output)
    }

    pub async fn execute_with_startup<W: Write, E: Write>(
        &self,
        context: CallerContext,
        build_info: BuildInfo,
        host: &dyn UpdateHost,
        startup: UpdateStartup<'_>,
        output: &mut W,
        diagnostics: &mut E,
    ) -> Result<(), CommandError> {
        if self.requires_startup() {
            run_startup(startup.context, startup.hooks, diagnostics)?;
            startup.preparation.ensure_update_config()?;
            startup.preparation.start_update_diagnostics()?;
        }
        self.execute(context, build_info, host, output, diagnostics)
            .await
    }
}

pub struct UpdateStartup<'a> {
    pub context: &'a Context,
    pub hooks: &'a dyn StartupHooks,
    pub preparation: &'a dyn UpdatePreparation,
}

pub trait UpdatePreparation {
    fn ensure_update_config(&self) -> Result<(), CommandError>;
    fn start_update_diagnostics(&self) -> Result<(), CommandError>;
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UpdateRuntime {
    pub https_proxy: String,
}

pub trait UpdateHost {
    fn load_update_runtime_config(&self) -> Result<UpdateRuntime, CommandError>;
    fn new_update_coordinator(&self, proxy: &str) -> Result<Coordinator, CommandError>;
}

fn validate(options: &UpdateOptions) -> Result<(), CommandError> {
    if options.json && !options.check {
        return Err(CommandError::Message(
            "--json is only supported with --check",
        ));
    }
    Ok(())
}

fn boolean(value: &str, flag: &str) -> Result<bool, CommandError> {
    match value {
        "true" | "True" | "TRUE" | "t" | "T" | "1" => Ok(true),
        "false" | "False" | "FALSE" | "f" | "F" | "0" => Ok(false),
        _ => Err(CommandError::MessageText(format!(
            "invalid argument {} for {} flag: strconv.ParseBool: parsing {}: invalid syntax",
            crate::search::quote(value),
            crate::search::quote(flag),
            crate::search::quote(value),
        ))),
    }
}

pub fn help_text() -> &'static str {
    "Check for or install updates\n\nUsage:\n  pixiv update [flags]\n\nExamples:\npixiv update --check\n\nFlags:\n      --check          check for an update without installing it\n  -h, --help           help for update\n      --json           print update check status as JSON (requires --check)\n      --prerelease     include prerelease updates\n      --proxy string   HTTP(S) proxy URL for this update command\n"
}

pub fn write_result<W: Write + ?Sized>(
    result: &UpdateResult,
    json: bool,
    output: &mut W,
) -> Result<(), CommandError> {
    let body = if json {
        let encoded = serde_json::to_string_pretty(result)
            .map_err(|error| CommandError::State(Box::new(error)))?;
        format!("{}\n", crate::go_json_escape(encoded))
    } else {
        format!(
            "source: {}\ncurrent version: {}\nlatest version: {}\nupdate available: {}\n",
            result.source,
            result.current_version,
            result.latest_version.as_deref().unwrap_or("none"),
            if result.update_available { "yes" } else { "no" },
        )
    };
    // Go の短い書き込みはエラーがなければ成功なので write_all を使わない。
    let _ = output.write(body.as_bytes())?;
    Ok(())
}

#[derive(Debug)]
struct UpdateCommandError {
    action: &'static str,
    cause: CommandError,
}
impl fmt::Display for UpdateCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.action, self.cause)
    }
}
impl Error for UpdateCommandError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.cause)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AutomaticCommand {
    pub names: Vec<String>,
    pub has_subcommands: bool,
    pub help_changed: bool,
    pub skip_automatic_update: bool,
    pub proxy: Option<String>,
    pub no_proxy_changed: bool,
}
impl AutomaticCommand {
    pub fn leaf(names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            names: names.into_iter().map(Into::into).collect(),
            ..Self::default()
        }
    }
}

pub fn should_check(command: &AutomaticCommand, build_info: &BuildInfo) -> bool {
    if build_info.is_development()
        || command.names.len() <= 1
        || command.has_subcommands
        || command.help_changed
    {
        return false;
    }
    let path = command.names.join(" ");
    if path == "pixiv auth export" || (path == "pixiv auth import" && command.skip_automatic_update)
    {
        return false;
    }
    !command
        .names
        .iter()
        .any(|name| matches!(name.as_str(), "help" | "mcp" | "update" | "_callback"))
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AutomaticRuntime {
    pub enabled: bool,
    pub https_proxy: String,
}

pub trait AutomaticCheckHost {
    fn load_automatic_update_runtime_config(&self) -> Result<AutomaticRuntime, CommandError>;
    fn new_automatic_update_checker(&self, proxy: &str) -> Result<AutomaticChecker, CommandError>;
}

pub async fn run_automatic_check<W: Write + ?Sized>(
    command: &AutomaticCommand,
    context: CallerContext,
    build_info: BuildInfo,
    host: &dyn AutomaticCheckHost,
    diagnostics: &mut W,
) {
    if !should_check(command, &build_info) {
        return;
    }
    let runtime = match host.load_automatic_update_runtime_config() {
        Ok(runtime) => runtime,
        Err(error) => {
            warning(diagnostics, "load automatic update configuration", &error);
            return;
        }
    };
    if !runtime.enabled {
        return;
    }
    if command.proxy.is_some() && command.no_proxy_changed {
        warning(
            diagnostics,
            "read automatic update proxy override",
            &CommandError::Message("use either --proxy or --no-proxy, not both"),
        );
        return;
    }
    let proxy = if command.no_proxy_changed {
        ""
    } else {
        command.proxy.as_deref().unwrap_or(&runtime.https_proxy)
    };
    let checker = match host.new_automatic_update_checker(proxy) {
        Ok(checker) => checker,
        Err(error) => {
            warning(diagnostics, "create automatic update checker", &error);
            return;
        }
    };
    match checker
        .check(context, AutomaticRequest { build_info })
        .await
    {
        Ok(Some(notice)) => {
            let body = format!(
                "update available: {} -> {}\nrun: {}\n",
                notice.current_version, notice.latest_version, notice.update_command
            );
            let _ = diagnostics.write(body.as_bytes());
        }
        Ok(None) => {}
        Err(error) => warning(diagnostics, "check for updates", error.as_ref()),
    }
}

pub async fn finish_with_automatic_check<W: Write + ?Sized>(
    result: Result<(), CommandError>,
    command: &AutomaticCommand,
    context: CallerContext,
    build_info: BuildInfo,
    host: &dyn AutomaticCheckHost,
    diagnostics: &mut W,
) -> Result<(), CommandError> {
    if result.is_ok() {
        run_automatic_check(command, context, build_info, host, diagnostics).await;
    }
    result
}

fn warning<W: Write + ?Sized>(output: &mut W, action: &str, error: &(dyn Error + 'static)) {
    let body = format!("warning: {action}: {error}\n");
    let _ = output.write(body.as_bytes());
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PostSuccessPolicy {
    pub skip_automatic_update: bool,
}

pub type PostSuccessFuture<'a> = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + 'a>>;
pub type PostSuccess<'a> = Option<&'a dyn Fn(PostSuccessPolicy) -> PostSuccessFuture<'a>>;

pub async fn notify_post_success(
    result: Result<(), CommandError>,
    callback: PostSuccess<'_>,
    policy: PostSuccessPolicy,
) -> Result<(), CommandError> {
    if result.is_ok()
        && let Some(callback) = callback
    {
        callback(policy).await;
    }
    result
}
