use std::{
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    sync::{Arc, Mutex},
};

pub type Release<E> = Box<dyn FnOnce() -> Result<(), E> + Send>;

pub struct Lease<T, E> {
    value: T,
    state: Mutex<ReleaseState<E>>,
}
struct ReleaseState<E> {
    release: Option<Release<E>>,
    result: Option<Result<(), Arc<E>>>,
}

impl<T, E> Lease<T, E> {
    pub fn new(value: T, release: Option<Release<E>>) -> Self {
        Self {
            value,
            state: Mutex::new(ReleaseState {
                release,
                result: None,
            }),
        }
    }
    pub fn value(&self) -> &T {
        &self.value
    }
    pub fn close(&self) -> Result<(), Arc<E>> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if let Some(result) = &state.result {
            return result.clone();
        }
        let release = state.release.take();
        match catch_unwind(AssertUnwindSafe(|| {
            release.map_or(Ok(()), |release| release())
        })) {
            Ok(result) => {
                let result = result.map_err(Arc::new);
                state.result = Some(result.clone());
                result
            }
            Err(panic) => {
                state.result = Some(Ok(()));
                drop(state);
                resume_unwind(panic)
            }
        }
    }
}
