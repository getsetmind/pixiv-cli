use super::{Shared, schema::Step, trace};
use pixiv_app::{callback_handler::CallbackResult, lifecycle::Context};
use pixiv_cli_rs::{
    CommandError,
    auth_accounts::AccountPrompts,
    fanbox_auth::{AutomaticUpdateHooks, UpdateRuntime},
    startup::StartupHooks,
};
use std::io;

pub struct Hooks {
    pub step: Step,
    pub observed: Shared,
}
impl StartupHooks for Hooks {
    fn cleanup_pending_update(&self) -> CallbackResult<()> {
        trace(&self.observed, "startup.cleanup");
        if self.step.startup_error {
            return Err(Box::new(io::Error::other("owned fixture startup failure")));
        }
        Ok(())
    }
    fn automatic_supported(&self) -> bool {
        trace(&self.observed, "startup.supported");
        self.step.startup_relay
    }
    fn ensure_if_needed(&self, _: &Context) -> CallbackResult<()> {
        trace(&self.observed, "startup.relay");
        Err(Box::new(io::Error::other("owned fixture relay failure")))
    }
}
impl AutomaticUpdateHooks for Hooks {
    fn runtime(&self) -> Result<UpdateRuntime, CommandError> {
        trace(&self.observed, "update.runtime");
        if self.step.update_mode == "runtime-error" {
            return Err(CommandError::Message(
                "owned fixture update runtime failure",
            ));
        }
        Ok(UpdateRuntime {
            enabled: self.step.update_mode != "disabled",
            https_proxy: "http://fixture-global-proxy:8080".into(),
        })
    }
    fn check(&self, _: &Context, proxy: &str) -> Result<(), CommandError> {
        trace(&self.observed, format!("update.factory/proxy={proxy}"));
        Err(CommandError::Message(
            "owned fixture update factory failure",
        ))
    }
}

pub struct Prompts {
    pub step: Step,
    pub observed: Shared,
}
impl AccountPrompts for Prompts {
    fn can_prompt(&self) -> bool {
        trace(&self.observed, "can-prompt");
        self.step.can_prompt
    }
    fn secret(&mut self, message: &str) -> Result<String, CommandError> {
        trace(&self.observed, format!("prompt.secret/{message}"));
        if self.step.prompt_error {
            return Err(CommandError::Message("owned fixture secret prompt failure"));
        }
        Ok(self.step.prompt_value.clone())
    }
    fn confirm(&mut self, message: &str, default: bool) -> Result<bool, CommandError> {
        trace(
            &self.observed,
            format!("prompt.confirm/{message}/default={default}"),
        );
        if self.step.prompt_error {
            return Err(CommandError::Message("owned fixture confirmation failure"));
        }
        Ok(self.step.confirm)
    }
    fn select(&mut self, _: &str, _: &[String]) -> Result<String, CommandError> {
        panic!("FANBOX auth does not present account selection")
    }
}
