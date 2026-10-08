use crate::diagnostics::{Event, Scope};
use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
    time::Instant,
};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Default)]
pub struct Attempt {
    committed: AtomicBool,
}
impl Attempt {
    pub fn commit(&self) {
        self.committed.store(true, Ordering::SeqCst);
    }
    pub fn committed(&self) -> bool {
        self.committed.load(Ordering::SeqCst)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContextError {
    Canceled,
    DeadlineExceeded,
}
impl fmt::Display for ContextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Canceled => "context canceled",
            Self::DeadlineExceeded => "context deadline exceeded",
        })
    }
}
impl std::error::Error for ContextError {}

#[derive(Clone, Debug)]
pub struct Context {
    state: Arc<ContextState>,
    scope: Option<Scope>,
}
#[derive(Debug)]
struct ContextState {
    reason: AtomicU8,
    deadline: Option<Instant>,
    token: CancellationToken,
}
impl Default for Context {
    fn default() -> Self {
        Self::new()
    }
}
impl Context {
    pub fn new() -> Self {
        Self::make(None)
    }
    pub fn with_deadline(deadline: Instant) -> Self {
        Self::make(Some(deadline))
    }
    fn make(deadline: Option<Instant>) -> Self {
        Self {
            state: Arc::new(ContextState {
                reason: AtomicU8::new(0),
                deadline,
                token: CancellationToken::new(),
            }),
            scope: None,
        }
    }
    pub fn with_scope(&self, scope: Scope) -> Self {
        Self {
            state: Arc::clone(&self.state),
            scope: Some(scope),
        }
    }
    pub fn with_child_scope(&self, module: impl Into<String>, request_id: u64) -> Self {
        match &self.scope {
            Some(scope) => self.with_scope(scope.child(module, request_id)),
            None => self.clone(),
        }
    }
    pub fn scope(&self) -> Option<&Scope> {
        self.scope.as_ref()
    }
    pub fn emit(&self, event: Event) {
        if let Some(scope) = &self.scope {
            scope.emit(event);
        }
    }
    pub fn cancel(&self) {
        if self.error().is_none() {
            self.set_reason(1);
        }
    }
    fn set_reason(&self, reason: u8) {
        let _ = self
            .state
            .reason
            .compare_exchange(0, reason, Ordering::SeqCst, Ordering::SeqCst);
        self.state.token.cancel();
    }
    pub fn error(&self) -> Option<ContextError> {
        if self.state.reason.load(Ordering::SeqCst) == 0
            && self
                .state
                .deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.set_reason(2);
        }
        match self.state.reason.load(Ordering::SeqCst) {
            1 => Some(ContextError::Canceled),
            2 => Some(ContextError::DeadlineExceeded),
            _ => None,
        }
    }
    pub async fn cancelled(&self) -> ContextError {
        if let Some(error) = self.error() {
            return error;
        }
        if let Some(deadline) = self.state.deadline {
            tokio::select! {
                _=self.state.token.cancelled()=>{},
                _=tokio::time::sleep_until(tokio::time::Instant::from_std(deadline))=>{self.set_reason(2);},
            }
        } else {
            self.state.token.cancelled().await;
        }
        self.error().unwrap_or(ContextError::Canceled)
    }
}
