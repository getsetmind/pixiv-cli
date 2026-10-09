mod document;
mod duration;
mod initialization;
mod mutations;
pub(crate) mod private_file;
#[cfg(windows)]
pub(crate) mod private_replace_windows;

pub use duration::parse as parse_duration;

pub use mutations::{
    ConfigMutationResult, cli_setting_aliases, public_setting_text, valid_setting_aliases,
};

use crate::facade::PoolConfig;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;
use toml::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrivateWriteOutcome {
    Unknown,
    NotCommitted,
    Committed,
}

pub enum ConfigError {
    Io(std::io::Error),
    Syntax(toml::de::Error),
    Invalid(String),
    Removed(&'static str),
    Joined(Vec<ConfigError>),
    Context(Box<ConfigError>, String),
    PrivateWrite(PrivateWriteOutcome, Box<ConfigError>),
    Operation(&'static str, Box<ConfigError>),
}

impl ConfigError {
    pub fn private_write_outcome(&self) -> Option<PrivateWriteOutcome> {
        match self {
            Self::PrivateWrite(outcome, _) => Some(*outcome),
            Self::Context(error, _) | Self::Operation(_, error) => error.private_write_outcome(),
            Self::Joined(errors) => errors.iter().find_map(Self::private_write_outcome),
            _ => None,
        }
    }

    pub fn is_removed(&self) -> bool {
        match self {
            Self::Removed(_) => true,
            Self::Context(error, _) | Self::PrivateWrite(_, error) | Self::Operation(_, error) => {
                error.is_removed()
            }
            _ => false,
        }
    }
}

impl fmt::Debug for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(formatter),
            Self::Syntax(error) => {
                let message = error.message();
                if message.starts_with("duplicate key") {
                    formatter.write_str("toml: key path is already defined")
                } else if message.contains("expected `]`") {
                    formatter.write_str("toml: expected character ]")
                } else {
                    write!(formatter, "toml: {message}")
                }
            }
            Self::Invalid(message) => formatter.write_str(message),
            Self::Context(error, suffix) => write!(formatter, "{error}{suffix}"),
            Self::PrivateWrite(_, error) => fmt::Display::fmt(error, formatter),
            Self::Operation(operation, error) => write!(formatter, "{operation}: {error}"),
            Self::Joined(errors) => {
                for (index, error) in errors.iter().enumerate() {
                    if index > 0 {
                        formatter.write_str("\n")?;
                    }
                    fmt::Display::fmt(error, formatter)?;
                }
                Ok(())
            }
            Self::Removed(alias) => write!(
                formatter,
                "removed_setting: config key {alias:?} was removed; clear it with `pixiv config unset {alias}`"
            ),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Syntax(error) => Some(error),
            Self::Context(error, _) | Self::PrivateWrite(_, error) | Self::Operation(_, error) => {
                Some(error.as_ref())
            }
            Self::Joined(errors) => errors.first().map(|error| error as &dyn std::error::Error),
            _ => None,
        }
    }
}

#[derive(Clone)]
pub enum Scalar {
    String(String),
    Bool(bool),
    Duration(i64),
}

#[derive(Clone)]
pub struct SettingValue {
    pub value: Option<Scalar>,
    pub text: String,
    pub source: &'static str,
}

#[derive(Clone)]
pub struct PixivNetworkConfig {
    pub proxy_url: Option<String>,
}

#[derive(Clone)]
pub struct ServiceNetworkConfig {
    pub proxy_url: Option<String>,
    pub user_agent: Option<String>,
}

#[derive(Clone)]
pub struct FlareSolverrConfig {
    pub url: String,
    pub proxy_url: String,
}

#[derive(Clone)]
pub struct RuntimeConfig {
    pub download_path: String,
    pub filename_template: String,
    pub directory_template: String,
    pub https_proxy: String,
    pub request_interval: Duration,
    pub log_level: String,
    pub log_format: String,
    pub pixiv_network: PixivNetworkConfig,
    pub fanbox_network: ServiceNetworkConfig,
    pub reverse_search_network: ServiceNetworkConfig,
    pub fanbox_flaresolverr: Option<FlareSolverrConfig>,
    pub reverse_search_flaresolverr: Option<FlareSolverrConfig>,
    pub output_json: bool,
    pub update_check_enabled: bool,
    pub login_open_browser: bool,
    pub login_use_after_login: bool,
    pub reverse_search_provider: String,
    pub reverse_search_pixiv_only: bool,
    pub saucenao_api_key: String,
    pub login_relay_public_url: String,
    pub login_relay_listen_addr: String,
    pub login_relay_tls_cert_file: String,
    pub login_relay_tls_key_file: String,
    pub account_pool: PoolConfig,
}

#[derive(Clone)]
pub struct Store {
    path: PathBuf,
}

impl Store {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn ensure_defaults(&self) -> Result<(), ConfigError> {
        initialization::ensure(&self.path)
    }

    pub fn current(&self) -> Result<Snapshot, ConfigError> {
        self.current_with_environment(SPECS.iter().flat_map(|spec| spec.environment).filter_map(
            |key| {
                std::env::var_os(key)
                    .map(|value| ((*key).into(), value.to_string_lossy().into_owned()))
            },
        ))
    }

    pub fn current_with_environment(
        &self,
        environment: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Snapshot, ConfigError> {
        let body = match std::fs::read(&self.path) {
            Ok(body) => body,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(ConfigError::Io(error)),
        };
        let text = std::str::from_utf8(&body)
            .map_err(|_| ConfigError::Invalid("toml: invalid UTF-8".into()))?;
        Snapshot::parse(text, environment)
    }

    pub fn read_pixiv_default_user_id(&self) -> Result<Option<i64>, ConfigError> {
        self.current()?.pixiv_default_user_id()
    }

    pub fn read_fanbox_default_user_id(&self) -> Result<Option<i64>, ConfigError> {
        self.current()?.fanbox_default_user_id()
    }
}

#[derive(Clone)]
pub struct Snapshot {
    file: toml::Table,
    environment: BTreeMap<String, String>,
}

#[derive(Clone, Copy)]
enum Kind {
    String,
    Bool,
    Duration,
    Removed,
}

struct Spec {
    alias: &'static str,
    path: &'static str,
    kind: Kind,
    environment: &'static [&'static str],
    default: Option<&'static str>,
    cli_managed: bool,
    sensitive: bool,
}

const SPECS: &[Spec] = &[
    Spec {
        alias: "download_path",
        path: "download.path",
        kind: Kind::String,
        environment: &["DOWNLOAD_PATH"],
        default: Some("./downloads"),
        cli_managed: true,
        sensitive: false,
    },
    Spec {
        alias: "filename_template",
        path: "download.filename_template",
        kind: Kind::String,
        environment: &["FILENAME_TEMPLATE"],
        default: Some("{author} - {title}_{id}"),
        cli_managed: true,
        sensitive: false,
    },
    Spec {
        alias: "directory_template",
        path: "download.directory_template",
        kind: Kind::String,
        environment: &["DIRECTORY_TEMPLATE"],
        default: None,
        cli_managed: true,
        sensitive: false,
    },
    Spec {
        alias: "https_proxy",
        path: "network.https_proxy",
        kind: Kind::String,
        environment: &["https_proxy", "HTTPS_PROXY"],
        default: None,
        cli_managed: true,
        sensitive: false,
    },
    Spec {
        alias: "request_interval",
        path: "network.request_interval",
        kind: Kind::Duration,
        environment: &["PIXIV_REQUEST_INTERVAL"],
        default: Some("0s"),
        cli_managed: true,
        sensitive: false,
    },
    Spec {
        alias: "log_level",
        path: "logging.level",
        kind: Kind::String,
        environment: &["PIXIV_LOG_LEVEL"],
        default: Some("info"),
        cli_managed: true,
        sensitive: false,
    },
    Spec {
        alias: "log_format",
        path: "logging.format",
        kind: Kind::String,
        environment: &["PIXIV_LOG_FORMAT"],
        default: Some("text"),
        cli_managed: true,
        sensitive: false,
    },
    Spec {
        alias: "output_json",
        path: "output.json",
        kind: Kind::Bool,
        environment: &[],
        default: Some("false"),
        cli_managed: false,
        sensitive: false,
    },
    Spec {
        alias: "web_fallback_enabled",
        path: "web.fallback_enabled",
        kind: Kind::Removed,
        environment: &[],
        default: None,
        cli_managed: false,
        sensitive: false,
    },
    Spec {
        alias: "update_check_enabled",
        path: "update.check_enabled",
        kind: Kind::Bool,
        environment: &[],
        default: Some("true"),
        cli_managed: false,
        sensitive: false,
    },
    Spec {
        alias: "login_open_browser",
        path: "login.open_browser",
        kind: Kind::Bool,
        environment: &[],
        default: Some("true"),
        cli_managed: false,
        sensitive: false,
    },
    Spec {
        alias: "login_use_after_login",
        path: "login.use_after_login",
        kind: Kind::Bool,
        environment: &[],
        default: Some("false"),
        cli_managed: false,
        sensitive: false,
    },
    Spec {
        alias: "reverse_search_provider",
        path: "reverse_search.provider",
        kind: Kind::String,
        environment: &[],
        default: Some("saucenao"),
        cli_managed: true,
        sensitive: false,
    },
    Spec {
        alias: "reverse_search_pixiv_only",
        path: "reverse_search.pixiv_only",
        kind: Kind::Bool,
        environment: &[],
        default: Some("true"),
        cli_managed: true,
        sensitive: false,
    },
    Spec {
        alias: "saucenao_api_key",
        path: "reverse_search.saucenao_api_key",
        kind: Kind::String,
        environment: &["SAUCENAO_API_KEY"],
        default: None,
        cli_managed: true,
        sensitive: true,
    },
    Spec {
        alias: "login_relay_public_url",
        path: "login.relay_public_url",
        kind: Kind::String,
        environment: &[],
        default: None,
        cli_managed: false,
        sensitive: false,
    },
    Spec {
        alias: "login_relay_listen_addr",
        path: "login.relay_listen_addr",
        kind: Kind::String,
        environment: &[],
        default: None,
        cli_managed: false,
        sensitive: false,
    },
    Spec {
        alias: "login_relay_tls_cert_file",
        path: "login.relay_tls_cert_file",
        kind: Kind::String,
        environment: &[],
        default: None,
        cli_managed: false,
        sensitive: false,
    },
    Spec {
        alias: "login_relay_tls_key_file",
        path: "login.relay_tls_key_file",
        kind: Kind::String,
        environment: &[],
        default: None,
        cli_managed: false,
        sensitive: false,
    },
    Spec {
        alias: "account_pool_enabled",
        path: "account_pool.enabled",
        kind: Kind::Bool,
        environment: &[],
        default: Some("false"),
        cli_managed: true,
        sensitive: false,
    },
    Spec {
        alias: "account_pool_strategy",
        path: "account_pool.strategy",
        kind: Kind::String,
        environment: &[],
        default: Some("round_robin"),
        cli_managed: true,
        sensitive: false,
    },
    Spec {
        alias: "account_pool_accounts",
        path: "account_pool.accounts",
        kind: Kind::Removed,
        environment: &[],
        default: None,
        cli_managed: false,
        sensitive: false,
    },
];

impl Snapshot {
    pub fn parse(
        body: &str,
        environment: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self, ConfigError> {
        let file = if body.trim().is_empty() {
            toml::Table::new()
        } else {
            body.parse().map_err(ConfigError::Syntax)?
        };
        Ok(Self {
            file,
            environment: environment.into_iter().collect(),
        })
    }

    fn get(&self, path: &str) -> Option<&Value> {
        let mut parts = path.split('.');
        let mut value = self.file.get(parts.next()?)?;
        for part in parts {
            value = value.as_table()?.get(part)?;
        }
        Some(value)
    }

    pub fn effective(&self, alias: &str) -> Result<SettingValue, ConfigError> {
        let spec = SPECS
            .iter()
            .find(|spec| spec.alias == alias)
            .ok_or_else(|| ConfigError::Invalid(format!("unknown config key {alias:?}")))?;
        let unset = |source| SettingValue {
            value: None,
            text: String::new(),
            source,
        };
        if matches!(spec.kind, Kind::Removed) {
            return if self.get(spec.path).is_some() {
                Err(ConfigError::Removed(spec.alias))
            } else {
                Ok(unset("unset"))
            };
        }
        let environment = spec
            .environment
            .iter()
            .find_map(|key| self.environment.get(*key));
        let raw_environment = environment.cloned().map(Value::String);
        let raw_default = spec.default.map(|value| Value::String(value.into()));
        let (raw, source) = if let Some(raw) = raw_environment.as_ref() {
            (raw, "env")
        } else if let Some(raw) = self.get(spec.path) {
            (raw, "file")
        } else if let Some(raw) = raw_default.as_ref() {
            (raw, "default")
        } else {
            return Ok(unset("unset"));
        };
        let (value, text) = match spec.kind {
            Kind::String => {
                let text = string_value(raw);
                let choices: &[&str] = match spec.alias {
                    "log_level" => &["info", "debug"],
                    "log_format" => &["text", "json"],
                    "reverse_search_provider" => {
                        &["saucenao", "ascii2d-color", "ascii2d-bovw", "all"]
                    }
                    _ => &[],
                };
                let text = if choices.is_empty() {
                    text
                } else {
                    let trimmed = text.trim();
                    if !choices.contains(&trimmed) {
                        return Err(ConfigError::Invalid(format!(
                            "{alias} must be one of: {}",
                            choices.join(", ")
                        )));
                    }
                    trimmed.into()
                };
                if text.is_empty() && spec.default.is_none() && source != "default" {
                    return Ok(unset(source));
                }
                (Scalar::String(text.clone()), text)
            }
            Kind::Bool => {
                let value = match raw {
                    Value::Boolean(value) => *value,
                    Value::String(value) => match value.trim() {
                        "1" | "t" | "T" | "TRUE" | "true" | "True" => true,
                        "0" | "f" | "F" | "FALSE" | "false" | "False" => false,
                        value => {
                            return Err(ConfigError::Invalid(format!(
                                "{alias}: strconv.ParseBool: parsing {value:?}: invalid syntax"
                            )));
                        }
                    },
                    _ => {
                        return Err(ConfigError::Invalid(format!(
                            "{alias}: expects bool, got {}",
                            type_name(raw)
                        )));
                    }
                };
                (Scalar::Bool(value), value.to_string())
            }
            Kind::Duration => {
                let text = raw.as_str().ok_or_else(|| {
                    ConfigError::Invalid(format!(
                        "{alias}: expects duration string, got {}",
                        type_name(raw)
                    ))
                })?;
                let value = duration::parse(text.trim())
                    .map_err(|message| ConfigError::Invalid(format!("{alias}: {message}")))?;
                if value < 0 {
                    return Err(ConfigError::Invalid(
                        "request_interval must not be negative".into(),
                    ));
                }
                (Scalar::Duration(value), duration::format(value as u64))
            }
            Kind::Removed => unreachable!(),
        };
        Ok(SettingValue {
            value: Some(value),
            text,
            source,
        })
    }

    fn string(&self, alias: &str) -> Result<String, ConfigError> {
        Ok(self.effective(alias)?.text)
    }

    fn boolean(&self, alias: &str) -> Result<bool, ConfigError> {
        match self.effective(alias)?.value {
            Some(Scalar::Bool(value)) => Ok(value),
            _ => unreachable!(),
        }
    }

    fn optional_string(&self, path: &str) -> Result<Option<String>, ConfigError> {
        self.get(path)
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| ConfigError::Invalid(format!("{path} must be a string")))
            })
            .transpose()
    }

    fn network(&self, service: &str) -> Result<ServiceNetworkConfig, ConfigError> {
        Ok(ServiceNetworkConfig {
            proxy_url: self.optional_string(&format!("{service}.network.proxy_url"))?,
            user_agent: self.optional_string(&format!("{service}.network.user_agent"))?,
        })
    }

    fn solver(&self, service: &str) -> Result<Option<FlareSolverrConfig>, ConfigError> {
        let table = format!("{service}.flaresolverr");
        let path = format!("{table}.url");
        let url = self.optional_string(&path)?;
        let proxy = self.optional_string(&format!("{table}.proxy_url"))?;
        if url.is_none() && proxy.is_none() {
            return Ok(None);
        }
        let url = url
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                ConfigError::Invalid(format!("{path} must be set when {table} is configured"))
            })?;
        Ok(Some(FlareSolverrConfig {
            url,
            proxy_url: proxy.unwrap_or_default(),
        }))
    }

    fn pool(&self) -> Result<PoolConfig, ConfigError> {
        self.effective("account_pool_accounts")?;
        let enabled = match self.get("account_pool.enabled") {
            None => false,
            Some(Value::Boolean(value)) => *value,
            Some(_) => {
                return Err(ConfigError::Invalid(
                    "account_pool.enabled must be a boolean".into(),
                ));
            }
        };
        let strategy = self
            .get("account_pool.strategy")
            .map_or(Some("round_robin"), Value::as_str)
            .map(str::trim)
            .filter(|value| matches!(*value, "round_robin" | "random"))
            .ok_or_else(|| {
                ConfigError::Invalid(
                    "account_pool.strategy must be one of: round_robin, random".into(),
                )
            })?;
        Ok(PoolConfig {
            enabled,
            strategy: strategy.into(),
        })
    }

    fn default_user_id(&self, service: &str) -> Result<Option<i64>, ConfigError> {
        let path = format!("{service}.auth.default_user_id");
        let Some(raw) = self.get(&path) else {
            return Ok(None);
        };
        let id = match raw {
            Value::Integer(value) if *value > 0 => Some(*value),
            Value::Float(value)
                if *value > 0.0
                    && *value < 9223372036854775808.0
                    && *value == (*value as i64) as f64 =>
            {
                Some(*value as i64)
            }
            _ => None,
        };
        id.map(Some).ok_or_else(|| {
            ConfigError::Invalid(format!("config: {path} must be a positive integer"))
        })
    }

    pub fn pixiv_default_user_id(&self) -> Result<Option<i64>, ConfigError> {
        self.default_user_id("pixiv")
    }
    pub fn fanbox_default_user_id(&self) -> Result<Option<i64>, ConfigError> {
        self.default_user_id("fanbox")
    }

    pub fn runtime(&self) -> Result<RuntimeConfig, ConfigError> {
        let download_path = self.string("download_path")?;
        let filename_template = self.string("filename_template")?;
        let directory_template = self.string("directory_template")?;
        let https_proxy = self.string("https_proxy")?;
        let request_interval = match self.effective("request_interval")?.value {
            Some(Scalar::Duration(value)) => Duration::from_nanos(value as u64),
            _ => unreachable!(),
        };
        let log_level = self.string("log_level")?;
        let log_format = self.string("log_format")?;
        let output_json = self.boolean("output_json")?;
        self.effective("web_fallback_enabled")?;
        let update_check_enabled = self.boolean("update_check_enabled")?;
        let login_open_browser = self.boolean("login_open_browser")?;
        let login_use_after_login = self.boolean("login_use_after_login")?;
        let reverse_search_provider = self.string("reverse_search_provider")?;
        let reverse_search_pixiv_only = self.boolean("reverse_search_pixiv_only")?;
        let saucenao_api_key = self.string("saucenao_api_key")?;
        let login_relay_public_url = self.string("login_relay_public_url")?;
        let login_relay_listen_addr = self.string("login_relay_listen_addr")?;
        let login_relay_tls_cert_file = self.string("login_relay_tls_cert_file")?;
        let login_relay_tls_key_file = self.string("login_relay_tls_key_file")?;
        let account_pool = self.pool()?;
        let pixiv_network = PixivNetworkConfig {
            proxy_url: self.optional_string("pixiv.network.proxy_url")?,
        };
        let fanbox_network = self.network("fanbox")?;
        let reverse_search_network = self.network("reverse_search")?;
        let fanbox_flaresolverr = self.solver("fanbox")?;
        let reverse_search_flaresolverr = self.solver("reverse_search")?;
        Ok(RuntimeConfig {
            download_path,
            filename_template,
            directory_template,
            https_proxy,
            request_interval,
            log_level,
            log_format,
            pixiv_network,
            fanbox_network,
            reverse_search_network,
            fanbox_flaresolverr,
            reverse_search_flaresolverr,
            output_json,
            update_check_enabled,
            login_open_browser,
            login_use_after_login,
            reverse_search_provider,
            reverse_search_pixiv_only,
            saucenao_api_key,
            login_relay_public_url,
            login_relay_listen_addr,
            login_relay_tls_cert_file,
            login_relay_tls_key_file,
            account_pool,
        })
    }
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::String(_) => "string",
        Value::Integer(_) => "int64",
        Value::Float(_) => "float64",
        Value::Boolean(_) => "bool",
        Value::Datetime(_) => "time.Time",
        Value::Array(_) => "[]interface {}",
        Value::Table(_) => "map[string]interface {}",
    }
}

fn string_value(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Integer(value) => value.to_string(),
        Value::Float(value) => float_string(*value),
        Value::Boolean(value) => value.to_string(),
        Value::Array(value) => format!(
            "[{}]",
            value.iter().map(string_value).collect::<Vec<_>>().join(" ")
        ),
        Value::Table(value) => format!(
            "map[{}]",
            value
                .iter()
                .map(|(key, value)| format!("{key}:{}", string_value(value)))
                .collect::<Vec<_>>()
                .join(" ")
        ),
        Value::Datetime(value) => {
            if value.offset.is_none() {
                return value.to_string();
            }
            let date = value
                .date
                .map(|date| date.to_string())
                .unwrap_or_else(|| "0000-01-01".into());
            let time = value
                .time
                .map(|time| time.to_string())
                .unwrap_or_else(|| "00:00:00".into());
            let offset = match value.offset {
                Some(toml::value::Offset::Custom { minutes }) if minutes != 0 => {
                    let sign = if minutes < 0 { '-' } else { '+' };
                    let absolute = minutes.unsigned_abs();
                    format!(
                        "{sign}{:02}{:02} {sign}{:02}{:02}",
                        absolute / 60,
                        absolute % 60,
                        absolute / 60,
                        absolute % 60
                    )
                }
                _ => "+0000 UTC".into(),
            };
            format!("{date} {time} {offset}")
        }
    }
}

fn float_string(value: f64) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value.is_infinite() {
        return if value.is_sign_negative() {
            "-Inf"
        } else {
            "+Inf"
        }
        .into();
    }
    let absolute = value.abs();
    if absolute != 0.0 && !(1e-4..1e6).contains(&absolute) {
        let scientific = format!("{value:e}");
        let (mantissa, exponent) = scientific.split_once('e').unwrap();
        let exponent: i32 = exponent.parse().unwrap();
        let sign = if exponent < 0 { '-' } else { '+' };
        format!("{mantissa}e{sign}{:02}", exponent.unsigned_abs())
    } else {
        value.to_string()
    }
}
