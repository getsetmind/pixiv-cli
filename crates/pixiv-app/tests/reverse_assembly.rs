use pixiv_app::{
    config::{FlareSolverrConfig, RuntimeConfig, Snapshot as ConfigSnapshot},
    reverse_search::{
        Error, ErrorCode, Provider, Request, Searcher, SourceKind,
        assembly::{FlareSolverrOptions, Options, build},
    },
};
use pixiv_sdk::context::{Context, ContextError};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

fn runtime() -> RuntimeConfig {
    ConfigSnapshot::parse("", []).unwrap().runtime().unwrap()
}

#[test]
fn runtime_proxy_resolution_preserves_the_frozen_cli_and_mcp_precedence() {
    let cases = [
        (None, None, "http://global.invalid", "http://global.invalid"),
        (
            Some("http://ascii.invalid"),
            None,
            "http://global.invalid",
            "http://ascii.invalid",
        ),
        (Some(""), None, "http://global.invalid", ""),
        (
            Some("http://ascii.invalid"),
            Some("http://flag.invalid"),
            "http://flag.invalid",
            "http://flag.invalid",
        ),
        (Some("http://ascii.invalid"), Some(""), "", ""),
        (None, Some(""), "", ""),
    ];
    for (service, override_proxy, standard, ascii) in cases {
        let mut runtime = runtime();
        runtime.https_proxy = "http://global.invalid".into();
        runtime.reverse_search_network.proxy_url = service.map(str::to_owned);
        let options = Options::from_runtime(&runtime, override_proxy);
        assert_eq!(options.proxy, standard);
        assert_eq!(options.ascii2d_proxy.as_deref(), Some(ascii));
    }
}

#[test]
fn runtime_options_own_one_snapshot_of_reverse_search_configuration() {
    let mut runtime = runtime();
    runtime.https_proxy = "http://standard.invalid".into();
    runtime.reverse_search_network.proxy_url = Some("http://ascii.invalid".into());
    runtime.reverse_search_network.user_agent = Some("fixture-browser-agent".into());
    runtime.saucenao_api_key = "fixture-key".into();
    runtime.reverse_search_flaresolverr = Some(FlareSolverrConfig {
        url: "http://solver.invalid".into(),
        proxy_url: "socks5://solver-browser.invalid:1080".into(),
    });
    let options = Options::from_runtime(&runtime, None);
    runtime.https_proxy.clear();
    runtime.reverse_search_network.proxy_url = None;
    runtime.reverse_search_network.user_agent = None;
    runtime.saucenao_api_key.clear();
    runtime.reverse_search_flaresolverr = None;
    assert_eq!(options.proxy, "http://standard.invalid");
    assert_eq!(
        options.ascii2d_proxy.as_deref(),
        Some("http://ascii.invalid")
    );
    assert_eq!(options.user_agent, "fixture-browser-agent");
    assert_eq!(options.saucenao_key, "fixture-key");
    let solver = options.flaresolverr.unwrap();
    assert_eq!(solver.url, "http://solver.invalid");
    assert_eq!(solver.proxy_url, "socks5://solver-browser.invalid:1080");
    let absent = Options::from_runtime(&runtime, None);
    assert!(absent.flaresolverr.is_none());
    assert_eq!(absent.user_agent, "");
}

fn construction_error(options: Options) -> Error {
    match build(options) {
        Ok(_) => panic!("invalid assembly configuration was accepted"),
        Err(error) => error,
    }
}

#[tokio::test]
async fn production_constructor_validates_the_standard_proxy_before_browser_options() {
    let error = construction_error(Options {
        proxy: "not a proxy".into(),
        user_agent: "invalid\nagent".into(),
        ..Options::default()
    });
    assert_eq!(error.code(), ErrorCode::Unknown);
    let error = construction_error(Options {
        ascii2d_proxy: Some("not a proxy".into()),
        ..Options::default()
    });
    assert_eq!(error.code(), ErrorCode::InvalidRequest);
    assert_eq!(error.to_string(), "ascii2d proxy URL is invalid");
    let error = construction_error(Options {
        user_agent: "invalid\nagent".into(),
        ..Options::default()
    });
    assert_eq!(error.code(), ErrorCode::InvalidRequest);
    assert_eq!(
        error.to_string(),
        "ascii2d user-agent contains invalid header characters"
    );
}

#[tokio::test]
async fn explicit_direct_browser_proxy_does_not_fall_back_to_the_standard_proxy() {
    let standard_proxy = "http://proxy.invalid/ordinary-client-path";
    let error = construction_error(Options {
        proxy: standard_proxy.into(),
        ..Options::default()
    });
    assert_eq!(error.code(), ErrorCode::InvalidRequest);
    assert_eq!(error.to_string(), "ascii2d proxy URL is invalid");
    let facade = build(Options {
        proxy: standard_proxy.into(),
        ascii2d_proxy: Some(String::new()),
        ..Options::default()
    })
    .unwrap();
    facade.close().await.unwrap();
}

#[tokio::test]
async fn production_constructor_passes_solver_configuration_to_the_browser_provider() {
    let error = construction_error(Options {
        flaresolverr: Some(FlareSolverrOptions {
            url: "ftp://solver.invalid".into(),
            proxy_url: "socks5://browser.invalid:1080".into(),
        }),
        ..Options::default()
    });
    assert_eq!(error.code(), ErrorCode::SolverUnavailable);
    let facade = build(Options {
        flaresolverr: Some(FlareSolverrOptions {
            url: "http://127.0.0.1:1".into(),
            proxy_url: "socks5://solver-browser.invalid:1080".into(),
        }),
        ..Options::default()
    })
    .unwrap();
    let (first, second) = tokio::join!(facade.close(), facade.close());
    first.unwrap();
    second.unwrap();
}

#[tokio::test]
async fn assembled_facade_preflights_credentials_and_cancellation_before_source_io() {
    let facade = build(Options::default()).unwrap();
    let result = facade
        .search(
            Arc::new(Context::background()),
            Request {
                source: "missing-source.png".into(),
                provider: Provider::SauceNao,
                pixiv_only: false,
            },
        )
        .await;
    let error = result.error.unwrap();
    assert_eq!(error.code(), ErrorCode::MissingCredential);
    assert_eq!(error.to_string(), "SauceNAO API key is required");
    assert_eq!(result.response.input.kind, SourceKind::Unspecified);
    let context = Context::new();
    context.cancel();
    assert_eq!(context.error(), Some(ContextError::Canceled));
    let result = facade
        .search(
            Arc::new(context),
            Request {
                source: "missing-source.png".into(),
                provider: Provider::All,
                pixiv_only: false,
            },
        )
        .await;
    assert_eq!(
        result.error.unwrap().context_error(),
        Some(ContextError::Canceled)
    );
    let (first, second) = tokio::join!(facade.close(), facade.close());
    first.unwrap();
    second.unwrap();
    facade.close().await.unwrap();
}

#[tokio::test]
async fn assembled_browser_search_keeps_source_identity_on_local_image_rejection() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("invalid-image.bin");
    std::fs::write(&source, b"fixture-image").unwrap();
    let facade = build(Options::default()).unwrap();
    let result = facade
        .search(
            Arc::new(Context::background()),
            Request {
                source: source.to_string_lossy().into_owned(),
                provider: Provider::Ascii2dColor,
                pixiv_only: false,
            },
        )
        .await;
    assert_eq!(result.response.input.kind, SourceKind::File);
    assert_eq!(result.response.input.sha256.len(), 64);
    assert_eq!(result.error.unwrap().code(), ErrorCode::InvalidSource);
    facade.close().await.unwrap();
}

fn accept(listener: &TcpListener) -> TcpStream {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                return stream;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "missing ordinary proxy request");
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("proxy accept failed: {error}"),
        }
    }
}

fn headers(stream: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut byte = [0];
    while !bytes.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).unwrap();
        bytes.push(byte[0]);
        assert!(bytes.len() < 65536);
    }
    String::from_utf8(bytes).unwrap()
}

#[tokio::test]
async fn source_and_saucenao_use_the_ordinary_proxy_without_browser_headers() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy = format!("http://{}", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let server = thread::spawn(move || {
        let mut source = accept(&listener);
        let source_headers = headers(&mut source);
        source
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\nConnection: close\r\n\r\nfixture-image",
            )
            .unwrap();
        drop(source);
        let mut sauce = accept(&listener);
        let sauce_headers = headers(&mut sauce);
        sauce
            .write_all(
                b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        (source_headers, sauce_headers)
    });
    let facade = build(Options {
        proxy,
        ascii2d_proxy: Some(String::new()),
        user_agent: "fixture-browser-agent".into(),
        saucenao_key: "fixture-key".into(),
        ..Options::default()
    })
    .unwrap();
    let result = facade
        .search(
            Arc::new(Context::background()),
            Request {
                source: "http://source.invalid/image.png".into(),
                provider: Provider::SauceNao,
                pixiv_only: false,
            },
        )
        .await;
    facade.close().await.unwrap();
    let (source_headers, sauce_headers) = server.join().unwrap();
    assert!(source_headers.starts_with("GET http://source.invalid/image.png HTTP/1.1\r\n"));
    assert!(
        source_headers
            .to_ascii_lowercase()
            .contains("user-agent: go-http-client/1.1\r\n")
    );
    assert!(!source_headers.contains("fixture-browser-agent"));
    assert!(!source_headers.to_ascii_lowercase().contains("sec-fetch-"));
    assert!(sauce_headers.starts_with("CONNECT saucenao.com:443 HTTP/1.1\r\n"));
    assert!(!sauce_headers.contains("fixture-key"));
    assert_eq!(result.response.input.kind, SourceKind::Url);
    assert_eq!(result.error.unwrap().code(), ErrorCode::ProviderFailed);
}
