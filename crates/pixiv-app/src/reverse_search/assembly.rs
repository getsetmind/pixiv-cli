use super::{
    Aggregator, AggregatorDependencies, Dependencies, Error, Facade, Loader, SourceLoaderOptions,
    ascii2d, http::ReqwestTransport, saucenao,
};
use crate::config::RuntimeConfig;
use std::sync::Arc;

#[derive(Clone, Default, Eq, PartialEq)]
pub struct FlareSolverrOptions {
    pub url: String,
    pub proxy_url: String,
}

#[derive(Clone, Default, Eq, PartialEq)]
pub struct Options {
    pub proxy: String,
    pub ascii2d_proxy: Option<String>,
    pub user_agent: String,
    pub saucenao_key: String,
    pub flaresolverr: Option<FlareSolverrOptions>,
}

impl Options {
    pub fn from_runtime(runtime: &RuntimeConfig, override_proxy: Option<&str>) -> Self {
        let (proxy, ascii2d_proxy) = match override_proxy {
            Some(proxy) => (proxy.to_owned(), proxy.to_owned()),
            None => (
                runtime.https_proxy.clone(),
                runtime
                    .reverse_search_network
                    .proxy_url
                    .clone()
                    .unwrap_or_else(|| runtime.https_proxy.clone()),
            ),
        };
        Self {
            proxy,
            ascii2d_proxy: Some(ascii2d_proxy),
            user_agent: runtime
                .reverse_search_network
                .user_agent
                .clone()
                .unwrap_or_default(),
            saucenao_key: runtime.saucenao_api_key.clone(),
            flaresolverr: runtime.reverse_search_flaresolverr.as_ref().map(|solver| {
                FlareSolverrOptions {
                    url: solver.url.clone(),
                    proxy_url: solver.proxy_url.clone(),
                }
            }),
        }
    }
}

pub fn build(options: Options) -> Result<Arc<Facade>, Error> {
    let standard = Arc::new(ReqwestTransport::new(&options.proxy).map_err(Error::from_box)?);
    let ascii2d_proxy = options
        .ascii2d_proxy
        .unwrap_or_else(|| options.proxy.clone());
    let ascii2d = Arc::new(ascii2d::Client::new(ascii2d::Options {
        proxy_url: ascii2d_proxy,
        user_agent: options.user_agent,
        flaresolverr: options
            .flaresolverr
            .map(|solver| ascii2d::FlareSolverrOptions {
                url: solver.url,
                proxy_url: solver.proxy_url,
                ..ascii2d::FlareSolverrOptions::default()
            }),
        ..ascii2d::Options::default()
    })?);
    let sauce_nao = Arc::new(saucenao::Client::new(saucenao::Options {
        api_key: options.saucenao_key,
        transport: Some(standard.clone()),
        ..saucenao::Options::default()
    }));
    let sources = Arc::new(Loader::new(SourceLoaderOptions {
        http_transport: Some(standard),
        ..SourceLoaderOptions::default()
    }));
    let payloads = Arc::new(Aggregator::new(AggregatorDependencies {
        sauce_nao: Some(sauce_nao),
        ascii2d: Some(ascii2d),
    }));
    Ok(Arc::new(Facade::new(Dependencies {
        sources: Some(sources),
        payloads: Some(payloads),
    })))
}
