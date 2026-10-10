use crate::{
    CommandError,
    diagnostics::{Clock, Presenter},
    finish_with_cleanup,
};
use pixiv_app::{config::RuntimeConfig, update::CallerContext};
use pixiv_sdk::{
    context::{Context, ContextError, ContextFuture, ContextKey, ContextValue, RequestContext},
    diagnostics::{Event, Scope},
};
use std::{
    any::TypeId,
    fmt,
    io::{self, Write},
    sync::{Arc, Mutex},
    time::Instant,
};

pub struct CommandDiagnostics {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    clock: Option<Clock>,
    active: Option<Active>,
}
struct Active {
    presenter: Arc<Presenter>,
    scope: Scope,
    operation: String,
}
struct SharedWriter(Arc<Mutex<Box<dyn Write + Send>>>);
impl Write for SharedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .flush()
    }
}
impl CommandDiagnostics {
    pub fn new(writer: Option<Box<dyn Write + Send>>, clock: Option<Clock>) -> Self {
        Self {
            writer: Arc::new(Mutex::new(writer.unwrap_or_else(|| Box::new(io::sink())))),
            clock,
            active: None,
        }
    }
    pub fn start(&mut self, runtime: &RuntimeConfig, operation: &str) {
        if runtime.log_level != "debug"
            || operation == "pixiv config"
            || operation.starts_with("pixiv config ")
        {
            return;
        }
        let module = if operation.starts_with("pixiv fanbox") {
            "FANBOX CLI"
        } else {
            "Pixiv CLI"
        };
        let presenter = Arc::new(Presenter::with_format(
            Some(Box::new(SharedWriter(self.writer.clone()))),
            runtime.log_format.clone(),
            self.clock.clone(),
        ));
        let scope = Scope::new(Some(presenter.clone()), module, 0);
        self.active = Some(Active {
            presenter,
            scope: scope.clone(),
            operation: operation.into(),
        });
        scope.emit(Event {
            kind: "started".into(),
            operation: operation.into(),
            ..Event::default()
        });
    }
    pub fn context(&self, context: &Context) -> Context {
        match &self.active {
            Some(active) => context.with_scope(active.scope.clone()),
            None => context.clone(),
        }
    }
    pub fn caller_context(&self, context: CallerContext) -> CallerContext {
        match &self.active {
            Some(active) => Arc::new(ScopedContext {
                parent: context,
                scope: active.scope.clone(),
            }),
            None => context,
        }
    }
    pub fn finish(&self, result: Result<(), CommandError>) -> Result<(), CommandError> {
        let Some(active) = &self.active else {
            return result;
        };
        active.scope.emit(Event {
            kind: if result.is_ok() {
                "completed"
            } else {
                "failed"
            }
            .into(),
            operation: active.operation.clone(),
            reason: if result.is_ok() { "" } else { "command failed" }.into(),
            ..Event::default()
        });
        let diagnostics = match active.presenter.error() {
            Some(error) => Err(CommandError::State(Box::new(DiagnosticWriteError(error)))),
            None => Ok(()),
        };
        finish_with_cleanup(result, diagnostics)
    }
}
#[derive(Debug)]
struct ScopedContext {
    parent: CallerContext,
    scope: Scope,
}
impl RequestContext for ScopedContext {
    fn error(&self) -> Option<ContextError> {
        self.parent.error()
    }
    fn deadline(&self) -> Option<Instant> {
        self.parent.deadline()
    }
    fn cancelled(&self) -> ContextFuture<'_> {
        self.parent.cancelled()
    }
    fn value(&self, key: &ContextKey) -> Option<ContextValue> {
        self.parent.value(key)
    }
    fn extension(&self, id: TypeId) -> Option<ContextValue> {
        self.parent.extension(id)
    }
    fn scope(&self) -> Option<&Scope> {
        Some(&self.scope)
    }
}
#[derive(Debug)]
struct DiagnosticWriteError(Arc<io::Error>);
impl fmt::Display for DiagnosticWriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("write diagnostics: ")?;
        if cfg!(unix) && self.0.raw_os_error() == Some(32) {
            f.write_str("broken pipe")
        } else {
            self.0.fmt(f)
        }
    }
}
impl std::error::Error for DiagnosticWriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref())
    }
}
