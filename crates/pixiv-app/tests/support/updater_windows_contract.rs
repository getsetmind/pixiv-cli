use pixiv_app::update::{
    ExternalError,
    installer::{WindowsFileReplacer, WindowsReplacementApi, must_preserve_replacement_source},
};
use serde_json::{Value, json};
use std::{
    error::Error,
    fmt, fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
#[derive(Debug)]
struct MockError {
    message: String,
    cause: Option<io::Error>,
}
impl fmt::Display for MockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl Error for MockError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.cause.as_ref().map(|e| e as &(dyn Error + 'static))
    }
}
fn fail(message: impl Into<String>) -> ExternalError {
    let message = message.into();
    let cause = if message == "invalid argument" {
        Some(io::Error::from_raw_os_error(22))
    } else if message == "file already exists" {
        Some(io::Error::from(io::ErrorKind::AlreadyExists))
    } else if message.contains("no such file or directory") {
        Some(io::Error::from(io::ErrorKind::NotFound))
    } else {
        None
    };
    Box::new(MockError { message, cause })
}
fn code_error(code: i32, wrapped: bool) -> ExternalError {
    let message = if code == 5 {
        "input/output error".into()
    } else {
        format!("errno {code}")
    };
    Box::new(MockError {
        message: if wrapped {
            format!("mock wrapped: {message}")
        } else {
            message
        },
        cause: Some(io::Error::from_raw_os_error(code)),
    })
}
fn write(path: &Path, body: &[u8]) {
    use std::os::unix::fs::PermissionsExt;
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}
struct Api {
    root: PathBuf,
    input: Value,
    trace: Mutex<Vec<Value>>,
}
impl Api {
    fn norm(&self, value: impl AsRef<str>) -> String {
        value
            .as_ref()
            .replace(self.root.to_str().unwrap(), "${ROOT}")
    }
    fn path(value: &[u16]) -> PathBuf {
        assert_eq!(value.last(), Some(&0));
        String::from_utf16(&value[..value.len() - 1])
            .unwrap()
            .into()
    }
    fn flag(&self, name: &str) -> bool {
        self.input[name].as_bool().unwrap_or(false)
    }
}
impl WindowsReplacementApi for Api {
    fn utf16(&self, path: &Path) -> Result<Vec<u16>, ExternalError> {
        let text = path.to_string_lossy();
        self.trace
            .lock()
            .unwrap()
            .push(json!({"op":"utf16","value":self.norm(&text)}));
        if text.contains('\0') {
            return Err(fail("invalid argument"));
        }
        let mut code = text.encode_utf16().collect::<Vec<_>>();
        code.push(0);
        Ok(code)
    }
    fn find_replace(&self) -> Result<(), ExternalError> {
        self.trace
            .lock()
            .unwrap()
            .push(json!({"op":"find","proc":"ReplaceFileW"}));
        if self.flag("find_error") {
            Err(fail("find denied"))
        } else {
            Ok(())
        }
    }
    fn move_file(&self, source: &[u16], target: &[u16], flags: u32) -> Result<(), ExternalError> {
        assert_eq!(flags, 0);
        let source = Self::path(source);
        let target = Self::path(target);
        self.trace.lock().unwrap().push(json!({"op":"move","source":self.norm(source.to_string_lossy()),"target":self.norm(target.to_string_lossy()),"flags":flags}));
        if self.flag("move_error") || self.flag("restore_error") {
            return Err(fail("move denied"));
        }
        if target.exists() {
            return Err(fail("file already exists"));
        }
        fs::rename(&source, &target).map_err(|e| fail(e.to_string()))
    }
    fn replace_file(
        &self,
        target: &[u16],
        source: &[u16],
        backup: &[u16],
        flags: u32,
        reserved_a: usize,
        reserved_b: usize,
    ) -> Result<(), ExternalError> {
        assert_eq!((flags, reserved_a, reserved_b), (0, 0, 0));
        let source = Self::path(source);
        let target = Self::path(target);
        let backup = Self::path(backup);
        self.trace.lock().unwrap().push(json!({"op":"replace","proc":"ReplaceFileW","target":self.norm(target.to_string_lossy()),"source":self.norm(source.to_string_lossy()),"backup":self.norm(backup.to_string_lossy()),"flags":flags,"reserved_a":reserved_a,"reserved_b":reserved_b}));
        if self.flag("missing_source") {
            return Err(fail(format!(
                "lstat {}: no such file or directory",
                self.norm(source.to_string_lossy())
            )));
        }
        let code = self.input["replace_code"].as_i64().unwrap_or(0) as i32;
        if code != 0 {
            if code == 1177 {
                fs::rename(&target, &backup).unwrap();
                if self.flag("recovery_conflict") {
                    write(&target, b"racing-target");
                }
            }
            return Err(code_error(code, self.flag("wrapped_code")));
        }
        fs::rename(&target, &backup).unwrap();
        fs::rename(&source, &target).unwrap();
        if self.flag("backup_directory") {
            fs::remove_file(&backup).unwrap();
            fs::create_dir(&backup).unwrap();
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&backup, fs::Permissions::from_mode(0o700)).unwrap();
            }
            write(&backup.join("old"), b"old");
        }
        Ok(())
    }
}
fn files(root: &Path) -> Vec<Value> {
    fn walk(root: &Path, path: &Path, out: &mut Vec<Value>) {
        use std::os::unix::fs::PermissionsExt;
        let mut entries = fs::read_dir(path)
            .unwrap()
            .map(Result::unwrap)
            .collect::<Vec<_>>();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let path = entry.path();
            let metadata = entry.metadata().unwrap();
            let mut item = json!({"path":path.strip_prefix(root).unwrap().to_string_lossy(),"kind":if metadata.is_dir(){"directory"}else{"file"},"mode":format!("{:04o}",metadata.permissions().mode()&0o777)});
            if !metadata.is_dir() {
                item["bytes"] =
                    Value::String(String::from_utf8_lossy(&fs::read(&path).unwrap()).into_owned());
            }
            out.push(item);
            if metadata.is_dir() {
                walk(root, &path, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out
}
fn error_has(error: &(dyn Error + 'static), predicate: fn(&io::Error) -> bool) -> bool {
    if error.downcast_ref::<io::Error>().is_some_and(predicate) {
        return true;
    }
    if let Some(joined) = error.downcast_ref::<pixiv_app::update::JoinedError>()
        && joined.causes().any(|e| error_has(e, predicate))
    {
        return true;
    }
    error.source().is_some_and(|e| error_has(e, predicate))
}
pub fn replay() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../pixiv-cli/tests/fixtures/updater-windows-replacement.json"
    ))
    .unwrap();
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 26);
    for row in fixture["cases"].as_array().unwrap() {
        let input = &row["input"];
        let name = input["name"].as_str().unwrap();
        let root = tempfile::tempdir().unwrap();
        let expand = |key: &str| {
            PathBuf::from(
                row["paths"][key]
                    .as_str()
                    .unwrap()
                    .replace("${ROOT}", root.path().to_str().unwrap()),
            )
        };
        let source = expand("source");
        let target = expand("target");
        let backup = expand("backup");
        if !input["missing_source"].as_bool().unwrap_or(false) {
            let source_clean = PathBuf::from(source.to_string_lossy().replace("\0x", ""));
            write(&source_clean, b"candidate");
        }
        if input["blocked_parent"].as_bool().unwrap_or(false) {
            write(&root.path().join("blocked"), b"block");
            write(&root.path().join("pixiv.exe"), b"old");
        } else if !input["missing_target"].as_bool().unwrap_or(false) {
            let target_clean = PathBuf::from(target.to_string_lossy().replace("\0x", ""));
            write(&target_clean, b"old");
        }
        let api = Arc::new(Api {
            root: root.path().into(),
            input: input.clone(),
            trace: Mutex::default(),
        });
        let replacer = WindowsFileReplacer::new(api.clone());
        let mut outcome = None;
        let result = match input["method"].as_str().unwrap() {
            "private" => {
                let (state, error) = replacer.replace_private(&source, &target);
                outcome = Some(
                    json!({"committed":state.committed,"preserve_source":state.preserve_source}),
                );
                match error {
                    Some(error) => Err(error),
                    None => Ok(()),
                }
            }
            "disposable" => replacer.replace_disposable(&source, &target).map(|_| ()),
            _ => replacer.replace_retained(&source, &target, &backup),
        };
        let messages = result
            .as_ref()
            .err()
            .map(|e| vec![api.norm(e.to_string())])
            .unwrap_or_default();
        assert_eq!(
            messages,
            row["error_messages"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| e.as_str().unwrap().to_string())
                .collect::<Vec<_>>(),
            "{name} error"
        );
        assert_eq!(
            *api.trace.lock().unwrap(),
            row["trace"].as_array().unwrap().clone(),
            "{name} trace"
        );
        assert_eq!(
            result
                .as_ref()
                .err()
                .is_some_and(|e| error_has(e.as_ref(), |e| e.raw_os_error() == Some(1177))),
            row["is_1177"].as_bool().unwrap(),
            "{name} 1177 identity"
        );
        assert_eq!(result.as_ref().err().is_some_and(|e|error_has(e.as_ref(),|e|e.kind()==io::ErrorKind::InvalidInput)),row["is_invalid_argument"].as_bool().unwrap(),"{name} invalid argument identity");
        assert_eq!(
            result
                .as_ref()
                .err()
                .is_some_and(|e| error_has(e.as_ref(), |e| e.kind() == io::ErrorKind::NotFound)),
            row["is_not_exist"].as_bool().unwrap(),
            "{name} not-found identity"
        );
        assert_eq!(
            result
                .as_ref()
                .err()
                .is_some_and(|e| error_has(e.as_ref(), |e| matches!(
                    e.kind(),
                    io::ErrorKind::AlreadyExists | io::ErrorKind::DirectoryNotEmpty
                ))),
            row["is_already_exist"].as_bool().unwrap(),
            "{name} already-exists identity"
        );
        assert_eq!(
            files(root.path()),
            row["files"].as_array().unwrap().clone(),
            "{name} files"
        );
        assert_eq!(
            outcome.unwrap_or(Value::Null),
            row["result"],
            "{name} outcome"
        );
        assert_eq!(
            result
                .as_ref()
                .err()
                .is_some_and(|e| must_preserve_replacement_source(e.as_ref())),
            row["preserve_replacement_source"].as_bool().unwrap(),
            "{name} preservation"
        );
    }
}
