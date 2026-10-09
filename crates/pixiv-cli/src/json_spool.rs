use crate::CommandError;
use pixiv_sdk::{dto::ArtworkDto, models::Artwork};
use std::{
    fs::{File, OpenOptions},
    io::{self, Seek, SeekFrom, Write},
    path::PathBuf,
};

pub(crate) struct JsonSpool {
    file: Option<File>,
    path: PathBuf,
    first: bool,
    fields: Vec<String>,
}

impl JsonSpool {
    pub(crate) fn new() -> Result<Self, CommandError> {
        Self::with_key("illusts")
    }
    pub(crate) fn with_key(key: &str) -> Result<Self, CommandError> {
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random).map_err(|error| io::Error::other(error.to_string()))?;
        let name = random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let path = std::env::temp_dir().join(format!("pixiv-cli-json-{name}.tmp"));
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path)?;
        let mut spool = Self {
            file: Some(file),
            path,
            first: true,
            fields: Vec::new(),
        };
        spool
            .file
            .as_mut()
            .expect("spool file is open")
            .write_all(format!("{{\n  \"{key}\": [").as_bytes())?;
        Ok(spool)
    }

    pub(crate) fn append(&mut self, items: &[Artwork]) -> Result<(), CommandError> {
        self.append_dtos(items.iter().map(ArtworkDto::from))
    }
    pub(crate) fn append_novels(
        &mut self,
        items: &[pixiv_sdk::models::Novel],
    ) -> Result<(), CommandError> {
        self.append_dtos(items.iter().map(pixiv_sdk::dto::NovelDto::from))
    }
    pub(crate) fn append_users(
        &mut self,
        items: &[pixiv_sdk::models::UserPreview],
    ) -> Result<(), CommandError> {
        self.append_dtos(items.iter().map(pixiv_sdk::dto::UserPreviewDto::from))
    }
    fn append_dtos<T: serde::Serialize>(
        &mut self,
        items: impl IntoIterator<Item = T>,
    ) -> Result<(), CommandError> {
        let file = self.file.as_mut().expect("spool file is open");
        for item in items {
            if !self.first {
                file.write_all(b",")?;
            }
            self.first = false;
            let encoded = serde_json::to_string_pretty(&item).map_err(io::Error::other)?;
            let encoded = crate::go_json_escape(encoded);
            file.write_all(b"\n    ")?;
            for (index, line) in encoded.split('\n').enumerate() {
                if index > 0 {
                    file.write_all(b"\n    ")?;
                }
                file.write_all(line.as_bytes())?;
            }
        }
        Ok(())
    }

    pub(crate) fn add_field<T: serde::Serialize>(
        &mut self,
        key: &str,
        value: &T,
    ) -> Result<(), CommandError> {
        self.fields.push(format!(
            ",\n  {}: {}",
            serde_json::to_string(key).map_err(io::Error::other)?,
            crate::go_json_escape(serde_json::to_string_pretty(value).map_err(io::Error::other)?)
                .replace('\n', "\n  ")
        ));
        Ok(())
    }
    pub(crate) fn commit<W: Write>(&mut self, out: &mut W) -> Result<(), CommandError> {
        let file = self.file.as_mut().expect("spool file is open");
        if self.first && !self.fields.is_empty() {
            file.write_all(b"]")?;
        } else {
            file.write_all(b"\n  ]")?;
        }
        for field in &self.fields {
            file.write_all(field.as_bytes())?;
        }
        file.write_all(b"\n}\n")?;
        file.seek(SeekFrom::Start(0))?;
        io::copy(file, out)?;
        Ok(())
    }
}

impl Drop for JsonSpool {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = std::fs::remove_file(&self.path);
    }
}
