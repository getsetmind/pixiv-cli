use pixiv_app::lifecycle::Context;
use pixiv_cli_rs::{
    CommandError,
    fanbox_auth::{
        BrowserCookiesFuture, BrowserProfilesFuture, CookieProfile, CookieProvider,
        SystemBrowserProvider,
    },
};
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub struct Observed {
    pub trace: Mutex<Vec<String>>,
    pub contexts: Mutex<Vec<usize>>,
}
impl Observed {
    fn trace(&self, entry: impl Into<String>) {
        self.trace.lock().unwrap().push(entry.into());
    }

    fn context(&self, context: &Context) {
        self.contexts
            .lock()
            .unwrap()
            .push(context as *const Context as usize);
    }
}

pub fn adapter(mode: &str, observed: Arc<Observed>) -> SystemBrowserProvider {
    let mode = mode.to_owned();
    SystemBrowserProvider::with_factory(Arc::new(move |name| {
        if name != "fixture-fanbox" {
            return Err(CommandError::Message("browsercookies: unknown browser"));
        }
        observed.trace("provider.factory");
        Ok(Arc::new(Provider {
            mode: mode.clone(),
            observed: observed.clone(),
        }) as Arc<dyn CookieProvider>)
    }))
}

struct Provider {
    mode: String,
    observed: Arc<Observed>,
}
impl CookieProvider for Provider {
    fn discover_profiles<'a>(&'a self, context: &'a Context) -> BrowserProfilesFuture<'a> {
        Box::pin(async move {
            self.observed.trace("provider.discover/context=command");
            self.observed.context(context);
            if self.mode == "pending-discover" {
                return std::future::pending().await;
            }
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
            self.observed.trace(format!(
                "provider.read/host={host}/name={name}/profile={profile}/context=command",
            ));
            self.observed.context(context);
            if self.mode == "pending-read" {
                return std::future::pending().await;
            }
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
        self.observed.trace("provider.close");
        if matches!(self.mode.as_str(), "close" | "discover-close") {
            return Err(CommandError::Message(
                "owned fixture provider close failure",
            ));
        }
        Ok(())
    }
}
