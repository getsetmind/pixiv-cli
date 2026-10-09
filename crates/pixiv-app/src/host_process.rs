use std::{
    collections::VecDeque,
    error::Error,
    ffi::{OsStr, OsString},
    fmt, io,
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    thread,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostPlatform {
    Linux,
    Darwin,
    Windows,
    FreeBsd,
    NetBsd,
    OpenBsd,
    Unsupported(&'static str),
}

impl HostPlatform {
    pub fn current() -> Self {
        match std::env::consts::OS {
            "linux" => Self::Linux,
            "macos" => Self::Darwin,
            "windows" => Self::Windows,
            "freebsd" => Self::FreeBsd,
            "netbsd" => Self::NetBsd,
            "openbsd" => Self::OpenBsd,
            other => Self::Unsupported(other),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessStdio {
    Inherit,
    Capture,
    CaptureStdout,
    Combined,
    Discard,
}

#[derive(Debug)]
pub struct ProcessOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Debug)]
pub enum HostProcessError {
    Lookup {
        program: OsString,
        source: io::Error,
    },
    RelativePath {
        program: OsString,
        path: PathBuf,
    },
    Spawn {
        program: OsString,
        source: io::Error,
    },
    Exit {
        program: OsString,
        output: ProcessOutput,
    },
    Captured {
        source: Box<HostProcessError>,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
    Native(io::Error),
    Context(crate::lifecycle::ContextError),
}

impl HostProcessError {
    pub fn captured_stdout(&self) -> &[u8] {
        match self {
            Self::Exit { output, .. } => &output.stdout,
            Self::Captured { stdout, .. } => stdout,
            _ => &[],
        }
    }

    fn with_captured_output(self, stdio: ProcessStdio, stdout: Vec<u8>, stderr: Vec<u8>) -> Self {
        if stdio == ProcessStdio::Combined {
            Self::Captured {
                source: Box::new(self),
                stdout,
                stderr,
            }
        } else {
            self
        }
    }

    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::Lookup { source, .. } if source.kind() == io::ErrorKind::NotFound)
    }
}

impl fmt::Display for HostProcessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lookup { program, source } => write!(
                f,
                "exec: {}: {source}",
                crate::auth_bundle::go_quote(&program.to_string_lossy())
            ),
            Self::RelativePath { program, .. } => write!(
                f,
                "exec: {}: cannot run executable found relative to current directory",
                crate::auth_bundle::go_quote(&program.to_string_lossy())
            ),
            Self::Spawn { program, source } => {
                write!(f, "fork/exec {}: ", program.to_string_lossy())?;
                #[cfg(unix)]
                if source.raw_os_error() == Some(libc::ENOENT) {
                    return f.write_str("no such file or directory");
                }
                source.fmt(f)
            }
            Self::Captured { source, .. } => source.fmt(f),
            Self::Native(source) => source.fmt(f),
            Self::Context(source) => source.fmt(f),
            Self::Exit { output, .. } => match output.status.code() {
                Some(code) => write!(f, "exit status {code}"),
                None => {
                    #[cfg(unix)]
                    {
                        use std::os::unix::process::ExitStatusExt;
                        if output.status.signal() == Some(libc::SIGKILL) {
                            return f.write_str("signal: killed");
                        }
                    }
                    output.status.fmt(f)
                }
            },
        }
    }
}

impl Error for HostProcessError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Lookup { source, .. } | Self::Spawn { source, .. } | Self::Native(source) => {
                Some(source)
            }
            Self::Context(source) => Some(source),
            Self::Captured { source, .. } => Some(source.as_ref()),
            Self::RelativePath { .. } | Self::Exit { .. } => None,
        }
    }
}

pub trait HostProcess: Send + Sync {
    fn platform(&self) -> HostPlatform;
    fn look_path(&self, program: &OsStr) -> Result<PathBuf, HostProcessError>;
    fn run(
        &self,
        program: &OsStr,
        args: &[OsString],
        stdio: ProcessStdio,
    ) -> Result<ProcessOutput, HostProcessError>;
    fn shell_open_url(&self, url: &str) -> Result<(), HostProcessError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemHostProcess;

fn lookup_error(program: &OsStr, source: io::Error) -> HostProcessError {
    HostProcessError::Lookup {
        program: program.to_owned(),
        source,
    }
}

fn contains_separator(program: &OsStr) -> bool {
    let bytes = program.as_encoded_bytes();
    bytes.contains(&b'/') || (cfg!(windows) && (bytes.contains(&b'\\') || bytes.contains(&b':')))
}

fn executable(path: &Path) -> io::Result<()> {
    let metadata = path.metadata()?;
    if metadata.is_dir() {
        #[cfg(unix)]
        return Err(io::Error::from_raw_os_error(libc::EISDIR));
        #[cfg(not(unix))]
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "not an executable file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let path = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "executable path contains NUL")
        })?;
        if unsafe { libc::faccessat(libc::AT_FDCWD, path.as_ptr(), libc::X_OK, libc::AT_EACCESS) }
            != 0
        {
            let error = io::Error::last_os_error();
            if matches!(error.raw_os_error(), Some(libc::ENOSYS | libc::EPERM)) {
                use std::os::unix::fs::PermissionsExt;
                if metadata.permissions().mode() & 0o111 == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "permission denied",
                    ));
                }
            } else {
                return Err(error);
            }
        }
    }
    Ok(())
}

fn allow_relative_path() -> bool {
    std::env::var("GODEBUG")
        .ok()
        .and_then(|value| {
            value
                .split(',')
                .rev()
                .find_map(|item| item.strip_prefix("execerrdot=").map(str::to_owned))
        })
        .is_some_and(|value| value == "0")
}

#[cfg(not(windows))]
fn checked_candidate(program: &OsStr, path: PathBuf) -> Result<PathBuf, HostProcessError> {
    if !path.is_absolute() && !allow_relative_path() {
        Err(HostProcessError::RelativePath {
            program: program.to_owned(),
            path,
        })
    } else {
        Ok(path)
    }
}

#[cfg(windows)]
fn path_extensions() -> Vec<String> {
    let extensions = std::env::var("PATHEXT")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| ".com;.exe;.bat;.cmd".into());
    let extensions: Vec<_> = extensions
        .split(';')
        .filter(|extension| !extension.is_empty())
        .map(|extension| {
            let extension = extension.to_lowercase();
            if extension.starts_with('.') {
                extension
            } else {
                format!(".{extension}")
            }
        })
        .collect();
    extensions
}

#[cfg(windows)]
fn candidates(path: &Path) -> Vec<PathBuf> {
    let extensions = path_extensions();
    let mut candidates = Vec::new();
    if extensions.is_empty()
        || path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().contains('.'))
    {
        candidates.push(path.to_owned());
    }
    for extension in extensions {
        let mut candidate = path.as_os_str().to_owned();
        candidate.push(extension);
        candidates.push(PathBuf::from(candidate));
    }
    candidates
}

#[cfg(windows)]
fn find_executable(path: &Path) -> Option<PathBuf> {
    candidates(path)
        .into_iter()
        .find(|candidate| executable(candidate).is_ok())
}

#[cfg(windows)]
fn same_file(left: &Path, right: &Path) -> io::Result<bool> {
    use std::{
        fs::OpenOptions,
        os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    };
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        GetFileInformationByHandle,
    };
    fn identity(path: &Path) -> io::Result<(u32, u32, u32)> {
        let file = OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((
            info.dwVolumeSerialNumber,
            info.nFileIndexHigh,
            info.nFileIndexLow,
        ))
    }
    Ok(identity(left)? == identity(right)?)
}

#[cfg(windows)]
fn windows_look_path(program: &OsStr) -> Result<PathBuf, HostProcessError> {
    let path = Path::new(program);
    if contains_separator(program) || path.is_absolute() {
        return find_executable(path).ok_or_else(|| {
            lookup_error(
                program,
                io::Error::new(io::ErrorKind::NotFound, "executable file not found"),
            )
        });
    }
    let mut relative = None;
    if std::env::var_os("NoDefaultCurrentDirectoryInExePath").is_none()
        && let Some(candidate) = find_executable(path)
    {
        if allow_relative_path() {
            return Ok(candidate);
        }
        relative = Some(candidate);
    }
    let search_path = std::env::var_os("PATH").unwrap_or_default();
    if !search_path.is_empty() {
        for directory in std::env::split_paths(&search_path)
            .filter(|directory| !directory.as_os_str().is_empty())
        {
            if let Some(candidate) = find_executable(&directory.join(program)) {
                if let Some(previous) = relative.as_ref()
                    && !same_file(previous, &candidate).unwrap_or(false)
                {
                    return Err(HostProcessError::RelativePath {
                        program: program.to_owned(),
                        path: previous.clone(),
                    });
                }
                if !candidate.is_absolute() && !allow_relative_path() {
                    relative.get_or_insert(candidate);
                    continue;
                }
                return Ok(candidate);
            }
        }
    }
    if let Some(path) = relative {
        return Err(HostProcessError::RelativePath {
            program: program.to_owned(),
            path,
        });
    }
    Err(lookup_error(
        program,
        io::Error::new(
            io::ErrorKind::NotFound,
            "executable file not found in %PATH%",
        ),
    ))
}

#[cfg(windows)]
fn windows_look_extensions(path: &Path) -> Result<PathBuf, HostProcessError> {
    if let Some(extension) = path.extension() {
        let extension = format!(".{}", extension.to_string_lossy());
        if path_extensions()
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(&extension))
        {
            return Ok(path.to_owned());
        }
    }
    windows_look_path(path.as_os_str())
}

pub(crate) fn prepare_command(
    host: &impl HostProcess,
    program: &OsStr,
    args: &[OsString],
    stdio: ProcessStdio,
) -> Result<(PathBuf, Command, Option<io::PipeReader>), HostProcessError> {
    let path = Path::new(program);
    let executable = if contains_separator(program) || path.is_absolute() {
        #[cfg(windows)]
        {
            windows_look_extensions(path)?
        }
        #[cfg(not(windows))]
        {
            path.to_owned()
        }
    } else {
        host.look_path(program)?
    };
    let mut command = Command::new(&executable);
    command.args(args).stdin(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.arg0(program);
    }
    let mut combined = None;
    match stdio {
        ProcessStdio::Inherit => {
            command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
        }
        ProcessStdio::Discard => {
            command.stdout(Stdio::null()).stderr(Stdio::null());
        }
        ProcessStdio::Combined => {
            let (reader, writer) = io::pipe().map_err(HostProcessError::Native)?;
            let stderr = writer.try_clone().map_err(HostProcessError::Native)?;
            command
                .stdout(Stdio::from(writer))
                .stderr(Stdio::from(stderr));
            combined = Some(reader);
        }
        ProcessStdio::Capture | ProcessStdio::CaptureStdout => {
            command.stdout(Stdio::piped()).stderr(Stdio::piped());
        }
    }
    Ok((executable, command, combined))
}

pub(crate) fn finish_output(
    program: &OsStr,
    stdio: ProcessStdio,
    mut output: ProcessOutput,
) -> Result<ProcessOutput, HostProcessError> {
    if stdio == ProcessStdio::CaptureStdout && output.status.success() {
        output.stderr.clear();
    }
    if output.status.success() {
        Ok(output)
    } else {
        Err(HostProcessError::Exit {
            program: program.to_owned(),
            output,
        })
    }
}

fn read_pipe(pipe: Option<impl Read>, bounded: bool) -> (Vec<u8>, Option<io::Error>) {
    let Some(mut pipe) = pipe else {
        return (Vec::new(), None);
    };
    let mut prefix = Vec::new();
    let mut suffix = VecDeque::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 8192];
    let error = loop {
        match pipe.read(&mut buffer) {
            Ok(0) => break None,
            Ok(length) => {
                total += length as u64;
                let bytes = &buffer[..length];
                if !bounded {
                    prefix.extend_from_slice(bytes);
                    continue;
                }
                let first = (32768 - prefix.len()).min(bytes.len());
                prefix.extend_from_slice(&bytes[..first]);
                for &byte in &bytes[first..] {
                    if suffix.len() == 32768 {
                        suffix.pop_front();
                    }
                    suffix.push_back(byte);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => break Some(error),
        }
    };
    if bounded {
        let skipped = total.saturating_sub((prefix.len() + suffix.len()) as u64);
        if skipped != 0 {
            prefix.extend_from_slice(format!("\n... omitting {skipped} bytes ...\n").as_bytes());
        }
        prefix.extend(suffix);
    }
    (prefix, error)
}

pub(crate) fn collect_child_output(
    program: &OsStr,
    stdio: ProcessStdio,
    mut child: Child,
    combined: Option<io::PipeReader>,
    wait: impl FnOnce(&mut Child) -> io::Result<(ExitStatus, Option<HostProcessError>)>,
) -> Result<ProcessOutput, HostProcessError> {
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    thread::scope(|scope| {
        let stdout = scope.spawn(move || match combined {
            Some(reader) => read_pipe(Some(reader), false),
            None => read_pipe(stdout, false),
        });
        let stderr = scope.spawn(move || read_pipe(stderr, stdio == ProcessStdio::CaptureStdout));
        let waited = wait(&mut child);
        if waited.is_err() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let (stdout, stdout_error) = stdout
            .join()
            .unwrap_or_else(|_| (Vec::new(), Some(io::Error::other("stdout reader panicked"))));
        let (stderr, stderr_error) = stderr
            .join()
            .unwrap_or_else(|_| (Vec::new(), Some(io::Error::other("stderr reader panicked"))));
        let (status, cancellation) = match waited {
            Ok(waited) => waited,
            Err(error) => {
                return Err(
                    HostProcessError::Native(error).with_captured_output(stdio, stdout, stderr)
                );
            }
        };
        let output = ProcessOutput {
            status,
            stdout,
            stderr,
        };
        if !status.success() {
            return finish_output(program, stdio, output);
        }
        if let Some(error) = cancellation {
            return Err(error.with_captured_output(stdio, output.stdout, output.stderr));
        }
        if let Some(error) = stdout_error.or(stderr_error) {
            return Err(HostProcessError::Native(error).with_captured_output(
                stdio,
                output.stdout,
                output.stderr,
            ));
        }
        finish_output(program, stdio, output)
    })
}

impl HostProcess for SystemHostProcess {
    fn platform(&self) -> HostPlatform {
        HostPlatform::current()
    }

    fn look_path(&self, program: &OsStr) -> Result<PathBuf, HostProcessError> {
        #[cfg(windows)]
        {
            windows_look_path(program)
        }
        #[cfg(not(windows))]
        {
            let path = Path::new(program);
            if contains_separator(program) || path.is_absolute() {
                executable(path).map_err(|source| lookup_error(program, source))?;
                return Ok(path.to_owned());
            }
            let search_path = std::env::var_os("PATH").unwrap_or_default();
            if !search_path.is_empty() {
                for directory in std::env::split_paths(&search_path) {
                    let candidate = directory.join(program);
                    if executable(&candidate).is_ok() {
                        return checked_candidate(program, candidate);
                    }
                }
            }
            Err(lookup_error(
                program,
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "executable file not found in $PATH",
                ),
            ))
        }
    }

    fn run(
        &self,
        program: &OsStr,
        args: &[OsString],
        stdio: ProcessStdio,
    ) -> Result<ProcessOutput, HostProcessError> {
        let (executable, mut command, combined) = prepare_command(self, program, args, stdio)?;
        if matches!(stdio, ProcessStdio::CaptureStdout | ProcessStdio::Combined) {
            let child = command.spawn().map_err(|source| HostProcessError::Spawn {
                program: executable.into_os_string(),
                source,
            })?;
            drop(command);
            return collect_child_output(program, stdio, child, combined, |child| {
                child.wait().map(|status| (status, None))
            });
        }
        let output = command.output().map_err(|source| HostProcessError::Spawn {
            program: executable.into_os_string(),
            source,
        })?;
        finish_output(
            program,
            stdio,
            ProcessOutput {
                status: output.status,
                stdout: output.stdout,
                stderr: output.stderr,
            },
        )
    }

    fn shell_open_url(&self, url: &str) -> Result<(), HostProcessError> {
        #[cfg(windows)]
        {
            use windows_sys::Win32::{
                Foundation::GetLastError,
                UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
            };
            if url.contains('\0') {
                panic!("windows: string with NUL passed to StringToUTF16");
            }
            let file: Vec<u16> = url.encode_utf16().chain(Some(0)).collect();
            let result = unsafe {
                ShellExecuteW(
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    file.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    SW_SHOWNORMAL,
                )
            };
            if result as usize <= 32 {
                let code = unsafe { GetLastError() };
                return Err(HostProcessError::Native(if code == 0 {
                    io::Error::new(io::ErrorKind::InvalidInput, "invalid argument")
                } else {
                    io::Error::from_raw_os_error(code as i32)
                }));
            }
            Ok(())
        }
        #[cfg(not(windows))]
        {
            let _ = url;
            Err(HostProcessError::Native(io::Error::new(
                io::ErrorKind::Unsupported,
                "Windows ShellExecuteW is unavailable on this host",
            )))
        }
    }
}
