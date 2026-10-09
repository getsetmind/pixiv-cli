mod http;

use crate::{
    config::RuntimeConfig,
    handoff_protocol,
    lifecycle::Context,
    login_bridge::{BridgeError, CallbackAccepter, LoginBridgeHooks},
    login_input::{equal_fold_ascii, split_host_port},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use pixiv_sdk::oauth::LoginUrl;
use std::{error::Error, fmt, net::IpAddr, sync::Arc};
use tokio::net::TcpListener;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RelayServerOptions {
    pub public_url: String,
    pub listen_addr: String,
    pub tls_cert_file: String,
    pub tls_key_file: String,
}

#[derive(Clone, Debug, Default)]
pub struct RelayServerFlagOverrides {
    pub public_url: Option<String>,
    pub listen_addr: Option<String>,
    pub tls_cert_file: Option<String>,
    pub tls_key_file: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RelayServerError(&'static str);
impl fmt::Display for RelayServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}
impl Error for RelayServerError {}

pub fn configured_relay_server_options(
    flags: &RelayServerFlagOverrides,
    config: &RuntimeConfig,
) -> Result<Option<RelayServerOptions>, RelayServerError> {
    let options = RelayServerOptions {
        public_url: flags
            .public_url
            .as_ref()
            .unwrap_or(&config.login_relay_public_url)
            .clone(),
        listen_addr: flags
            .listen_addr
            .as_ref()
            .unwrap_or(&config.login_relay_listen_addr)
            .clone(),
        tls_cert_file: flags
            .tls_cert_file
            .as_ref()
            .unwrap_or(&config.login_relay_tls_cert_file)
            .clone(),
        tls_key_file: flags
            .tls_key_file
            .as_ref()
            .unwrap_or(&config.login_relay_tls_key_file)
            .clone(),
    };
    if [
        &options.public_url,
        &options.listen_addr,
        &options.tls_cert_file,
        &options.tls_key_file,
    ]
    .iter()
    .all(|value| value.is_empty())
    {
        return Ok(None);
    }
    if options.public_url.is_empty() || options.listen_addr.is_empty() {
        return Err(RelayServerError(
            "remote login relay requires login_relay_public_url and login_relay_listen_addr",
        ));
    }
    if options.tls_cert_file.is_empty() != options.tls_key_file.is_empty() {
        return Err(RelayServerError(
            "remote login relay TLS requires both certificate and key files",
        ));
    }
    validate_relay_server_options(&options)?;
    Ok(Some(options))
}

pub fn canonical_public_url(raw: &str) -> Result<String, RelayServerError> {
    handoff_protocol::canonical_relay_origin(raw)
        .map_err(|_| RelayServerError("invalid remote login relay public URL"))
}

pub fn validate_relay_server_options(options: &RelayServerOptions) -> Result<(), RelayServerError> {
    canonical_public_url(&options.public_url)?;
    let (host, _) = split_host_port(&options.listen_addr).map_err(|_| {
        RelayServerError("remote login relay listen address must include host and port")
    })?;
    if host.trim().is_empty() {
        return Err(RelayServerError(
            "remote login relay listen address must include host and port",
        ));
    }
    let loopback = equal_fold_ascii(host, "localhost")
        || host.parse::<IpAddr>().is_ok_and(|ip| match ip {
            IpAddr::V4(ip) => ip.is_loopback(),
            IpAddr::V6(ip) => {
                ip.is_loopback() || ip.to_ipv4_mapped().is_some_and(|ip| ip.is_loopback())
            }
        });
    let https = LoginUrl::parse(options.public_url.trim())
        .is_some_and(|url| url.scheme().eq_ignore_ascii_case("https"));
    if https && options.tls_cert_file.is_empty() && !loopback {
        return Err(RelayServerError(
            "HTTPS relay without TLS PEM must listen on loopback for a same-host reverse proxy",
        ));
    }
    Ok(())
}

pub fn handoff_relay_deep_link(origin: &str, session_id: &str, proof: &str) -> String {
    let query = pixiv_sdk::transport::encode_query_pairs(&[
        ("access".to_owned(), proof.to_owned()),
        ("origin".to_owned(), origin.to_owned()),
        ("session".to_owned(), session_id.to_owned()),
    ]);
    format!("pixiv://account/remote-login?{query}")
}

fn new_id() -> Result<String, BridgeError> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| std::io::Error::other(error.to_string()))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

struct SessionCapabilities {
    public_url: String,
    session_id: String,
    proof: String,
    result_id: String,
}

pub use http::RelayLoginServer;
pub struct RelayLoginResult {
    pub code: String,
    pub server: RelayLoginServer,
}

pub async fn wait_for_handoff_relay_login_code(
    context: &Context,
    options: RelayServerOptions,
    accepts_callback: Option<CallbackAccepter>,
    login_url: &str,
    hooks: Arc<dyn LoginBridgeHooks>,
) -> Result<RelayLoginResult, BridgeError> {
    canonical_public_url(&options.public_url)?;
    let listener = crate::login_bridge::bind_login_listener(&options.listen_addr).await?;
    wait_for_handoff_relay_login_code_on_listener(
        context,
        options,
        accepts_callback,
        login_url,
        hooks,
        listener,
    )
    .await
}

pub async fn wait_for_handoff_relay_login_code_on_listener(
    context: &Context,
    options: RelayServerOptions,
    accepts_callback: Option<CallbackAccepter>,
    login_url: &str,
    hooks: Arc<dyn LoginBridgeHooks>,
    listener: TcpListener,
) -> Result<RelayLoginResult, BridgeError> {
    let public_url = canonical_public_url(&options.public_url)?;
    let session_id = new_id()?;
    let proof = new_id()?;
    let result_id = new_id()?;
    http::wait(
        context,
        options,
        accepts_callback,
        login_url,
        hooks,
        listener,
        SessionCapabilities {
            public_url,
            session_id,
            proof,
            result_id,
        },
    )
    .await
}
