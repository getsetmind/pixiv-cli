use crate::scheduler::SchedulerError;
use std::path::Path;

pub(super) struct ReplacementError {
    pub error: SchedulerError,
    pub preserve_source: bool,
}

#[cfg(not(windows))]
pub(super) fn replace_file(source: &Path, target: &Path) -> Result<(), ReplacementError> {
    std::fs::rename(source, target).map_err(|error| ReplacementError {
        error: SchedulerError::Message(format!(
            "rename {} {}: {error}",
            source.display(),
            target.display()
        )),
        preserve_source: false,
    })
}

#[cfg(windows)]
pub(super) fn replace_file(source: &Path, target: &Path) -> Result<(), ReplacementError> {
    crate::config::private_replace_windows::replace(source, target).map_err(|(outcome, error)| {
        ReplacementError {
            error: error.into(),
            preserve_source: outcome == crate::config::PrivateWriteOutcome::Unknown,
        }
    })
}
