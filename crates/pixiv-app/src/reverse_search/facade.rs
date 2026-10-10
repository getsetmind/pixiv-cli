use super::{
    CallerContext, Error, ErrorCode, Input, PayloadQuery, PayloadRequest, PayloadSearcher, Request,
    ReverseFuture, SearchOutcome, Searcher, SourceLoader,
};
use futures_util::{FutureExt, future::Shared};
use std::sync::{Arc, Mutex};

type CloseFuture = Shared<ReverseFuture<'static, std::result::Result<(), Error>>>;

#[derive(Default)]
pub struct Dependencies {
    pub sources: Option<Arc<dyn SourceLoader>>,
    pub payloads: Option<Arc<dyn PayloadSearcher>>,
}
pub struct Facade {
    dependencies: Dependencies,
    close_future: Mutex<Option<CloseFuture>>,
}
impl Facade {
    pub fn new(dependencies: Dependencies) -> Self {
        Self {
            dependencies,
            close_future: Mutex::new(None),
        }
    }
    async fn search_inner(&self, context: CallerContext, request: Request) -> SearchOutcome {
        let Some(sources) = &self.dependencies.sources else {
            return SearchOutcome::failure(Error::new(
                ErrorCode::SourceLoaderNotConfigured,
                "reverse search source loader is not configured",
                None,
            ));
        };
        let Some(payloads) = &self.dependencies.payloads else {
            return SearchOutcome::failure(Error::new(
                ErrorCode::ProviderNotConfigured,
                "reverse search payload searcher is not configured",
                None,
            ));
        };
        if let Err(error) = payloads
            .preflight(
                context.clone(),
                PayloadQuery {
                    provider: request.provider.clone(),
                    pixiv_only: request.pixiv_only,
                },
            )
            .await
        {
            return SearchOutcome::failure(error);
        }
        let snapshot = match sources.load(context.clone(), &request.source).await {
            Ok(snapshot) => snapshot,
            Err(error) => return SearchOutcome::failure(error),
        };
        let mut outcome = payloads
            .search_payload(
                context,
                PayloadRequest {
                    snapshot: snapshot.clone(),
                    provider: request.provider,
                    pixiv_only: request.pixiv_only,
                },
            )
            .await;
        outcome.response.input = Input {
            kind: snapshot.kind(),
            sha256: snapshot.sha256().to_owned(),
        };
        if let Err(error) = snapshot.close() {
            outcome.error = Error::join(outcome.error.into_iter().chain(std::iter::once(error)));
        }
        outcome
    }
    pub async fn close(&self) -> std::result::Result<(), Error> {
        let future = {
            let mut stored = self
                .close_future
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            stored
                .get_or_insert_with(|| {
                    let payloads = self.dependencies.payloads.clone();
                    let future: ReverseFuture<'static, std::result::Result<(), Error>> =
                        Box::pin(async move {
                            if let Some(payloads) = payloads {
                                payloads.close().await
                            } else {
                                Ok(())
                            }
                        });
                    future.shared()
                })
                .clone()
        };
        future.await
    }
}
impl Searcher for Facade {
    fn search(&self, context: CallerContext, request: Request) -> ReverseFuture<'_, SearchOutcome> {
        Box::pin(self.search_inner(context, request))
    }
    fn close(&self) -> ReverseFuture<'_, std::result::Result<(), Error>> {
        Box::pin(Facade::close(self))
    }
}
