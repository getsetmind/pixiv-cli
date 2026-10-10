pub use crate::lease::Lease;
pub use pixiv_sdk::context::{Context, ContextError};
use std::sync::atomic::{AtomicBool, Ordering};

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
