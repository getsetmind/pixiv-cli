use super::{
    ASCII2DClient, CallerContext, Error, ErrorCode, PayloadQuery, PayloadRequest, PayloadSearcher,
    Provider, ProviderClient, ProviderError, ProviderResponse, ProviderStatus, ProviderSummary,
    Response, ReverseFuture, SearchOutcome, Snapshot, normalize::append_matches,
};
use futures_util::{FutureExt, future::Shared};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

type CloseFuture = Shared<ReverseFuture<'static, std::result::Result<(), Error>>>;

#[derive(Default)]
pub struct AggregatorDependencies {
    pub sauce_nao: Option<Arc<dyn ProviderClient>>,
    pub ascii2d: Option<Arc<dyn ASCII2DClient>>,
}
pub struct Aggregator {
    dependencies: AggregatorDependencies,
    close_future: Mutex<Option<CloseFuture>>,
}
impl Aggregator {
    pub fn new(dependencies: AggregatorDependencies) -> Self {
        Self {
            dependencies,
            close_future: Mutex::new(None),
        }
    }
    async fn preflight_inner(
        &self,
        context: CallerContext,
        query: PayloadQuery,
    ) -> std::result::Result<(), Error> {
        if let Some(error) = context.error() {
            return Err(error.into());
        }
        match query.provider {
            Provider::SauceNao => {
                self.dependencies
                    .sauce_nao
                    .as_ref()
                    .ok_or_else(missing_sauce)?
                    .preflight(context)
                    .await
            }
            Provider::Ascii2dColor | Provider::Ascii2dBovw => {
                self.dependencies
                    .ascii2d
                    .as_ref()
                    .ok_or_else(missing_ascii)?
                    .preflight(context)
                    .await
            }
            Provider::All => Ok(()),
            _ => Err(invalid_provider()),
        }
    }
    async fn search_inner(&self, context: CallerContext, request: PayloadRequest) -> SearchOutcome {
        if let Some(error) = context.error() {
            return SearchOutcome::failure(error.into());
        }
        if request.provider == Provider::All {
            return self
                .search_all(context, request.snapshot, request.pixiv_only)
                .await;
        }
        let response = self
            .search_single(context.clone(), request.provider.clone(), request.snapshot)
            .await;
        match response {
            Err(error) => {
                if let Some(error) = cancellation(&context, Some(&error)) {
                    return SearchOutcome::failure(error);
                }
                let (provider_error, safe_error) = safe_failure(request.provider.clone(), &error);
                SearchOutcome {
                    response: Response {
                        providers: Some(vec![ProviderSummary {
                            name: request.provider,
                            status: ProviderStatus::Error,
                            result_count: 0,
                            quota: None,
                        }]),
                        results: Some(Vec::new()),
                        provider_errors: Some(vec![provider_error]),
                        ..Response::default()
                    },
                    error: Some(safe_error),
                }
            }
            Ok(response) => assemble(vec![(request.provider, Ok(response))], request.pixiv_only),
        }
    }
    async fn search_single(
        &self,
        context: CallerContext,
        provider: Provider,
        snapshot: Arc<Snapshot>,
    ) -> std::result::Result<ProviderResponse, Error> {
        match provider {
            Provider::SauceNao => {
                self.dependencies
                    .sauce_nao
                    .as_ref()
                    .ok_or_else(missing_sauce)?
                    .search(context, snapshot)
                    .await
            }
            Provider::Ascii2dColor | Provider::Ascii2dBovw => {
                let session = self
                    .dependencies
                    .ascii2d
                    .as_ref()
                    .ok_or_else(missing_ascii)?
                    .upload(context.clone(), snapshot)
                    .await?;
                session.search(context, provider).await
            }
            _ => Err(invalid_provider()),
        }
    }
    async fn search_all(
        &self,
        context: CallerContext,
        snapshot: Arc<Snapshot>,
        pixiv_only: bool,
    ) -> SearchOutcome {
        let sauce = async {
            let client = self
                .dependencies
                .sauce_nao
                .as_ref()
                .ok_or_else(missing_sauce)?;
            client.preflight(context.clone()).await?;
            client.search(context.clone(), snapshot.clone()).await
        };
        let ascii = async {
            let client = match self.dependencies.ascii2d.as_ref() {
                Some(client) => client,
                None => {
                    let error = missing_ascii();
                    return (Err(error.clone()), Err(error));
                }
            };
            if let Err(error) = client.preflight(context.clone()).await {
                return (Err(error.clone()), Err(error));
            }
            let session = match client.upload(context.clone(), snapshot.clone()).await {
                Ok(session) => session,
                Err(error) => return (Err(error.clone()), Err(error)),
            };
            tokio::join!(
                session.search(context.clone(), Provider::Ascii2dColor),
                session.search(context.clone(), Provider::Ascii2dBovw)
            )
        };
        let (sauce, (color, bovw)) = tokio::join!(sauce, ascii);
        let outcomes = vec![
            (Provider::SauceNao, sauce),
            (Provider::Ascii2dColor, color),
            (Provider::Ascii2dBovw, bovw),
        ];
        for (_, outcome) in &outcomes {
            if let Some(error) = cancellation(&context, outcome.as_ref().err()) {
                return SearchOutcome::failure(error);
            }
        }
        assemble(outcomes, pixiv_only)
    }
    pub async fn close(&self) -> std::result::Result<(), Error> {
        let future = {
            let mut stored = self
                .close_future
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            stored
                .get_or_insert_with(|| {
                    let ascii = self.dependencies.ascii2d.clone();
                    let sauce = self.dependencies.sauce_nao.clone();
                    let future: ReverseFuture<'static, std::result::Result<(), Error>> =
                        Box::pin(async move {
                            let mut error = None;
                            if let Some(client) = ascii {
                                error = Error::join(client.close().await.err());
                            }
                            if let Some(client) = sauce {
                                error = Error::join(
                                    error.into_iter().chain(client.close().await.err()),
                                );
                            }
                            match error {
                                Some(error) => Err(error),
                                None => Ok(()),
                            }
                        });
                    future.shared()
                })
                .clone()
        };
        future.await
    }
}
impl PayloadSearcher for Aggregator {
    fn preflight(
        &self,
        context: CallerContext,
        query: PayloadQuery,
    ) -> ReverseFuture<'_, std::result::Result<(), Error>> {
        Box::pin(self.preflight_inner(context, query))
    }
    fn search_payload(
        &self,
        context: CallerContext,
        request: PayloadRequest,
    ) -> ReverseFuture<'_, SearchOutcome> {
        Box::pin(self.search_inner(context, request))
    }
    fn close(&self) -> ReverseFuture<'_, std::result::Result<(), Error>> {
        Box::pin(Aggregator::close(self))
    }
}
fn assemble(
    outcomes: Vec<(Provider, std::result::Result<ProviderResponse, Error>)>,
    pixiv_only: bool,
) -> SearchOutcome {
    let count = outcomes.len();
    let mut providers = Vec::with_capacity(count);
    let mut results = Vec::new();
    let mut provider_errors = Vec::new();
    let mut canonical = HashMap::new();
    let mut successes = 0;
    for (provider, outcome) in outcomes {
        match outcome {
            Err(error) => {
                let (provider_error, _) = safe_failure(provider.clone(), &error);
                providers.push(ProviderSummary {
                    name: provider,
                    status: ProviderStatus::Error,
                    result_count: 0,
                    quota: None,
                });
                provider_errors.push(provider_error);
            }
            Ok(response) => {
                successes += 1;
                providers.push(ProviderSummary {
                    name: provider.clone(),
                    status: ProviderStatus::Success,
                    result_count: response.matches.len() as i64,
                    quota: response.quota.clone(),
                });
                append_matches(
                    &mut results,
                    &mut canonical,
                    provider,
                    &response.matches,
                    pixiv_only,
                );
            }
        }
    }
    SearchOutcome {
        response: Response {
            providers: Some(providers),
            results: Some(results),
            provider_errors: Some(provider_errors),
            partial: successes != 0 && successes != count,
            ..Response::default()
        },
        error: (successes == 0).then(|| {
            Error::new(
                ErrorCode::AllProvidersFailed,
                "all reverse search providers failed",
                None,
            )
        }),
    }
}
fn cancellation(context: &CallerContext, error: Option<&Error>) -> Option<Error> {
    context.error().map(Error::from).or_else(|| {
        error
            .filter(|error| error.context_error().is_some())
            .cloned()
    })
}
fn safe_failure(provider: Provider, error: &Error) -> (ProviderError, Error) {
    let (code, message) = error
        .classified()
        .unwrap_or((ErrorCode::ProviderFailed, "reverse search provider failed"));
    (
        ProviderError {
            provider,
            code,
            message: message.to_owned(),
        },
        Error::new(code, message, None),
    )
}
fn missing_sauce() -> Error {
    Error::new(
        ErrorCode::ProviderNotConfigured,
        "SauceNAO provider is not configured",
        None,
    )
}
fn missing_ascii() -> Error {
    Error::new(
        ErrorCode::ProviderNotConfigured,
        "ascii2d provider is not configured",
        None,
    )
}
fn invalid_provider() -> Error {
    Error::new(
        ErrorCode::InvalidRequest,
        "reverse search provider is invalid",
        None,
    )
}
