use super::ConfigError;
use std::{fs, io, path::Path};

pub(super) fn ensure_with_body(path: &Path, body: &[u8]) -> Result<(), ConfigError> {
    super::private_file::ensure_directory(super::private_file::directory(path))?;
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
        super::private_file::write_body(&mut file, body)?;
        file.sync_all()
    })();
    let closed = super::private_file::close(file);
    match written {
        Ok(()) => closed.map_err(ConfigError::Io),
        Err(error) => {
            let mut errors = vec![ConfigError::Io(error)];
            if let Err(error) = closed {
                errors.push(ConfigError::Io(error));
            }
            if let Err(error) = fs::remove_file(path)
                && error.kind() != io::ErrorKind::NotFound
            {
                errors.push(ConfigError::Io(error));
            }
            if errors.len() == 1 {
                Err(errors.remove(0))
            } else {
                Err(ConfigError::Joined(errors))
            }
        }
    }
}
