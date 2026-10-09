use crate::{
    account_management::AccountDefaultStore,
    account_service::{AccountService, AccountSummary},
    account_transfer::AccountTransfer,
    database::PixivAccount,
    lifecycle::{Context, ContextError},
    scheduler::SchedulerError,
};
use pixiv_sdk::{
    Error, Reason,
    error::{Cause, TransportKind},
    oauth::{Credentials, LoginSession},
    transport::{Request, Response, Transport},
};

#[derive(Clone, Debug, Default)]
pub struct LoginStart {
    session: Option<LoginSession>,
    pub authorization_url: String,
}
impl LoginStart {
    pub fn accepts_callback_url(&self, url: &str) -> bool {
        self.session
            .as_ref()
            .is_some_and(|session| session.accepts_callback_url(url))
    }
}
#[derive(Clone, Debug, Default)]
pub struct LoginCompleteRequest {
    pub callback_or_code: String,
    pub use_after_login: bool,
}
#[derive(Default)]
pub struct LoginService<'a> {
    pub pixiv: Option<&'a AccountService>,
    pub defaults: Option<&'a dyn AccountDefaultStore>,
}
impl LoginService<'_> {
    pub fn start(&self) -> Result<LoginStart, SchedulerError> {
        let session = LoginSession::begin()?;
        let authorization_url = session.authorization_url().to_owned();
        Ok(LoginStart {
            session: Some(session),
            authorization_url,
        })
    }
    pub async fn complete<T: Transport>(
        &self,
        context: &Context,
        start: &LoginStart,
        request: &LoginCompleteRequest,
        transport: &T,
    ) -> Result<AccountSummary, SchedulerError> {
        let session = start
            .session
            .as_ref()
            .ok_or_else(|| SchedulerError::Message("login session is not initialized".into()))?;
        let credentials = session
            .complete(
                &LoginTransport { context, transport },
                &request.callback_or_code,
            )
            .await?;
        let service = self.pixiv.ok_or_else(|| {
            SchedulerError::Message("pixiv account service is not configured".into())
        })?;
        service.complete_login(
            context,
            &credentials,
            request.use_after_login,
            self.defaults,
        )
    }
}
struct LoginTransport<'a, T> {
    context: &'a Context,
    transport: &'a T,
}
fn cancellation(operation: &'static str, error: ContextError) -> Error {
    let cause = match error {
        ContextError::Canceled => Cause::Canceled,
        ContextError::DeadlineExceeded => Cause::DeadlineExceeded,
    };
    Error::new(Reason::UpstreamUnavailable, operation)
        .with_transport(TransportKind::Http)
        .with_cause(Cause::TransportFailure(Box::new(cause)))
}
impl<T: Transport> Transport for LoginTransport<'_, T> {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        let operation = request.operation;
        if let Some(error) = self.context.error() {
            return Err(cancellation(operation, error));
        }
        tokio::select! {
            result = self.transport.send(request) => result,
            error = self.context.cancelled() => Err(cancellation(operation, error)),
        }
    }
}
fn wrapped(message: &str, source: SchedulerError) -> SchedulerError {
    SchedulerError::Wrapped {
        message: message.into(),
        source: Box::new(source),
    }
}
impl AccountService {
    pub fn complete_login(
        &self,
        context: &Context,
        credentials: &Credentials,
        set_default: bool,
        defaults: Option<&dyn AccountDefaultStore>,
    ) -> Result<AccountSummary, SchedulerError> {
        if credentials.user_id <= 0 || credentials.refresh_token().is_empty() {
            return Err(SchedulerError::Message(
                "login credentials are incomplete".into(),
            ));
        }
        let mut account = PixivAccount::new(
            credentials.user_id,
            &credentials.username,
            credentials.refresh_token().as_bytes(),
        );
        account.credential_revision = 1;
        self.repository
            .save_credential(context, &account)
            .map_err(|source| wrapped("save pixiv account", source))?;
        let defaults = defaults.ok_or_else(|| {
            wrapped(
                "read pixiv default account",
                SchedulerError::Message("pixiv default account store is not configured".into()),
            )
        })?;
        let selected = defaults
            .read()
            .map_err(|source| wrapped("read pixiv default account", source))?;
        if set_default || selected.is_none() {
            defaults
                .set(credentials.user_id)
                .map_err(|source| wrapped("set pixiv default account", source))?;
        }
        self.transfer(defaults)
            .summary(context, credentials.user_id)
    }
}
impl AccountTransfer<'_> {
    pub fn complete_login(
        &self,
        context: &Context,
        credentials: &Credentials,
        set_default: bool,
    ) -> Result<AccountSummary, SchedulerError> {
        self.service
            .complete_login(context, credentials, set_default, Some(self.defaults))
    }
}
