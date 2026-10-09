#![cfg(unix)]

use pixiv_app::{
    host_context::ContextHostProcess,
    host_process::{HostProcess, HostProcessError, ProcessStdio, SystemHostProcess},
    lifecycle::{Context, ContextError},
};
use std::{
    ffi::OsString,
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};

// Concurrent forks can temporarily inherit another fixture's writable executable descriptor.
static EXECUTABLE_FIXTURES: Mutex<()> = Mutex::new(());

fn script(directory: &Path, body: &str) -> std::path::PathBuf {
    let path = directory.join("trusted-child");
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}

#[test]
fn lookup_precedes_cancellation_but_explicit_path_spawn_does_not() {
    let _fixtures = EXECUTABLE_FIXTURES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let directory = tempfile::tempdir().unwrap();
    let executable = script(directory.path(), "printf started");
    for (context, wanted) in [
        {
            let context = Context::new();
            context.cancel();
            (context, ContextError::Canceled)
        },
        (
            Context::with_deadline(Instant::now() - Duration::from_secs(1)),
            ContextError::DeadlineExceeded,
        ),
    ] {
        let error = SystemHostProcess
            .run_context(
                &context,
                "missing-host-context-child-for-pixiv".as_ref(),
                &[],
                ProcessStdio::Capture,
            )
            .unwrap_err();
        assert!(matches!(error, HostProcessError::Lookup { .. }));
        let absent = directory.path().join("absent");
        for program in [executable.as_os_str(), absent.as_os_str()] {
            let error = SystemHostProcess
                .run_context(&context, program, &[], ProcessStdio::Capture)
                .unwrap_err();
            assert!(matches!(error, HostProcessError::Context(reason) if reason == wanted));
            assert_eq!(error.to_string(), wanted.to_string());
        }
    }
}

#[test]
fn uncanceled_context_preserves_argument_boundaries_stdin_eof_and_captured_bytes() {
    let _fixtures = EXECUTABLE_FIXTURES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let directory = tempfile::tempdir().unwrap();
    let executable = script(
        directory.path(),
        "if read value; then exit 41; fi\nprintf '%s' \"$1\"\nprintf diagnostic >&2",
    );
    let argument = OsString::from("a b;$(never-run)\nquoted");
    let output = SystemHostProcess
        .run_context(
            &Context::new(),
            executable.as_os_str(),
            std::slice::from_ref(&argument),
            ProcessStdio::Capture,
        )
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, argument.as_encoded_bytes());
    assert_eq!(output.stderr, b"diagnostic");
}

#[test]
fn already_canceled_context_does_not_change_browser_runner() {
    let _fixtures = EXECUTABLE_FIXTURES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let directory = tempfile::tempdir().unwrap();
    let executable = script(directory.path(), "printf started");
    let context = Context::new();
    context.cancel();
    assert_eq!(
        SystemHostProcess
            .run(executable.as_os_str(), &[], ProcessStdio::Capture)
            .unwrap()
            .stdout,
        b"started"
    );
}

#[test]
fn capture_stdout_and_discard_follow_go_output_and_run_boundaries() {
    let _fixtures = EXECUTABLE_FIXTURES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let directory = tempfile::tempdir().unwrap();
    let executable = script(directory.path(), "printf stdout; printf stderr >&2");
    for context in [false, true] {
        for mode in [ProcessStdio::CaptureStdout, ProcessStdio::Discard] {
            let output = if context {
                SystemHostProcess.run_context(&Context::new(), executable.as_os_str(), &[], mode)
            } else {
                SystemHostProcess.run(executable.as_os_str(), &[], mode)
            }
            .unwrap();
            assert_eq!(
                output.stdout,
                if mode == ProcessStdio::CaptureStdout {
                    b"stdout".as_slice()
                } else {
                    b""
                }
            );
            assert!(output.stderr.is_empty());
        }
    }
    let executable = script(directory.path(), "printf stdout; printf stderr >&2; exit 7");
    let error = SystemHostProcess
        .run_context(
            &Context::new(),
            executable.as_os_str(),
            &[],
            ProcessStdio::CaptureStdout,
        )
        .unwrap_err();
    assert_eq!(error.to_string(), "exit status 7");
    let HostProcessError::Exit { output, .. } = error else {
        panic!("exit error required")
    };
    assert_eq!(output.stdout, b"stdout");
    assert_eq!(output.stderr, b"stderr");
}

#[test]
fn simultaneous_stdout_stderr_over_pipe_capacity_is_drained() {
    let _fixtures = EXECUTABLE_FIXTURES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let directory = tempfile::tempdir().unwrap();
    let bytes = "x".repeat(131072);
    let executable = script(
        directory.path(),
        &format!("printf '{bytes}'; printf '{bytes}' >&2"),
    );
    let output = SystemHostProcess
        .run_context(
            &Context::new(),
            executable.as_os_str(),
            &[],
            ProcessStdio::Capture,
        )
        .unwrap();
    assert_eq!(output.stdout, bytes.as_bytes());
    assert_eq!(output.stderr, bytes.as_bytes());
}

#[test]
fn post_start_cancel_and_deadline_return_killed_exit_capture_and_reap() {
    let _fixtures = EXECUTABLE_FIXTURES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for deadline in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("started");
        let executable = script(
            directory.path(),
            "printf stdout; printf stderr >&2; printf '%s' \"$$\" > \"$1\"; while :; do :; done",
        );
        let context = if deadline {
            Context::with_deadline(Instant::now() + Duration::from_millis(500))
        } else {
            Context::new()
        };
        let cloned = context.clone();
        let args = [marker.as_os_str().to_owned()];
        thread::scope(|scope| {
            let handle = scope.spawn(|| {
                SystemHostProcess.run_context(
                    &context,
                    executable.as_os_str(),
                    &args,
                    ProcessStdio::Capture,
                )
            });
            let limit = Instant::now() + Duration::from_secs(3);
            while fs::read_to_string(&marker).unwrap_or_default().is_empty() {
                if Instant::now() >= limit {
                    cloned.cancel();
                    let _ = handle.join();
                    panic!("child did not start")
                }
                thread::sleep(Duration::from_millis(1));
            }
            if !deadline {
                cloned.cancel();
            }
            let error = handle.join().unwrap().unwrap_err();
            assert_eq!(error.to_string(), "signal: killed");
            let HostProcessError::Exit { output, .. } = error else {
                panic!("exit error required")
            };
            assert_eq!(output.stdout, b"stdout");
            assert_eq!(output.stderr, b"stderr");
            let pid: i32 = fs::read_to_string(&marker).unwrap().parse().unwrap();
            assert_eq!(
                unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) },
                -1
            );
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ECHILD)
            );
        });
    }
}

#[test]
fn output_stderr_retains_go_prefix_suffix_and_omission_count() {
    let _fixtures = EXECUTABLE_FIXTURES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let directory = tempfile::tempdir().unwrap();
    let diagnostic = format!(
        "{}{}{}",
        "a".repeat(32768),
        "b".repeat(4464),
        "c".repeat(32768)
    );
    let executable = script(
        directory.path(),
        &format!("printf stdout; printf '{diagnostic}' >&2; exit 7"),
    );
    let wanted = format!(
        "{}\n... omitting 4464 bytes ...\n{}",
        "a".repeat(32768),
        "c".repeat(32768)
    );
    for context in [false, true] {
        let error = if context {
            SystemHostProcess.run_context(
                &Context::new(),
                executable.as_os_str(),
                &[],
                ProcessStdio::CaptureStdout,
            )
        } else {
            SystemHostProcess.run(executable.as_os_str(), &[], ProcessStdio::CaptureStdout)
        }
        .unwrap_err();
        assert_eq!(error.to_string(), "exit status 7");
        let HostProcessError::Exit { output, .. } = error else {
            panic!("exit error required")
        };
        assert_eq!(output.stdout, b"stdout");
        assert_eq!(output.stderr, wanted.as_bytes());
    }
}

#[test]
#[ignore]
fn descendant_pipe_fixture() {
    let executable = std::env::current_exe().unwrap();
    let directory = executable.parent().unwrap();
    if executable.file_name().unwrap() == "context-pipe-parent" {
        let mut child = std::process::Command::new(directory.join("context-pipe-child"))
            .args([
                "--exact",
                "descendant_pipe_fixture",
                "--ignored",
                "--nocapture",
            ])
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .unwrap();
        let limit = Instant::now() + Duration::from_secs(3);
        while !directory.join("ready").exists() {
            if Instant::now() >= limit {
                let _ = child.kill();
                let _ = child.wait();
                std::process::exit(43);
            }
            thread::sleep(Duration::from_millis(1));
        }
        std::process::exit(0);
    }
    assert_eq!(executable.file_name().unwrap(), "context-pipe-child");
    fs::write(directory.join("ready"), b"ready").unwrap();
    thread::sleep(Duration::from_millis(500));
    use std::io::Write;
    std::io::stdout().write_all(b"pipe-tail").unwrap();
    std::io::stdout().flush().unwrap();
    std::process::exit(0);
}

#[test]
fn descendant_pipe_is_drained_past_context_deadline_after_parent_exits() {
    let _fixtures = EXECUTABLE_FIXTURES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let directory = tempfile::tempdir().unwrap();
    let parent = directory.path().join("context-pipe-parent");
    fs::copy(std::env::current_exe().unwrap(), &parent).unwrap();
    fs::hard_link(&parent, directory.path().join("context-pipe-child")).unwrap();
    let context = Context::with_deadline(Instant::now() + Duration::from_millis(200));
    let args = [
        "--exact",
        "descendant_pipe_fixture",
        "--ignored",
        "--nocapture",
    ]
    .map(OsString::from);
    let started = Instant::now();
    let output = SystemHostProcess
        .run_context(
            &context,
            parent.as_os_str(),
            &args,
            ProcessStdio::CaptureStdout,
        )
        .unwrap();
    assert!(output.stdout.ends_with(b"pipe-tail"));
    assert_eq!(fs::read(directory.path().join("ready")).unwrap(), b"ready");
    assert!(started.elapsed() >= Duration::from_millis(450));
    assert_eq!(context.error(), Some(ContextError::DeadlineExceeded));
}
