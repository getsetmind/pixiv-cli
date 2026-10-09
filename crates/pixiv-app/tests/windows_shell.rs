use pixiv_app::{
    lifecycle::{Context, ContextError},
    windows_shell::{SystemWindowsShell, WindowsShell, WindowsShellError},
};
use std::{
    error::Error,
    fs, io,
    process::Command,
    time::{Duration, Instant},
};

#[test]
fn context_errors_precede_class_and_url_nul_conversion() {
    let canceled = Context::new();
    canceled.cancel();
    let deadline = Context::with_deadline(Instant::now() - Duration::from_secs(1));
    for (context, wanted) in [
        (canceled, ContextError::Canceled),
        (deadline, ContextError::DeadlineExceeded),
    ] {
        let error = SystemWindowsShell
            .open_class(&context, "class\0", "pixiv://unlisted\0")
            .unwrap_err();
        assert!(matches!(error, WindowsShellError::Context(reason) if reason == wanted));
        assert_eq!(error.to_string(), wanted.to_string());
        assert_eq!(error.source().unwrap().to_string(), wanted.to_string());
    }
}

#[test]
fn nul_in_class_or_url_returns_invalid_argument_without_native_execution() {
    for (class, url) in [
        ("class\0", "url\0"),
        ("class\0", "pixiv://unlisted"),
        ("Previous.Handler", "url\0"),
    ] {
        let error = SystemWindowsShell
            .open_class(&Context::new(), class, url)
            .unwrap_err();
        assert_eq!(error.to_string(), "invalid argument");
        assert_eq!(error.source().unwrap().to_string(), "invalid argument");
        assert!(
            matches!(error, WindowsShellError::Native(source) if source.kind() == io::ErrorKind::InvalidInput)
        );
    }
}

#[cfg(not(windows))]
#[test]
fn native_class_open_on_other_hosts_is_unsupported_and_never_noop_success() {
    for (class, url) in [
        ("Previous.Handler.🐈", "pixiv://unlisted?text=絵🐈"),
        ("", ""),
    ] {
        let error = SystemWindowsShell
            .open_class(&Context::new(), class, url)
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Windows ShellExecuteExW is unavailable on this host"
        );
        assert!(
            matches!(error, WindowsShellError::Native(source) if source.kind() == io::ErrorKind::Unsupported)
        );
    }
}

#[test]
fn source_driven_native_branch_preserves_frozen_go_abi_and_error_priority() {
    let source = include_str!("../src/windows_shell.rs");
    assert_eq!(source.matches("#[cfg(windows)]").count(), 1);
    assert_eq!(source.matches("#[cfg(not(windows))]").count(), 1);
    assert_eq!(source.matches("use windows_sys::Win32::").count(), 1);
    let source = source
        .replace("#[cfg(windows)]", "")
        .replace("#[cfg(not(windows))]", "#[cfg(any())]")
        .replace(
            "use windows_sys::Win32::",
            "use crate::windows_sys::Win32::",
        );
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("windows_shell_contract.rs");
    let executable = directory.path().join(if cfg!(windows) {
        "windows_shell_contract.exe"
    } else {
        "windows_shell_contract"
    });
    let payload = include_str!("support/windows_shell_native.rs");
    fs::write(
        &path,
        format!("{payload}\nmod windows_shell {{\n{source}\n}}\n"),
    )
    .unwrap();
    let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let output = Command::new(compiler)
        .args(["--test", "--edition=2024", "-Dwarnings"])
        .arg(&path)
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "source-driven mock compilation failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = Command::new(&executable)
        .arg("--nocapture")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "source-driven native branch contracts failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
