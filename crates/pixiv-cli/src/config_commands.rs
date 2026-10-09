use crate::CommandError;
use pixiv_app::config::{ConfigError, Store, cli_setting_aliases, public_setting_text};
use std::io::{Read, Write};

pub enum ConfigCommand {
    Help(String),
    Path,
    Get(String),
    Set(String, Option<String>),
    Unset(String),
}

fn managed(alias: &str) -> bool {
    cli_setting_aliases().contains(&alias)
}
fn removed(alias: &str) -> bool {
    matches!(alias, "web_fallback_enabled" | "account_pool_accounts")
}
fn validate_key(alias: &str, allow_removed: bool) -> Result<(), CommandError> {
    if removed(alias) {
        if allow_removed {
            return Ok(());
        }
        return Err(CommandError::MessageText(
            ConfigError::Removed(if alias == "web_fallback_enabled" {
                "web_fallback_enabled"
            } else {
                "account_pool_accounts"
            })
            .to_string(),
        ));
    }
    if !managed(alias) {
        return Err(CommandError::MessageText(format!(
            "unknown config key {}. valid keys: {}",
            crate::search::quote(alias),
            cli_setting_aliases().join(", ")
        )));
    }
    Ok(())
}

impl ConfigCommand {
    pub fn parse<R: Read>(
        args: &[String],
        input: &mut R,
        terminal: bool,
    ) -> Result<Self, CommandError> {
        let mut positional = Vec::new();
        let mut help = false;
        let mut flags = true;
        for arg in args {
            if flags && arg == "--" {
                flags = false;
                continue;
            }
            if flags && arg == "--help" {
                help = true;
                continue;
            }
            if flags && arg.starts_with("--help=") {
                help = parse_help_bool(arg.split_once('=').unwrap().1)?;
                continue;
            }
            if flags && arg.starts_with("--") {
                let name = arg.split('=').next().unwrap_or(arg);
                return Err(CommandError::Usage(format!("unknown option '{name}'")));
            }
            if flags && arg.starts_with('-') && arg != "-" {
                let mut cluster = arg[1..].chars().peekable();
                while let Some(flag) = cluster.next() {
                    if flag != 'h' {
                        return Err(CommandError::Usage(format!("unknown option '-{flag}'")));
                    }
                    if cluster.peek() == Some(&'=') {
                        cluster.next();
                        help = parse_help_bool(&cluster.collect::<String>())?;
                        break;
                    }
                    help = true;
                }
                continue;
            }
            positional.push(arg.clone());
        }
        let operation = positional.first().map(String::as_str).unwrap_or("");
        if help || !matches!(operation, "path" | "get" | "set" | "unset") {
            return Ok(Self::Help(help_text(
                if matches!(operation, "path" | "get" | "set" | "unset") {
                    operation
                } else {
                    ""
                },
            )));
        }
        let mut values = positional[1..].to_vec();
        let usage = match operation {
            "path" => "usage: pixiv config path",
            "get" => "usage: pixiv config get KEY",
            "unset" => "usage: pixiv config unset KEY",
            _ => "usage: pixiv config set KEY [VALUE]",
        };
        let sensitive =
            operation == "set" && values.first().is_some_and(|key| key == "saucenao_api_key");
        let fill = if operation == "set" { 1 } else { 0 };
        if operation != "path" && !sensitive && !terminal && values.len() == fill {
            match crate::search::read_text_value(
                input,
                false,
                usage,
                "stdin value is not valid UTF-8",
            ) {
                Ok(value) => values.push(value),
                Err(CommandError::Message(message)) if message == usage => {}
                Err(error) => return Err(error),
            }
        }
        if (operation == "path" && !values.is_empty())
            || (matches!(operation, "get" | "unset") && values.len() != 1)
            || (operation == "set" && !(1..=2).contains(&values.len()))
        {
            return Err(CommandError::Message(usage));
        }
        match operation {
            "path" => Ok(Self::Path),
            "get" => Ok(Self::Get(values.remove(0))),
            "unset" => Ok(Self::Unset(values.remove(0))),
            _ => {
                let key = values.remove(0);
                validate_key(&key, false)?;
                if sensitive {
                    if !values.is_empty() {
                        return Err(CommandError::Message(
                            "sensitive config values must be provided through non-TTY stdin",
                        ));
                    }
                    if terminal {
                        return Err(CommandError::Message(
                            "sensitive config values require non-TTY stdin",
                        ));
                    }
                } else if values.is_empty() {
                    return Err(CommandError::Message(
                        "VALUE is required for this config key",
                    ));
                }
                Ok(Self::Set(key, values.pop()))
            }
        }
    }
    pub fn requires_config(&self) -> bool {
        !matches!(self, Self::Help(_))
    }
    pub fn execute<R: Read, W: Write, E: Write>(
        &self,
        store: &Store,
        input: &mut R,
        out: &mut W,
        diagnostics: &mut E,
    ) -> Result<(), CommandError> {
        let state = |error| CommandError::State(Box::new(error));
        if let Self::Help(text) = self {
            let _ = out.write_all(text.as_bytes());
            return Ok(());
        }
        store.ensure_defaults().map_err(state)?;
        match self {
            Self::Help(_) => unreachable!(),
            Self::Path => {
                writeln!(out, "{}", store.path().display())?;
            }
            Self::Get(key) => {
                validate_key(key, false)?;
                let value = store.get(key).map_err(state)?;
                if key == "saucenao_api_key" {
                    writeln!(out, "<redacted>")?;
                } else if value.value.is_none() {
                    let _ = writeln!(out, "<unset>");
                    return Err(CommandError::Message("config value is unset"));
                } else {
                    writeln!(out, "{}", public_setting_text(key, &value.text))?;
                }
            }
            Self::Set(key, value) => {
                let raw = if let Some(value) = value {
                    value.clone()
                } else {
                    let mut body = Vec::new();
                    input.read_to_end(&mut body).map_err(|error| {
                        CommandError::MessageText(format!(
                            "read sensitive config value from stdin: {error}"
                        ))
                    })?;
                    if body.ends_with(b"\r\n") {
                        body.truncate(body.len() - 2);
                    } else if body.ends_with(b"\n") {
                        body.pop();
                    }
                    String::from_utf8(body)
                        .map_err(|_| CommandError::Message("stdin value is not valid UTF-8"))?
                };
                let result = store.set(key, &raw).map_err(state)?;
                if result.has_override {
                    override_note(diagnostics, key, &result.env_override);
                }
                writeln!(out, "{key} updated")?;
            }
            Self::Unset(key) => {
                validate_key(key, true)?;
                let result = store.unset(key).map_err(state)?;
                if result.has_override {
                    override_note(diagnostics, key, &result.env_override);
                }
                writeln!(out, "{key} removed")?;
            }
        }
        Ok(())
    }
}
fn override_note<W: Write>(out: &mut W, key: &str, value: &str) {
    if matches!(key, "https_proxy" | "saucenao_api_key") {
        let _ = writeln!(
            out,
            "note: {key} is currently overridden by environment; effective value remains controlled by environment"
        );
    } else {
        let _ = writeln!(
            out,
            "note: {key} is currently overridden by environment and effective value remains {}",
            crate::search::quote(value)
        );
    }
}
fn help_text(operation: &str) -> String {
    if operation.is_empty() {
        return "Manage global Pixiv CLI settings\n\nUsage:\n  pixiv config [flags]\n  pixiv config [command]\n\nAvailable Commands:\n  get         Print one effective config value\n  path        Print the config.toml path\n  set         Set one config value in config.toml\n  unset       Remove one config value from config.toml\n\nFlags:\n  -h, --help   help for config\n\nUse \"pixiv config [command] --help\" for more information about a command.\n".into();
    }
    let (description, usage) = match operation {
        "path" => ("Print the config.toml path".to_owned(), "path"),
        "get" => (
            format!(
                "Print one effective config value. KEY must be one of: {}",
                cli_setting_aliases().join(", ")
            ),
            "get KEY",
        ),
        "set" => (
            format!(
                "Set one config value in config.toml. KEY must be one of: {}",
                cli_setting_aliases().join(", ")
            ),
            "set KEY [VALUE]",
        ),
        _ => (
            format!(
                "Remove one config value from config.toml. KEY must be one of: {}",
                cli_setting_aliases().join(", ")
            ),
            "unset KEY",
        ),
    };
    format!(
        "{description}\n\nUsage:\n  pixiv config {usage} [flags]\n\nFlags:\n  -h, --help   help for {operation}\n"
    )
}

fn parse_help_bool(raw: &str) -> Result<bool, CommandError> {
    match raw {
        "1" | "t" | "T" | "TRUE" | "true" | "True" => Ok(true),
        "0" | "f" | "F" | "FALSE" | "false" | "False" => Ok(false),
        _ => Err(CommandError::MessageText(format!(
            "invalid argument {} for \"-h, --help\" flag: strconv.ParseBool: parsing {}: invalid syntax",
            crate::search::quote(raw),
            crate::search::quote(raw)
        ))),
    }
}
