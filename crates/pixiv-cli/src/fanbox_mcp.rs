use crate::CommandError;
use pixiv_app::fanbox_facade::Facade;
use std::{future::Future, sync::Arc};

pub struct Data<S, R> {
    pub service_factory: S,
    pub run_server: R,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct McpCommand {
    pub proxy: Option<String>,
    pub no_proxy: Option<bool>,
}
impl McpCommand {
    pub fn parse(args: &[String]) -> Result<Self, CommandError> {
        let args = args.strip_prefix(&["fanbox".into()]).unwrap_or(args);
        let args = args.strip_prefix(&["mcp".into()]).unwrap_or(args);
        let mut command = Self::default();
        let mut index = 0;
        let mut positional = false;
        while let Some(argument) = args.get(index) {
            index += 1;
            if argument == "--" && !positional {
                positional = true;
                continue;
            }
            if positional || !argument.starts_with('-') || argument == "-" {
                return Err(CommandError::Message("usage: pixiv fanbox mcp"));
            }
            let (flag, attached) = argument
                .split_once('=')
                .map_or((argument.as_str(), None), |(flag, value)| {
                    (flag, Some(value))
                });
            match flag {
                "--proxy" => {
                    let value = if let Some(value) = attached {
                        value
                    } else {
                        let value = args.get(index).ok_or_else(|| {
                            CommandError::Usage("flag needs an argument: --proxy".into())
                        })?;
                        index += 1;
                        value
                    };
                    command.proxy = Some(value.to_owned());
                }
                "--no-proxy" => {
                    let value = attached.unwrap_or("true");
                    command.no_proxy = Some(match value {
                        "1" | "t" | "T" | "TRUE" | "true" | "True" => true,
                        "0" | "f" | "F" | "FALSE" | "false" | "False" => false,
                        _ => {
                            return Err(CommandError::Usage(format!(
                                "invalid argument {value:?} for {flag:?} flag: strconv.ParseBool: parsing {value:?}: invalid syntax"
                            )));
                        }
                    });
                }
                _ => return Err(CommandError::Usage(format!("unknown option '{flag}'"))),
            }
        }
        Ok(command)
    }
    pub fn proxy_override(&self) -> Result<Option<String>, CommandError> {
        if self.proxy.is_some() && self.no_proxy.is_some() {
            return Err(CommandError::Message(
                "use either --proxy or --no-proxy, not both",
            ));
        }
        Ok(if self.no_proxy == Some(true) {
            Some(String::new())
        } else {
            self.proxy.clone()
        })
    }
    pub async fn run<S, R, F>(self, data: Data<S, R>) -> Result<(), CommandError>
    where
        S: FnOnce() -> Result<Option<Arc<Facade>>, CommandError>,
        R: FnOnce(Arc<Facade>, Option<String>) -> F,
        F: Future<Output = Result<(), CommandError>>,
    {
        let proxy = self.proxy_override()?;
        let facade = (data.service_factory)()?.ok_or(CommandError::Message(
            "fanbox is not available: cannot open the local account store",
        ))?;
        (data.run_server)(facade, proxy).await
    }
}
