use pixiv_app::lifecycle::Context;
use pixiv_cli_rs::{
    CommandError,
    fanbox_auth::{BrowserFuture, BrowserProvider},
    fanbox_browser::{
        BrowserCookieBytesFuture, BrowserCookiesFuture, BrowserProfilesFuture, CookieProfile,
        CookieProvider, SystemBrowserProvider,
    },
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct StringBrowser;
impl BrowserProvider for StringBrowser {
    fn read_session<'a>(&'a self, _: &'a Context, _: &'a str, _: &'a str) -> BrowserFuture<'a> {
        Box::pin(async { Ok("synthetic-ascii".into()) })
    }
}

struct ByteCookies {
    values: Vec<Vec<u8>>,
    closes: Arc<AtomicUsize>,
}
impl CookieProvider for ByteCookies {
    fn discover_profiles<'a>(&'a self, _: &'a Context) -> BrowserProfilesFuture<'a> {
        Box::pin(async {
            Ok(vec![CookieProfile {
                id: "Default".into(),
                path: "/owned-profile".into(),
            }])
        })
    }
    fn read<'a>(
        &'a self,
        _: &'a Context,
        _: &'a str,
        _: &'a str,
        _: &'a str,
    ) -> BrowserCookiesFuture<'a> {
        panic!("the byte adapter must not call the String cookie method")
    }
    fn read_bytes<'a>(
        &'a self,
        _: &'a Context,
        host: &'a str,
        name: &'a str,
        profile: &'a str,
    ) -> BrowserCookieBytesFuture<'a> {
        Box::pin(async move {
            assert_eq!(
                (host, name, profile),
                (".fanbox.cc", "FANBOXSESSID", "Default")
            );
            Ok(self.values.clone())
        })
    }
    fn close(&self) -> Result<(), CommandError> {
        self.closes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn existing_string_browser_implementations_keep_their_byte_default() {
    assert_eq!(
        StringBrowser
            .read_session_bytes(&Context::background(), "chrome", "Default")
            .await
            .unwrap(),
        b"synthetic-ascii"
    );
}

#[tokio::test]
async fn chromium_opaque_bytes_survive_the_single_cookie_adapter() {
    let closes = Arc::new(AtomicUsize::new(0));
    let provider: Arc<dyn CookieProvider> = Arc::new(ByteCookies {
        values: vec![vec![b'x', 0xff, 0, b'y']],
        closes: closes.clone(),
    });
    let browser = SystemBrowserProvider::with_factory(Arc::new(move |_| Ok(provider.clone())));
    assert_eq!(
        browser
            .read_session_bytes(&Context::background(), "chrome", "")
            .await
            .unwrap(),
        [b'x', 0xff, 0, b'y']
    );
    assert_eq!(closes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn byte_adapter_preserves_cardinality_errors_and_once_owned_close() {
    for (values, error) in [
        (
            vec![],
            "browser profile does not contain a FANBOXSESSID cookie",
        ),
        (
            vec![b"first".to_vec(), b"second".to_vec()],
            "browser profile contains multiple FANBOXSESSID cookies",
        ),
    ] {
        let closes = Arc::new(AtomicUsize::new(0));
        let provider: Arc<dyn CookieProvider> = Arc::new(ByteCookies {
            values,
            closes: closes.clone(),
        });
        let browser = SystemBrowserProvider::with_factory(Arc::new(move |_| Ok(provider.clone())));
        assert_eq!(
            browser
                .read_session_bytes(&Context::background(), "chrome", "")
                .await
                .unwrap_err()
                .to_string(),
            error
        );
        assert_eq!(closes.load(Ordering::SeqCst), 1);
    }
}
