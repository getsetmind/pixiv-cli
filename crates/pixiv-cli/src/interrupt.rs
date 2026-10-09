use pixiv_app::lifecycle::Context;
use std::io;
use tokio::task::JoinHandle;

pub struct OwnedSignalContext {
    context: Context,
    watcher: JoinHandle<()>,
}

impl OwnedSignalContext {
    pub fn new() -> io::Result<Self> {
        let context = Context::new();
        let observed = context.clone();
        #[cfg(unix)]
        let mut interrupt =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
        #[cfg(windows)]
        let mut interrupt = tokio::signal::windows::ctrl_c()?;
        #[cfg(windows)]
        let mut control_break = tokio::signal::windows::ctrl_break()?;
        let watcher = tokio::spawn(async move {
            #[cfg(unix)]
            let received = interrupt.recv().await.is_some();
            #[cfg(windows)]
            let received = tokio::select! {
                event = interrupt.recv() => event.is_some(),
                event = control_break.recv() => event.is_some(),
            };
            if received {
                observed.cancel();
            }
        });
        Ok(Self { context, watcher })
    }

    pub fn context(&self) -> Context {
        self.context.clone()
    }
}

impl Drop for OwnedSignalContext {
    fn drop(&mut self) {
        self.context.cancel();
        self.watcher.abort();
        // Tokio keeps its process-wide handler after drop; restoring it would disrupt other owners.
    }
}
