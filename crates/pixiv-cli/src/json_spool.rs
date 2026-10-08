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
}

impl JsonSpool {
    pub(crate) fn new() -> Result<Self, CommandError> {
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
        };
        spool
            .file
            .as_mut()
            .expect("spool file is open")
            .write_all(b"{\n  \"illusts\": [")?;
        Ok(spool)
    }

    pub(crate) fn append(&mut self, items: &[Artwork]) -> Result<(), CommandError> {
        let file = self.file.as_mut().expect("spool file is open");
        for item in items {
            if !self.first {
                file.write_all(b",")?;
            }
            self.first = false;
            let encoded =
                serde_json::to_string_pretty(&ArtworkDto::from(item)).map_err(io::Error::other)?;
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

    pub(crate) fn commit<W: Write>(&mut self, out: &mut W) -> Result<(), CommandError> {
        let file = self.file.as_mut().expect("spool file is open");
        file.write_all(b"\n  ]\n}\n")?;
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
