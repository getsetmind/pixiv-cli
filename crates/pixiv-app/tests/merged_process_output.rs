#![cfg(unix)]

use pixiv_app::{
    host_context::ContextHostProcess,
    host_process::{HostProcess, HostProcessError, ProcessStdio, SystemHostProcess},
    lifecycle::Context,
};
use serde::Deserialize;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};

// Concurrent forks can temporarily inherit another fixture's writable executable descriptor.
static EXECUTABLE_FIXTURES: Mutex<()> = Mutex::new(());

#[derive(Deserialize)]
struct Case {
    name: String,
    exit: i32,
    writes: Vec<Write>,
}

#[derive(Deserialize)]
struct Write {
    stderr: bool,
    hex: String,
    repeat: usize,
}

fn script(directory: &Path, body: &str) -> std::path::PathBuf {
    let path = directory.join("trusted-merged-child");
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}

#[test]
fn combined_output_matches_frozen_go_bytes_and_exit_errors() {
    let _fixtures = EXECUTABLE_FIXTURES.lock().unwrap();
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("fixtures/merged_process_output.json")).unwrap();
    for case in cases {
        let directory = tempfile::tempdir().unwrap();
        let mut body = String::from("if read value; then exit 41; fi\n");
        let mut expected = Vec::new();
        for write in case.writes {
            let chunk: Vec<_> = write
                .hex
                .as_bytes()
                .chunks_exact(2)
                .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                .collect();
            let octal: String = chunk.iter().map(|byte| format!("\\{byte:03o}")).collect();
            let redirect = if write.stderr { " >&2" } else { "" };
            body.push_str(&format!(
                "i=0; while [ \"$i\" -lt {} ]; do printf '{octal}'{redirect}; i=$((i+1)); done\n",
                write.repeat
            ));
            expected.extend(chunk.repeat(write.repeat));
        }
        body.push_str(&format!("exit {}", case.exit));
        let executable = script(directory.path(), &body);
        for with_context in [false, true] {
            let result = if with_context {
                SystemHostProcess.run_context(
                    &Context::new(),
                    executable.as_os_str(),
                    &[],
                    ProcessStdio::Combined,
                )
            } else {
                SystemHostProcess.run(executable.as_os_str(), &[], ProcessStdio::Combined)
            };
            let output = if case.exit == 0 {
                result.unwrap()
            } else {
                let error = result.unwrap_err();
                assert_eq!(error.to_string(), format!("exit status {}", case.exit));
                assert_eq!(error.captured_stdout(), expected);
                let HostProcessError::Exit { output, .. } = error else {
                    panic!("exit error required")
                };
                output
            };
            assert_eq!(
                output.status.code(),
                Some(case.exit),
                "{} context={with_context}",
                case.name
            );
            assert_eq!(
                output.stdout, expected,
                "{} context={with_context}",
                case.name
            );
            assert!(
                output.stderr.is_empty(),
                "{} context={with_context}",
                case.name
            );
        }
    }
}

#[test]
fn combined_output_preserves_lookup_and_prestart_cancellation_order() {
    let _fixtures = EXECUTABLE_FIXTURES.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let executable = script(directory.path(), "printf started");
    let context = Context::new();
    context.cancel();
    assert!(matches!(
        SystemHostProcess.run_context(
            &context,
            "missing-merged-host-child-for-pixiv".as_ref(),
            &[],
            ProcessStdio::Combined
        ),
        Err(HostProcessError::Lookup { .. })
    ));
    for path in [executable, directory.path().join("absent")] {
        assert!(matches!(
            SystemHostProcess.run_context(&context, path.as_os_str(), &[], ProcessStdio::Combined),
            Err(HostProcessError::Context(_))
        ));
    }
}

#[test]
fn combined_output_cancel_retains_bytes_killed_exit_and_reaps_child() {
    let _fixtures = EXECUTABLE_FIXTURES.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("started");
    let executable = script(
        directory.path(),
        "printf '\\000\\377'; printf '\\200err' >&2; printf '%s' \"$$\" > \"$1\"; while :; do :; done",
    );
    let context = Context::new();
    thread::scope(|scope| {
        let handle = scope.spawn(|| {
            SystemHostProcess.run_context(
                &context,
                executable.as_os_str(),
                &[marker.as_os_str().to_owned()],
                ProcessStdio::Combined,
            )
        });
        let limit = Instant::now() + Duration::from_secs(3);
        while !marker.exists() {
            if Instant::now() >= limit {
                context.cancel();
                panic!("child did not start")
            }
            thread::sleep(Duration::from_millis(1));
        }
        let pid: i32 = fs::read_to_string(&marker).unwrap().parse().unwrap();
        context.cancel();
        let error = handle.join().unwrap().unwrap_err();
        assert_eq!(error.to_string(), "signal: killed");
        let HostProcessError::Exit { output, .. } = error else {
            panic!("exit error required")
        };
        assert_eq!(output.stdout, [0, 255, 128, b'e', b'r', b'r']);
        assert!(output.stderr.is_empty());
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    });
}

#[test]
fn combined_pipe_drains_descendant_bytes_after_direct_child_exit_and_cancellation() {
    let _fixtures = EXECUTABLE_FIXTURES.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let ready = directory.path().join("ready");
    let release = directory.path().join("release");
    let parent = directory.path().join("parent");
    let executable = script(
        directory.path(),
        "printf '%s' \"$$\" > \"$3\"; (printf ready > \"$1\"; while [ ! -f \"$2\" ]; do :; done; printf tail; printf diagnostic >&2) &\nexit 0",
    );
    let context = Context::new();
    thread::scope(|scope| {
        let handle = scope.spawn(|| {
            SystemHostProcess.run_context(
                &context,
                executable.as_os_str(),
                &[
                    ready.as_os_str().to_owned(),
                    release.as_os_str().to_owned(),
                    parent.as_os_str().to_owned(),
                ],
                ProcessStdio::Combined,
            )
        });
        let limit = Instant::now() + Duration::from_secs(3);
        while !ready.exists() {
            if Instant::now() >= limit {
                fs::write(&release, b"release").unwrap();
                context.cancel();
                panic!("descendant did not start")
            }
            thread::sleep(Duration::from_millis(1));
        }
        let pid: i32 = fs::read_to_string(&parent).unwrap().parse().unwrap();
        while unsafe { libc::kill(pid, 0) } == 0 {
            if Instant::now() >= limit {
                fs::write(&release, b"release").unwrap();
                context.cancel();
                panic!("direct child was not reaped");
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
        context.cancel();
        fs::write(&release, b"release").unwrap();
        let output = handle.join().unwrap().unwrap();
        assert_eq!(output.stdout, b"taildiagnostic");
        assert!(output.stderr.is_empty());
    });
}

#[test]
fn captured_context_and_native_errors_retain_binary_bytes_and_error_sources() {
    use pixiv_app::lifecycle::ContextError;
    use std::error::Error;

    for source in [
        HostProcessError::Context(ContextError::Canceled),
        HostProcessError::Native(std::io::Error::other(
            "exec: canceling Cmd: synthetic cancellation failure",
        )),
    ] {
        let message = source.to_string();
        let error = HostProcessError::Captured {
            source: Box::new(source),
            stdout: vec![0, 255, 128, b'e', b'r', b'r', b't', b'a', b'i', b'l'],
            stderr: Vec::new(),
        };
        assert_eq!(error.to_string(), message);
        assert_eq!(error.source().unwrap().to_string(), message);
        assert_eq!(
            error.captured_stdout(),
            [0, 255, 128, b'e', b'r', b'r', b't', b'a', b'i', b'l']
        );
    }
    assert!(
        HostProcessError::Context(ContextError::Canceled)
            .captured_stdout()
            .is_empty()
    );
}
