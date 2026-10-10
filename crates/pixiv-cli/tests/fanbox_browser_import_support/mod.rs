pub mod isolation;
pub mod owned;
pub mod schema;

use crate::fanbox_auth_support::{self, Shared, context_marker};
use pixiv_app::{
    browser_cookies::{BrowserCookieBackend, BrowserCookieError, BrowserProfile},
    browser_dpapi::{DpapiBlob, WindowsDpapi, WindowsDpapiApi},
    lifecycle::Context,
};
use pixiv_cli_rs::{
    CommandError,
    fanbox_auth::{BrowserBytesFuture, BrowserFuture, BrowserProvider},
    fanbox_browser::{CookieProvider, NativeCookieProvider, SystemBrowserProvider},
};
use schema::Case;
use serde_json::{Value, json};
use std::{
    ffi::c_void,
    path::Path,
    sync::{Arc, Mutex},
};

struct NeverDpapi;
unsafe impl WindowsDpapiApi for NeverDpapi {
    unsafe fn crypt_unprotect_data(
        &self,
        _: *const DpapiBlob,
        _: *mut *mut u16,
        _: *const DpapiBlob,
        _: *mut c_void,
        _: *const c_void,
        _: u32,
        _: *mut DpapiBlob,
    ) -> bool {
        panic!("the Linux connected fixture must not call native Windows APIs")
    }
    unsafe fn local_free(&self, _: *mut c_void) -> *mut c_void {
        panic!("the Linux connected fixture must not call native Windows APIs")
    }
}

fn backend(
    home: &Path,
    case: &Case,
    process: Arc<owned::Process>,
    browser: &str,
) -> Result<BrowserCookieBackend, BrowserCookieError> {
    BrowserCookieBackend::new(
        browser,
        Arc::new(owned::Environment {
            home: home.into(),
            xdg_empty: case.xdg_empty,
        }),
        Arc::new(owned::Files { home: home.into() }),
        process,
        Arc::new(WindowsDpapi::new(Arc::new(NeverDpapi))),
    )
}
fn profiles(profiles: Vec<BrowserProfile>, home: &Path) -> Vec<Value> {
    profiles
        .into_iter()
        .map(|profile| {
            assert!(profile.path.starts_with(home));
            json!({
                "ID": String::from_utf8(profile.id).unwrap(),
                "Name": String::from_utf8(profile.name).unwrap(),
                "Path": profile.path.to_str().unwrap().replace(home.to_str().unwrap(), "$HOME"),
            })
        })
        .collect()
}
pub fn discover(home: &Path, case: &Case, process: Arc<owned::Process>) -> Value {
    match backend(home, case, process, &case.browser) {
        Err(error) => json!({"profiles": [], "error": error.to_string()}),
        Ok(backend) => {
            let result = backend.discover_profiles(&Context::background());
            backend.close().unwrap();
            match result {
                Ok(discovered) => json!({"profiles": profiles(discovered, home), "error": ""}),
                Err(error) => json!({"profiles": [], "error": error.to_string()}),
            }
        }
    }
}

#[derive(Default)]
struct NativeResult {
    hex: String,
    error: String,
}
struct RecordingBrowser {
    inner: SystemBrowserProvider,
    observed: Shared,
    native: Arc<Mutex<NativeResult>>,
}
impl BrowserProvider for RecordingBrowser {
    fn read_session<'a>(&'a self, _: &'a Context, _: &'a str, _: &'a str) -> BrowserFuture<'a> {
        Box::pin(async {
            panic!("connected FANBOX import must use the opaque-byte browser boundary")
        })
    }
    fn read_session_bytes<'a>(
        &'a self,
        context: &'a Context,
        browser: &'a str,
        profile: &'a str,
    ) -> BrowserBytesFuture<'a> {
        Box::pin(async move {
            self.observed.lock().unwrap().trace.push(format!(
                "browser.read/browser={browser}/profile={profile}/context={}",
                context_marker(context)
            ));
            let result = self
                .inner
                .read_session_bytes(context, browser, profile)
                .await;
            let mut native = self.native.lock().unwrap();
            match &result {
                Ok(bytes) => native.hex = owned::hex(bytes),
                Err(error) => native.error = error.to_string(),
            }
            result
        })
    }
}

pub async fn observe(
    home: &Path,
    case: &Case,
    sqlite_bytes: &[u8],
    network_denied: bool,
) -> (Value, Vec<Value>) {
    owned::prepare(home, case, sqlite_bytes);
    let process = Arc::new(owned::Process::new(home));
    let discovered = discover(home, case, process.clone());
    assert!(
        process.commands.lock().unwrap().is_empty(),
        "discovery executed child commands"
    );
    let mut native_results = Vec::new();
    let mut command_offsets = Vec::new();
    let home = home.to_owned();
    let owned_case = case.clone();
    let factory_process = process.clone();
    let auth = case.auth_case();
    let observed = fanbox_auth_support::observe_with_browser(&home, &auth, |_, observation| {
        command_offsets.push(factory_process.commands.lock().unwrap().len());
        let native = Arc::new(Mutex::new(NativeResult::default()));
        native_results.push(native.clone());
        let factory_home = home.clone();
        let factory_case = owned_case.clone();
        let process = factory_process.clone();
        let inner = SystemBrowserProvider::with_factory(Arc::new(move |browser| {
            backend(&factory_home, &factory_case, process.clone(), browser)
                .map(|backend| {
                    Arc::new(NativeCookieProvider::new(Arc::new(backend)))
                        as Arc<dyn CookieProvider>
                })
                .map_err(|error| CommandError::MessageText(error.to_string()))
        }));
        Box::new(RecordingBrowser {
            inner,
            observed: observation,
            native,
        })
    })
    .await;
    assert_eq!(observed.len(), native_results.len());
    let commands = process.commands.lock().unwrap();
    command_offsets.push(commands.len());
    let mut observations = Vec::new();
    for (index, (observed, native)) in observed.into_iter().zip(native_results).enumerate() {
        let mut value = serde_json::to_value(observed).unwrap();
        let native = native.lock().unwrap();
        value["native_session_hex"] = json!(native.hex);
        value["native_session_error"] = json!(native.error);
        value["socket_denied"] = json!(network_denied);
        value["exec_denied"] = json!(false);
        value["commands"] = json!(&commands[command_offsets[index]..command_offsets[index + 1]]);
        observations.push(value);
    }
    (discovered, observations)
}
