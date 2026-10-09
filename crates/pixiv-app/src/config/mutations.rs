use super::{
    ConfigError, Kind, SPECS, Scalar, SettingValue, Snapshot, Spec, Store, document, private_file,
};
use std::collections::BTreeMap;

pub struct ConfigMutationResult {
    pub alias: String,
    pub env_override: String,
    pub has_override: bool,
}

pub fn valid_setting_aliases() -> Vec<&'static str> {
    let mut aliases: Vec<_> = SPECS
        .iter()
        .filter(|spec| !matches!(spec.kind, Kind::Removed))
        .map(|spec| spec.alias)
        .collect();
    aliases.sort_unstable();
    aliases
}

pub fn cli_setting_aliases() -> Vec<&'static str> {
    let mut aliases: Vec<_> = SPECS
        .iter()
        .filter(|spec| spec.cli_managed && !matches!(spec.kind, Kind::Removed))
        .map(|spec| spec.alias)
        .collect();
    aliases.sort_unstable();
    aliases
}

pub fn public_setting_text(alias: &str, text: &str) -> String {
    if !text.is_empty()
        && SPECS
            .iter()
            .any(|spec| spec.alias == alias && spec.sensitive)
    {
        "<redacted>".into()
    } else {
        text.into()
    }
}

fn environment() -> impl Iterator<Item = (String, String)> {
    SPECS
        .iter()
        .flat_map(|spec| spec.environment)
        .filter_map(|key| {
            std::env::var_os(key).map(|value| ((*key).into(), value.to_string_lossy().into_owned()))
        })
}

fn spec(alias: &str) -> Result<&'static Spec, ConfigError> {
    SPECS
        .iter()
        .find(|spec| spec.alias == alias)
        .ok_or_else(|| {
            ConfigError::Invalid(format!(
                "unknown config key {alias:?}. valid keys: {}",
                valid_setting_aliases().join(", ")
            ))
        })
}

fn mutation_result(spec: &Spec, environment: BTreeMap<String, String>) -> ConfigMutationResult {
    let value = spec
        .environment
        .iter()
        .find_map(|key| environment.get(*key));
    ConfigMutationResult {
        alias: spec.alias.into(),
        env_override: if spec.sensitive {
            String::new()
        } else {
            value.cloned().unwrap_or_default()
        },
        has_override: value.is_some(),
    }
}

fn parse_input(spec: &Spec, raw: &str) -> Result<toml_edit::Value, ConfigError> {
    if matches!(spec.kind, Kind::Removed) {
        return Err(ConfigError::Removed(spec.alias));
    }
    let raw = raw.trim();
    let mut table = toml::Table::new();
    let (section, key) = spec
        .path
        .split_once('.')
        .expect("setting path contains a table");
    let mut values = toml::Table::new();
    values.insert(key.into(), toml::Value::String(raw.into()));
    table.insert(section.into(), toml::Value::Table(values));
    let snapshot = Snapshot {
        file: table,
        environment: BTreeMap::new(),
    };
    let value = snapshot
        .effective(spec.alias)
        .map_err(|error| match spec.kind {
            Kind::Bool => ConfigError::Invalid(error.to_string().replacen(
                &format!("{}: ", spec.alias),
                "expects bool value: ",
                1,
            )),
            Kind::Duration if error.to_string().starts_with(&format!("{}: ", spec.alias)) => {
                ConfigError::Invalid(error.to_string().replacen(
                    &format!("{}: ", spec.alias),
                    "expects duration value: ",
                    1,
                ))
            }
            _ => error,
        })?;
    Ok(match value.value {
        Some(Scalar::Bool(value)) => toml_edit::Value::from(value),
        Some(Scalar::Duration(_)) => toml_edit::Value::from(value.text),
        Some(Scalar::String(value)) => toml_edit::Value::from(value),
        None => toml_edit::Value::from(raw),
    })
}

impl Store {
    pub fn get(&self, alias: &str) -> Result<SettingValue, ConfigError> {
        self.get_with_environment(alias, environment())
    }

    pub fn get_with_environment(
        &self,
        alias: &str,
        environment: impl IntoIterator<Item = (String, String)>,
    ) -> Result<SettingValue, ConfigError> {
        self.current_with_environment(environment)?
            .effective(alias)
            .map_err(|error| {
                ConfigError::Context(
                    Box::new(error),
                    format!(". valid keys: {}", valid_setting_aliases().join(", ")),
                )
            })
    }

    pub fn set(&self, alias: &str, raw: &str) -> Result<ConfigMutationResult, ConfigError> {
        self.set_with_environment(alias, raw, environment())
    }

    pub fn set_with_environment(
        &self,
        alias: &str,
        raw: &str,
        environment: impl IntoIterator<Item = (String, String)>,
    ) -> Result<ConfigMutationResult, ConfigError> {
        let spec = spec(alias)?;
        let value = parse_input(spec, raw)?;
        self.mutate(spec, Some(value))?;
        Ok(mutation_result(spec, environment.into_iter().collect()))
    }

    pub fn unset(&self, alias: &str) -> Result<ConfigMutationResult, ConfigError> {
        self.unset_with_environment(alias, environment())
    }

    pub fn unset_with_environment(
        &self,
        alias: &str,
        environment: impl IntoIterator<Item = (String, String)>,
    ) -> Result<ConfigMutationResult, ConfigError> {
        let spec = spec(alias)?;
        self.mutate(spec, None)?;
        Ok(mutation_result(spec, environment.into_iter().collect()))
    }

    fn mutate(&self, spec: &Spec, value: Option<toml_edit::Value>) -> Result<(), ConfigError> {
        let body = match std::fs::read_to_string(&self.path) {
            Ok(body) => body,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(ConfigError::Io(error)),
        };
        let body = document::mutate(&body, spec.path, value)?;
        private_file::write(&self.path, body.as_bytes())
    }
}
