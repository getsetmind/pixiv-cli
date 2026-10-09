use super::{ConfigError, PrivateWriteOutcome};
use std::{
    fs, io,
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
};
use windows_sys::Win32::{
    Foundation::ERROR_UNABLE_TO_MOVE_REPLACEMENT_2,
    Storage::FileSystem::{MoveFileExW, ReplaceFileW},
};

pub(super) fn replace(
    source: &Path,
    target: &Path,
) -> Result<(), (PrivateWriteOutcome, ConfigError)> {
    let source_wide = encode_path(source).map_err(not_committed)?;
    let target_wide = encode_path(target).map_err(not_committed)?;
    match fs::symlink_metadata(target) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return move_file(&source_wide, &target_wide).map_err(not_committed);
        }
        Err(error) => return Err(not_committed(error)),
    }

    let mut backup_name = source.as_os_str().to_os_string();
    backup_name.push(".recovery");
    let backup = PathBuf::from(backup_name);
    let backup_wide = encode_path(&backup).map_err(not_committed)?;
    // ReplaceFileW は WRITE_THROUGH をサポートせず、MoveFileEx の上書きとは同等ではない。
    let replaced = unsafe {
        ReplaceFileW(
            target_wide.as_ptr(),
            source_wide.as_ptr(),
            backup_wide.as_ptr(),
            0,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if replaced != 0 {
        return fs::remove_file(&backup).map_err(|error| {
            (
                PrivateWriteOutcome::Committed,
                contextual_io("remove committed replacement backup", error),
            )
        });
    }

    let replacement_error = io::Error::last_os_error();
    if replacement_error.raw_os_error() != Some(ERROR_UNABLE_TO_MOVE_REPLACEMENT_2 as i32) {
        return Err(not_committed(replacement_error));
    }

    // 復元時に上書きすると、並行作成された target を失い、復元材料も残せない。
    match move_file(&backup_wide, &target_wide) {
        Ok(()) => Err(not_committed(replacement_error)),
        Err(restore_error) => Err((
            PrivateWriteOutcome::Unknown,
            ConfigError::Joined(vec![
                ConfigError::Io(replacement_error),
                contextual_io("restore replaced target from backup", restore_error),
            ]),
        )),
    }
}

fn encode_path(path: &Path) -> io::Result<Vec<u16>> {
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid argument",
        ));
    }
    wide.push(0);
    Ok(wide)
}

fn move_file(source: &[u16], target: &[u16]) -> io::Result<()> {
    if unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), 0) } != 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn not_committed(error: io::Error) -> (PrivateWriteOutcome, ConfigError) {
    (PrivateWriteOutcome::NotCommitted, ConfigError::Io(error))
}

fn contextual_io(context: &'static str, source: io::Error) -> ConfigError {
    ConfigError::Operation(context, Box::new(ConfigError::Io(source)))
}
