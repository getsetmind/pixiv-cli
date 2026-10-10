use super::{ConfigError, initialization, private_file};
use std::path::{Path, PathBuf};

pub trait ConfigFiles: Send + Sync {
    fn path(&self) -> Result<PathBuf, ConfigError>;
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, ConfigError>;
    fn write_private_file(&self, path: &Path, body: &[u8]) -> Result<(), ConfigError>;
    fn ensure_private_file(&self, path: &Path, body: &[u8]) -> Result<(), ConfigError>;
}

pub struct SystemConfigFiles {
    path: PathBuf,
}
impl SystemConfigFiles {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}
impl ConfigFiles for SystemConfigFiles {
    fn path(&self) -> Result<PathBuf, ConfigError> {
        Ok(self.path.clone())
    }
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, ConfigError> {
        std::fs::read(path).map_err(ConfigError::Io)
    }
    fn write_private_file(&self, path: &Path, body: &[u8]) -> Result<(), ConfigError> {
        private_file::write(path, body)
    }
    fn ensure_private_file(&self, path: &Path, body: &[u8]) -> Result<(), ConfigError> {
        initialization::ensure_with_body(path, body)
    }
}
