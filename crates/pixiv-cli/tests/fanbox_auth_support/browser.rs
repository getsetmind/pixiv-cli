use super::{Shared, context_marker, schema::Step, trace};
use pixiv_app::lifecycle::Context;
use pixiv_cli_rs::{
    CommandError,
    fanbox_auth::{
        BrowserCookiesFuture, BrowserFuture, BrowserProfilesFuture, BrowserProvider, CookieProfile,
        CookieProvider, SystemBrowserProvider,
    },
};
use std::sync::Arc;

pub struct Browser {
    pub step: Step,
    pub observed: Shared,
}
impl BrowserProvider for Browser {
    fn read_session<'a>(
        &'a self,
        context: &'a Context,
        browser: &'a str,
        profile: &'a str,
    ) -> BrowserFuture<'a> {
        Box::pin(async move {
            if !self.step.system_browser_mode.is_empty() {
                let observed = self.observed.clone();
                let mode = self.step.system_browser_mode.clone();
                let system = SystemBrowserProvider::with_factory(Arc::new(move |name| {
                    if name != "fixture-fanbox" {
                        return Err(CommandError::Message("browsercookies: unknown browser"));
                    }
                    trace(&observed, "provider.factory");
                    Ok(Arc::new(Provider {
                        mode: mode.clone(),
                        observed: observed.clone(),
                    }) as Arc<dyn CookieProvider>)
                }));
                return system.read_session(context, browser, profile).await;
            }
            trace(
                &self.observed,
                format!(
                    "browser.read/browser={browser}/profile={profile}/context={}",
                    context_marker(context)
                ),
            );
            if self.step.browser_error {
                return Err(CommandError::Message("owned fixture browser failure"));
            }
            Ok(self.step.browser_value.clone())
        })
    }
}

struct Provider {
    mode: String,
    observed: Shared,
}
impl CookieProvider for Provider {
    fn discover_profiles<'a>(&'a self, context: &'a Context) -> BrowserProfilesFuture<'a> {
        Box::pin(async move {
            trace(
                &self.observed,
                format!("provider.discover/context={}", context_marker(context)),
            );
            if matches!(self.mode.as_str(), "discover" | "discover-close") {
                return Err(CommandError::Message("owned fixture discovery failure"));
            }
            if self.mode == "none" {
                return Ok(vec![]);
            }
            let mut profiles = vec![CookieProfile {
                id: "default".into(),
                path: "/owned-private-path-canary".into(),
            }];
            if self.mode == "multiple" {
                profiles.push(CookieProfile {
                    id: "secondary".into(),
                    path: "/owned-private-path-canary-2".into(),
                });
            }
            Ok(profiles)
        })
    }
    fn read<'a>(
        &'a self,
        context: &'a Context,
        host: &'a str,
        name: &'a str,
        profile: &'a str,
    ) -> BrowserCookiesFuture<'a> {
        Box::pin(async move {
            trace(
                &self.observed,
                format!(
                    "provider.read/host={host}/name={name}/profile={profile}/context={}",
                    context_marker(context)
                ),
            );
            if self.mode == "read" {
                return Err(CommandError::Message("owned fixture cookie read failure"));
            }
            if self.mode == "zero" {
                return Ok(vec![]);
            }
            let mut cookies = vec!["synthetic-browser-secret".into()];
            if self.mode == "cookies" {
                cookies.push("synthetic-browser-secret-2".into());
            }
            Ok(cookies)
        })
    }
    fn close(&self) -> Result<(), CommandError> {
        trace(&self.observed, "provider.close");
        if matches!(self.mode.as_str(), "close" | "discover-close") {
            return Err(CommandError::Message(
                "owned fixture provider close failure",
            ));
        }
        Ok(())
    }
}
