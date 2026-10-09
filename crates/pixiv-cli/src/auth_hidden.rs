use crate::{CommandError, auth_accounts::boolean};
use pixiv_app::{
    callback_handler::{
        BrowserOpener, CallbackEndpointStore, ClientHandoff, DefaultCallbackEndpointStore,
        PreviousHandler, RemoteLoginHandoff, run_callback,
    },
    handoff_client::NativeHandoffTransport,
    handoff_state::DefaultHandoffState,
    lifecycle::Context,
    native_browser::NativeBrowserOpener,
    url_handler::{SystemUrlHandler, UrlHandler},
};
use std::io::Write;
use tokio_util::sync::CancellationToken;

pub enum HiddenAuthCommand {
    Callback(String),
    Install,
    Help(String),
}
pub struct HiddenAuthDependencies<'a, E, H, P, B> {
    pub endpoint: &'a E,
    pub handoff: &'a H,
    pub previous: &'a P,
    pub browser: &'a B,
    pub handler: &'a dyn UrlHandler,
}
impl HiddenAuthCommand {
    pub fn parse(args: &[String]) -> Result<Option<Self>, CommandError> {
        let operation = crate::auth_accounts::discover_auth_operation(args);
        let Some(operation @ ("_callback" | "_install-handler")) = operation else {
            return Ok(None);
        };
        let mut values = Vec::new();
        let mut help = false;
        let mut flags = true;
        for arg in args {
            if flags && arg == "--" {
                flags = false;
                continue;
            }
            if flags && arg.starts_with("--") {
                let (name, raw) = arg
                    .split_once('=')
                    .map_or((arg.as_str(), "true"), |(name, raw)| (name, raw));
                if name != "--help" {
                    return Err(CommandError::Usage(format!("unknown option '{name}'")));
                }
                help = boolean(raw, "-h, --help")?;
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
        if help {
            let usage = if operation == "_callback" {
                "_callback URL"
            } else {
                "_install-handler"
            };
            return Ok(Some(Self::Help(format!(
                "Usage:\n  pixiv auth {usage} [flags]\n\nFlags:\n  -h, --help   help for {operation}\n"
            ))));
        }
        let expected = if operation == "_callback" { 2 } else { 1 };
        if values.len() != expected {
            let usage = if operation == "_callback" {
                "_callback URL"
            } else {
                "_install-handler"
            };
            return Err(CommandError::MessageText(format!(
                "usage: pixiv auth {usage}"
            )));
        }
        Ok(Some(if operation == "_callback" {
            Self::Callback(values[1].clone())
        } else {
            Self::Install
        }))
    }
    pub async fn execute_system<W: Write, E: Write>(
        &self,
        context: &Context,
        out: &mut W,
        diagnostics: &mut E,
    ) -> Result<(), CommandError> {
        let handler = SystemUrlHandler::system();
        let handoff = ClientHandoff::new(NativeHandoffTransport::new(), DefaultHandoffState);
        self.execute_with_dependencies(
            context,
            out,
            diagnostics,
            HiddenAuthDependencies {
                endpoint: &DefaultCallbackEndpointStore,
                handoff: &handoff,
                previous: &handler,
                browser: &NativeBrowserOpener::default(),
                handler: &handler,
            },
        )
        .await
    }
    pub async fn execute_with_dependencies<
        W: Write,
        D: Write,
        E: CallbackEndpointStore,
        H: RemoteLoginHandoff,
        P: PreviousHandler,
        B: BrowserOpener,
    >(
        &self,
        context: &Context,
        out: &mut W,
        diagnostics: &mut D,
        dependencies: HiddenAuthDependencies<'_, E, H, P, B>,
    ) -> Result<(), CommandError> {
        match self {
            Self::Help(text) => {
                let _ = out.write_all(text.as_bytes());
                Ok(())
            }
            Self::Install => {
                if let Err(error) = dependencies.handler.ensure_if_needed(context) {
                    let _ = writeln!(
                        diagnostics,
                        "warning: persistent pixiv:// callback handler was not initialized: {error}"
                    );
                }
                Ok(())
            }
            Self::Callback(raw) => {
                let cancellation = CancellationToken::new();
                if context.error().is_some() {
                    cancellation.cancel();
                }
                let _ownership = CancelOnDrop(cancellation.clone());
                let mut callback = Box::pin(run_callback(
                    raw,
                    &cancellation,
                    dependencies.endpoint,
                    dependencies.handoff,
                    dependencies.previous,
                    dependencies.browser,
                ));
                tokio::select! {
                    result = &mut callback => result,
                    _ = context.cancelled() => { cancellation.cancel(); callback.await },
                }
                .map_err(CommandError::State)
            }
        }
    }
}
struct CancelOnDrop(CancellationToken);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
