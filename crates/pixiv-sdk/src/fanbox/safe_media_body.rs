use super::{
    safe_external_error,
    transport::{BodyFuture, ExternalError, RawBody, RawRead},
};
use crate::context::RequestContext;
use std::sync::Arc;

pub(super) struct SafeMediaBody {
    context: Arc<dyn RequestContext>,
    body: Box<dyn RawBody>,
}
impl SafeMediaBody {
    pub(super) fn new(context: Arc<dyn RequestContext>, body: Box<dyn RawBody>) -> Self {
        Self { context, body }
    }
}
impl RawBody for SafeMediaBody {
    fn read<'a>(&'a mut self, output: &'a mut [u8]) -> BodyFuture<'a, RawRead> {
        Box::pin(async move {
            let mut read = self.body.read(output).await;
            if let Some(error) = read.error.take() {
                read.error = Some(Box::new(safe_external_error(
                    self.context.as_ref(),
                    "read FANBOX media failed",
                    error.as_ref(),
                )));
            }
            read
        })
    }
    fn close(&mut self) -> BodyFuture<'_, std::result::Result<(), ExternalError>> {
        Box::pin(async move {
            self.body.close().await.map_err(|error| {
                Box::new(safe_external_error(
                    self.context.as_ref(),
                    "close FANBOX media failed",
                    error.as_ref(),
                )) as ExternalError
            })
        })
    }
}
