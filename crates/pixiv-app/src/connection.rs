use crate::config::RuntimeConfig;
use std::{fmt, time::Duration};

#[derive(Clone)]
pub struct CommandConnection {
    proxy: String,
    pacing: Duration,
}

#[derive(Clone, Copy)]
pub enum ProxyError {
    Parse,
    Unsupported,
}

impl fmt::Debug for ProxyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl fmt::Display for ProxyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Parse => "parse proxy URL: invalid proxy configuration",
            Self::Unsupported => {
                "proxy URL must use http, https, socks5, or socks5h: invalid proxy configuration"
            }
        })
    }
}

impl std::error::Error for ProxyError {}

impl CommandConnection {
    pub fn resolve(
        runtime: &RuntimeConfig,
        override_proxy: Option<&str>,
    ) -> Result<Self, ProxyError> {
        let proxy = override_proxy
            .or(runtime.pixiv_network.proxy_url.as_deref())
            .unwrap_or(&runtime.https_proxy);
        validate_proxy(proxy)?;
        Ok(Self {
            proxy: proxy.into(),
            pacing: runtime.request_interval,
        })
    }

    pub fn proxy(&self) -> &str {
        &self.proxy
    }
    pub fn pacing(&self) -> Duration {
        self.pacing
    }

    pub fn open_transport(&self) -> pixiv_sdk::Result<pixiv_sdk::transport::HttpTransport> {
        Ok(pixiv_sdk::transport::HttpTransport::new(Some(&self.proxy))?.with_pacing(self.pacing))
    }
}

fn validate_escapes(text: &str) -> Result<(), ProxyError> {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if bytes
                .get(index + 1)
                .is_none_or(|byte| !byte.is_ascii_hexdigit())
                || bytes
                    .get(index + 2)
                    .is_none_or(|byte| !byte.is_ascii_hexdigit())
            {
                return Err(ProxyError::Parse);
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    Ok(())
}

fn validate_proxy(text: &str) -> Result<(), ProxyError> {
    if text.is_empty() {
        return Ok(());
    }
    if text.starts_with(char::is_whitespace)
        || text.chars().any(|character| character.is_ascii_control())
    {
        return Err(ProxyError::Parse);
    }
    let (without_fragment, fragment) = text
        .split_once('#')
        .map_or((text, None), |(base, fragment)| (base, Some(fragment)));
    validate_escapes(
        without_fragment
            .split_once('?')
            .map_or(without_fragment, |(base, _)| base),
    )?;
    if let Some(fragment) = fragment {
        validate_escapes(fragment)?;
    }
    let Some((scheme, authority)) = without_fragment.split_once("://") else {
        return Err(ProxyError::Unsupported);
    };
    if !matches!(scheme, "http" | "https" | "socks5" | "socks5h")
        || authority.is_empty()
        || authority.starts_with('/')
    {
        return Err(ProxyError::Unsupported);
    }
    let parsed = url::Url::parse(text).map_err(|error| match error {
        url::ParseError::EmptyHost => ProxyError::Unsupported,
        _ => ProxyError::Parse,
    })?;
    if parsed.host_str().is_none_or(str::is_empty) {
        return Err(ProxyError::Unsupported);
    }
    Ok(())
}
