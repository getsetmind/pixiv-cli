use std::{fmt, sync::Arc};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Event {
    pub module: String,
    pub kind: String,
    pub operation: String,
    pub resource: String,
    pub route: String,
    pub target: String,
    pub proxy: String,
    pub user_agent: String,
    pub reason: String,
    pub status: i64,
    pub count: i64,
    pub request_id: u64,
    pub duration_ns: i64,
}

pub trait Sink: Send + Sync {
    fn emit(&self, event: Event);
}
impl<F: Fn(Event) + Send + Sync> Sink for F {
    fn emit(&self, event: Event) {
        self(event);
    }
}

#[derive(Clone, Default)]
pub struct Scope {
    sink: Option<Arc<dyn Sink>>,
    module: String,
    request_id: u64,
}
impl fmt::Debug for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Scope")
            .field("module", &self.module)
            .field("request_id", &self.request_id)
            .finish_non_exhaustive()
    }
}
impl Scope {
    pub fn new(sink: Option<Arc<dyn Sink>>, module: impl Into<String>, request_id: u64) -> Self {
        Self {
            sink,
            module: module.into(),
            request_id,
        }
    }
    pub fn child(&self, module: impl Into<String>, request_id: u64) -> Self {
        Self::new(self.sink.clone(), module, request_id)
    }
    pub fn emit(&self, mut event: Event) {
        let Some(sink) = &self.sink else {
            return;
        };
        if event.module.is_empty() {
            event.module.clone_from(&self.module);
        }
        event.request_id = self.request_id;
        sink.emit(event);
    }
}
