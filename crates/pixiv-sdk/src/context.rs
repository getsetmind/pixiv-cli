//! SDK caller context and opt-in diagnostic ownership.
//!
//! Go callers use standard `context.Context`; this is a Rust API mapping,
//! not an SDK context API from Go. Owned `Any + Eq` keys preserve Rust key
//! type namespaces and equality. Go's arbitrary comparable keys, interface
//! identities, nil-constructor panics, concrete error types, and wall-clock
//! deadline timestamps are not source-compatible Rust contracts.
//!
//! Deadlines use monotonic `Instant`. Constructors work without a runtime:
//! `error()` observes elapsed deadlines and `cancelled()` owns its awaited
//! Tokio timer. There is no independently running Go-style Done-channel timer
//! or background task. Dropping a notification future releases its timer.

use crate::diagnostics::{Event, Scope};
use std::{
    any::{Any, TypeId},
    fmt,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex, Weak},
    time::Instant,
};
use tokio_util::sync::CancellationToken;

pub type ContextFuture<'a> = Pin<Box<dyn Future<Output = ContextError> + Send + 'a>>;
pub type ContextValue = Arc<dyn Any + Send + Sync>;

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

trait ErasedKey: Send + Sync {
    fn as_any(&self) -> &dyn Any;
    fn equals(&self, other: &dyn ErasedKey) -> bool;
}
impl<T: Any + Eq + Send + Sync> ErasedKey for T {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn equals(&self, other: &dyn ErasedKey) -> bool {
        other
            .as_any()
            .downcast_ref::<T>()
            .is_some_and(|key| self == key)
    }
}

#[derive(Clone)]
pub struct ContextKey(Arc<dyn ErasedKey>);
impl ContextKey {
    pub fn new<T: Any + Eq + Send + Sync>(key: T) -> Self {
        Self(Arc::new(key))
    }
}
impl PartialEq for ContextKey {
    fn eq(&self, other: &Self) -> bool {
        self.0.equals(other.0.as_ref())
    }
}
impl Eq for ContextKey {}
impl fmt::Debug for ContextKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContextKey").finish_non_exhaustive()
    }
}

/// Raw dependencies receive the caller's exact `Arc<dyn RequestContext>`.
/// A dependency may observe cancellation or return a response despite it;
/// forwarding context is not permission to reject a caller unconditionally.
pub trait RequestContext: Send + Sync + fmt::Debug {
    fn error(&self) -> Option<ContextError>;
    fn deadline(&self) -> Option<Instant>;
    fn cancelled(&self) -> ContextFuture<'_>;
    fn value(&self, _key: &ContextKey) -> Option<ContextValue> {
        None
    }
    fn extension(&self, _type_id: TypeId) -> Option<ContextValue> {
        None
    }
    fn scope(&self) -> Option<&Scope> {
        None
    }
    fn emit(&self, event: Event) {
        if let Some(scope) = self.scope() {
            scope.emit(event);
        }
    }
}

#[derive(Clone)]
pub struct Context {
    state: Arc<ContextState>,
    metadata: Option<Arc<Metadata>>,
    scope: Option<Scope>,
}
impl fmt::Debug for Context {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Context")
            .field("cancelable", &self.state.cancelable)
            .field("deadline", &self.state.deadline)
            .field("scope", &self.scope)
            .finish_non_exhaustive()
    }
}

struct ContextState {
    cancelable: bool,
    reason: Mutex<Option<Completion>>,
    deadline: Option<Instant>,
    token: CancellationToken,
    parent: Option<Arc<ContextState>>,
    children: Mutex<Vec<Weak<ContextState>>>,
}

#[derive(Clone, Copy)]
struct Completion {
    error: ContextError,
    at: Instant,
}

#[derive(Eq, PartialEq)]
enum MetadataKey {
    Value(ContextKey),
    Extension(TypeId),
}
struct Metadata {
    key: MetadataKey,
    value: Option<ContextValue>,
    parent: Option<Arc<Metadata>>,
}

impl ContextState {
    fn new(cancelable: bool, deadline: Option<Instant>, parent: Option<Arc<Self>>) -> Self {
        Self {
            cancelable,
            reason: Mutex::new(None),
            deadline,
            token: CancellationToken::new(),
            parent,
            children: Mutex::new(Vec::new()),
        }
    }

    fn completion(&self) -> Option<Completion> {
        *self
            .reason
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn complete(&self, error: ContextError, at: Instant) {
        let mut reason = self
            .reason
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if reason.is_some() || !self.cancelable {
            return;
        }
        let (error, at) = match self.deadline {
            Some(deadline) if deadline <= at => (ContextError::DeadlineExceeded, deadline),
            _ => (error, at),
        };
        *reason = Some(Completion { error, at });
        drop(reason);
        self.token.cancel();
        if let Some(parent) = &self.parent {
            parent
                .children
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .retain(|child| child.strong_count() != 0 && !std::ptr::eq(child.as_ptr(), self));
        }
        let children = self
            .children
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .drain(..)
            .filter_map(|child| child.upgrade())
            .collect::<Vec<_>>();
        for child in children {
            child.complete(error, at);
        }
    }

    fn deadline_elapsed(&self, deadline: Instant) {
        if self.error().is_some() {
            return;
        }
        if let Some(parent) = &self.parent
            && let Some(parent_deadline) = parent.deadline.filter(|parent| *parent <= deadline)
        {
            parent.deadline_elapsed(parent_deadline);
        }
        self.complete(ContextError::DeadlineExceeded, deadline);
    }

    fn error(&self) -> Option<ContextError> {
        if let Some(completed) = self.completion() {
            return Some(completed.error);
        }
        if let Some(parent) = &self.parent {
            let _ = parent.error();
            if let Some(completed) = parent.completion() {
                self.complete(completed.error, completed.at);
                return self.completion().map(|completed| completed.error);
            }
        }
        if let Some(deadline) = self.deadline.filter(|deadline| Instant::now() >= *deadline) {
            self.complete(ContextError::DeadlineExceeded, deadline);
        }
        self.completion().map(|completed| completed.error)
    }
}

impl Default for Context {
    fn default() -> Self {
        Self::new()
    }
}
impl Context {
    pub fn new() -> Self {
        Self::root(true, None)
    }

    pub fn background() -> Self {
        Self::root(false, None)
    }

    pub fn todo() -> Self {
        Self::background()
    }

    pub fn with_deadline(deadline: Instant) -> Self {
        Self::root(true, Some(deadline))
    }

    fn root(cancelable: bool, deadline: Option<Instant>) -> Self {
        let context = Self {
            state: Arc::new(ContextState::new(cancelable, deadline, None)),
            metadata: None,
            scope: None,
        };
        let _ = context.error();
        context
    }

    pub fn child(&self) -> Self {
        self.make_child(self.deadline())
    }

    pub fn child_with_deadline(&self, deadline: Instant) -> Self {
        self.make_child(Some(
            self.deadline()
                .map_or(deadline, |parent| parent.min(deadline)),
        ))
    }

    fn make_child(&self, deadline: Option<Instant>) -> Self {
        let _ = self.error();
        let state = Arc::new(ContextState::new(true, deadline, Some(self.state.clone())));
        let mut children = self
            .state
            .children
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if let Some(completed) = self.state.completion() {
            // A parent's completed error precedes a deadline requested while
            // constructing a new child, even when that deadline is in the past.
            *state
                .reason
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()) = Some(completed);
            state.token.cancel();
        } else {
            children.retain(|child| child.strong_count() != 0);
            children.push(Arc::downgrade(&state));
        }
        drop(children);
        let context = Self {
            state,
            metadata: self.metadata.clone(),
            scope: self.scope.clone(),
        };
        let _ = context.error();
        context
    }

    pub fn without_cancel(&self) -> Self {
        Self {
            state: Arc::new(ContextState::new(false, None, None)),
            metadata: self.metadata.clone(),
            scope: self.scope.clone(),
        }
    }

    pub fn with_value<T: Any + Send + Sync>(&self, key: ContextKey, value: Arc<T>) -> Self {
        self.with_metadata(MetadataKey::Value(key), Some(value))
    }

    pub fn without_value(&self, key: ContextKey) -> Self {
        self.with_metadata(MetadataKey::Value(key), None)
    }

    pub fn value<T: Any + Send + Sync>(&self, key: &ContextKey) -> Option<Arc<T>> {
        self.lookup(&MetadataKey::Value(key.clone()))?
            .downcast()
            .ok()
    }

    pub fn with_extension<T: Any + Send + Sync>(&self, value: Arc<T>) -> Self {
        self.with_metadata(MetadataKey::Extension(TypeId::of::<T>()), Some(value))
    }

    pub fn without_extension<T: Any + Send + Sync>(&self) -> Self {
        self.with_metadata(MetadataKey::Extension(TypeId::of::<T>()), None)
    }

    pub fn extension<T: Any + Send + Sync>(&self) -> Option<Arc<T>> {
        self.lookup(&MetadataKey::Extension(TypeId::of::<T>()))?
            .downcast()
            .ok()
    }

    fn with_metadata(&self, key: MetadataKey, value: Option<ContextValue>) -> Self {
        Self {
            state: self.state.clone(),
            metadata: Some(Arc::new(Metadata {
                key,
                value,
                parent: self.metadata.clone(),
            })),
            scope: self.scope.clone(),
        }
    }

    fn lookup(&self, key: &MetadataKey) -> Option<ContextValue> {
        let mut entry = self.metadata.as_deref();
        while let Some(metadata) = entry {
            if &metadata.key == key {
                return metadata.value.clone();
            }
            entry = metadata.parent.as_deref();
        }
        None
    }

    pub fn with_scope(&self, scope: Scope) -> Self {
        Self {
            state: self.state.clone(),
            metadata: self.metadata.clone(),
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
        RequestContext::emit(self, event);
    }

    pub fn cancel(&self) {
        let _ = self.error();
        self.state.complete(ContextError::Canceled, Instant::now());
    }

    pub fn error(&self) -> Option<ContextError> {
        self.state.error()
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.state.deadline
    }

    pub async fn cancelled(&self) -> ContextError {
        if let Some(error) = self.error() {
            return error;
        }
        if let Some(deadline) = self.deadline() {
            tokio::select! {
                _ = self.state.token.cancelled() => {},
                _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {
                    self.state.deadline_elapsed(deadline);
                },
            }
        } else {
            self.state.token.cancelled().await;
        }
        self.error().unwrap_or(ContextError::Canceled)
    }
}

impl RequestContext for Context {
    fn error(&self) -> Option<ContextError> {
        Context::error(self)
    }
    fn deadline(&self) -> Option<Instant> {
        Context::deadline(self)
    }
    fn cancelled(&self) -> ContextFuture<'_> {
        Box::pin(Context::cancelled(self))
    }
    fn value(&self, key: &ContextKey) -> Option<ContextValue> {
        self.lookup(&MetadataKey::Value(key.clone()))
    }
    fn extension(&self, type_id: TypeId) -> Option<ContextValue> {
        self.lookup(&MetadataKey::Extension(type_id))
    }
    fn scope(&self) -> Option<&Scope> {
        Context::scope(self)
    }
}
