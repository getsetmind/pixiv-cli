use super::ConfigError;
use std::{
    fs,
    io::{self, Write},
    path::Path,
};

pub(super) fn ensure(path: &Path) -> Result<(), ConfigError> {
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        builder.mode(0o700);
        builder.create(directory).map_err(ConfigError::Io)?;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
            .map_err(ConfigError::Io)?;
    }
    #[cfg(not(unix))]
    builder.create(directory).map_err(ConfigError::Io)?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => return Ok(()),
        Err(error) => return Err(ConfigError::Io(error)),
    };
    let written = (|| {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(include_bytes!("default.toml"))?;
        file.sync_all()
    })();
    drop(file);
    match written {
        Ok(()) => Ok(()),
        Err(error) => match fs::remove_file(path) {
            Ok(()) => Err(ConfigError::Io(error)),
            Err(cleanup) if cleanup.kind() == io::ErrorKind::NotFound => {
                Err(ConfigError::Io(error))
            }
            Err(cleanup) => Err(ConfigError::Joined(vec![
                ConfigError::Io(error),
                ConfigError::Io(cleanup),
            ])),
        },
    }
}
