use crate::CommandError;
use pixiv_app::{
    callback_handler::CallbackResult,
    lifecycle::Context,
    url_handler::{SystemUrlHandler, UrlHandler},
};
use std::io::Write;

pub trait StartupHooks {
    fn cleanup_pending_update(&self) -> CallbackResult<()>;
    fn automatic_supported(&self) -> bool;
    fn ensure_if_needed(&self, context: &Context) -> CallbackResult<()>;
}
pub struct SystemStartupHooks {
    handler: SystemUrlHandler,
}
impl SystemStartupHooks {
    pub fn system() -> Self {
        Self {
            handler: SystemUrlHandler::system(),
        }
    }
}
impl StartupHooks for SystemStartupHooks {
    fn cleanup_pending_update(&self) -> CallbackResult<()> {
        pixiv_app::pending_update::cleanup_pending_windows_update()
            .map_err(|error| Box::new(error) as _)
    }
    fn automatic_supported(&self) -> bool {
        self.handler.automatic_supported()
    }
    fn ensure_if_needed(&self, context: &Context) -> CallbackResult<()> {
        self.handler.ensure_if_needed(context)
    }
}
pub fn run_startup<W: Write>(
    context: &Context,
    hooks: &dyn StartupHooks,
    diagnostics: &mut W,
) -> Result<(), CommandError> {
    hooks
        .cleanup_pending_update()
        .map_err(|error| CommandError::Startup(format!("clean pending update: {error}")))?;
    if hooks.automatic_supported() && hooks.ensure_if_needed(context).is_err() {
        let _ = writeln!(
            diagnostics,
            "warning: persistent pixiv:// callback handler was not initialized"
        );
    }
    Ok(())
}
pub fn run_system_startup<W: Write>(
    context: &Context,
    diagnostics: &mut W,
) -> Result<(), CommandError> {
    run_startup(context, &SystemStartupHooks::system(), diagnostics)
}
