use pixiv_app::{
    callback_handler::BrowserOpener,
    host_process::{
        HostPlatform, HostProcess, HostProcessError, ProcessOutput, ProcessStdio, SystemHostProcess,
    },
    native_browser::NativeBrowserOpener,
};
use serde::Deserialize;
use std::{
    ffi::{OsStr, OsString},
    io,
    path::PathBuf,
    sync::{Arc, Mutex},
};

#[derive(Deserialize)]
struct Fixture {
    url: String,
    linux: Vec<ProviderCase>,
}
#[derive(Deserialize)]
struct ProviderCase {
    name: String,
    providers: Vec<String>,
    selected: String,
    exit: i32,
}
fn fixture() -> Fixture {
    serde_json::from_str(include_str!("fixtures/native_browser.json")).unwrap()
}

#[derive(Clone, Debug, PartialEq)]
enum Call {
    Lookup(OsString),
    Run(OsString, Vec<OsString>, ProcessStdio),
    Shell(String),
}
struct RecordedHost {
    platform: HostPlatform,
    providers: Vec<String>,
    calls: Arc<Mutex<Vec<Call>>>,
    run_error: bool,
    shell_error: bool,
}
impl HostProcess for RecordedHost {
    fn platform(&self) -> HostPlatform {
        self.platform
    }
    fn look_path(&self, program: &OsStr) -> Result<PathBuf, HostProcessError> {
        self.calls
            .lock()
            .unwrap()
            .push(Call::Lookup(program.to_owned()));
        if self
            .providers
            .iter()
            .any(|name| OsStr::new(name) == program)
        {
            Ok(PathBuf::from("/synthetic/bin").join(program))
        } else {
            Err(HostProcessError::Lookup {
                program: program.to_owned(),
                source: io::Error::new(io::ErrorKind::NotFound, "synthetic provider missing"),
            })
        }
    }
    fn run(
        &self,
        program: &OsStr,
        args: &[OsString],
        stdio: ProcessStdio,
    ) -> Result<ProcessOutput, HostProcessError> {
        self.calls
            .lock()
            .unwrap()
            .push(Call::Run(program.to_owned(), args.to_vec(), stdio));
        if self.run_error {
            return Err(HostProcessError::Spawn {
                program: program.to_owned(),
                source: io::Error::new(io::ErrorKind::PermissionDenied, "synthetic launch failure"),
            });
        }
        let status = {
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                std::process::ExitStatus::from_raw(0)
            }
            #[cfg(windows)]
            {
                use std::os::windows::process::ExitStatusExt;
                std::process::ExitStatus::from_raw(0)
            }
        };
        Ok(ProcessOutput {
            status,
            stdout: Vec::new(),
            stderr: Vec::new(),
        })
    }
    fn shell_open_url(&self, url: &str) -> Result<(), HostProcessError> {
        self.calls.lock().unwrap().push(Call::Shell(url.to_owned()));
        if self.shell_error {
            Err(HostProcessError::Native(io::Error::from_raw_os_error(5)))
        } else {
            Ok(())
        }
    }
}

#[test]
fn linux_provider_order_and_raw_arguments_match_go_fixture() {
    let fixture = fixture();
    for case in fixture.linux {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let opener = NativeBrowserOpener::new(RecordedHost {
            platform: HostPlatform::Linux,
            providers: case.providers,
            calls: calls.clone(),
            run_error: case.exit != 0,
            shell_error: false,
        });
        let result = opener.open(&fixture.url);
        let mut expected = Vec::new();
        for provider in ["xdg-open", "x-www-browser", "www-browser"] {
            expected.push(Call::Lookup(provider.into()));
            if provider == case.selected {
                expected.push(Call::Run(
                    provider.into(),
                    vec![fixture.url.clone().into()],
                    ProcessStdio::Inherit,
                ));
                break;
            }
        }
        assert_eq!(*calls.lock().unwrap(), expected, "{}", case.name);
        if case.selected.is_empty() {
            let error = result.unwrap_err();
            assert!(
                error
                    .downcast_ref::<HostProcessError>()
                    .unwrap()
                    .is_not_found()
            );
            assert!(
                error
                    .to_string()
                    .contains("xdg-open,x-www-browser,www-browser")
            );
        } else if case.exit != 0 {
            let error = result.unwrap_err();
            assert!(
                matches!(error.downcast_ref::<HostProcessError>(),Some(HostProcessError::Spawn{source,..}) if source.kind()==io::ErrorKind::PermissionDenied)
            );
        } else {
            result.unwrap();
        }
    }
}

#[test]
fn darwin_and_windows_dispatch_use_the_actual_host_dependency() {
    for platform in [HostPlatform::Darwin, HostPlatform::Windows] {
        for fail in [false, true] {
            let calls = Arc::new(Mutex::new(Vec::new()));
            let opener = NativeBrowserOpener::new(RecordedHost {
                platform,
                providers: Vec::new(),
                calls: calls.clone(),
                run_error: fail,
                shell_error: fail,
            });
            let url = fixture().url;
            assert_eq!(opener.open(&url).is_err(), fail);
            let expected = if platform == HostPlatform::Windows {
                Call::Shell(url)
            } else {
                Call::Run("open".into(), vec![url.into()], ProcessStdio::Inherit)
            };
            assert_eq!(*calls.lock().unwrap(), vec![expected]);
        }
    }
}

#[test]
fn bsd_missing_provider_diagnostics_and_unsupported_platform_match_go() {
    for (platform, suffix) in [
        (HostPlatform::FreeBsd, "ports(8)"),
        (HostPlatform::OpenBsd, "ports(8)"),
        (HostPlatform::NetBsd, "pkgsrc(7)"),
    ] {
        struct MissingHost(HostPlatform);
        impl HostProcess for MissingHost {
            fn platform(&self) -> HostPlatform {
                self.0
            }
            fn look_path(&self, program: &OsStr) -> Result<PathBuf, HostProcessError> {
                Err(HostProcessError::Lookup {
                    program: program.to_owned(),
                    source: io::Error::new(io::ErrorKind::NotFound, "missing"),
                })
            }
            fn run(
                &self,
                program: &OsStr,
                _: &[OsString],
                _: ProcessStdio,
            ) -> Result<ProcessOutput, HostProcessError> {
                Err(HostProcessError::Lookup {
                    program: program.to_owned(),
                    source: io::Error::new(io::ErrorKind::NotFound, "missing"),
                })
            }
            fn shell_open_url(&self, _: &str) -> Result<(), HostProcessError> {
                panic!("no native shell expected")
            }
        }
        let error = NativeBrowserOpener::new(MissingHost(platform))
            .open("synthetic://url")
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("xdg-open: command not found - install xdg-utils from {suffix}")
        );
    }
    let calls = Arc::new(Mutex::new(Vec::new()));
    let opener = NativeBrowserOpener::new(RecordedHost {
        platform: HostPlatform::Unsupported("plan9"),
        providers: Vec::new(),
        calls: calls.clone(),
        run_error: false,
        shell_error: false,
    });
    assert_eq!(
        opener.open("synthetic://url").unwrap_err().to_string(),
        "openBrowser: unsupported operating system: plan9"
    );
    assert!(calls.lock().unwrap().is_empty());
}

#[cfg(target_os = "linux")]
#[test]
#[ignore]
fn synthetic_host_process_child() {
    if std::env::var_os("PIXIV_SYNTHETIC_BROWSER_CHILD").is_none() {
        return;
    }
    let result = NativeBrowserOpener::default().open(&fixture().url);
    match result {
        Ok(()) => println!("result:success"),
        Err(error) => match error.downcast_ref::<HostProcessError>().unwrap() {
            HostProcessError::Exit { output, .. } => {
                println!("result:exit:{}", output.status.code().unwrap())
            }
            HostProcessError::Lookup { .. } => println!("result:not_found"),
            HostProcessError::Spawn { .. } => println!("result:spawn:{error}"),
            other => panic!("unexpected result: {other}"),
        },
    }
}

#[cfg(target_os = "linux")]
#[test]
fn real_process_boundary_runs_only_synthetic_providers_in_isolated_children() {
    use std::{fs, os::unix::fs::PermissionsExt, process::Command};
    let fixture = fixture();
    for case in fixture.linux {
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join("bin");
        fs::create_dir(&bin).unwrap();
        for provider in &case.providers {
            let code = if provider == &case.selected {
                case.exit
            } else {
                0
            };
            let script = format!(
                "#!/bin/sh\nprintf '%s\\n' '{provider}' \"$#\" \"$1\" \"$PWD\" \"$HOME\" \"$SYNTHETIC_BROWSER_ENV\" > \"$HOME/invocation\"\nif read value; then printf 'stdin-data\\n' >> \"$HOME/invocation\"; else printf 'stdin-eof\\n' >> \"$HOME/invocation\"; fi\nprintf 'provider-output\\n'\nprintf 'provider-error\\n' >&2\nprintf 'finished\\n' >> \"$HOME/invocation\"\nexit {code}\n"
            );
            let path = bin.join(provider);
            fs::write(&path, script).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "synthetic_host_process_child",
                "--ignored",
                "--nocapture",
            ])
            .env_clear()
            .env("PIXIV_SYNTHETIC_BROWSER_CHILD", "1")
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .env("PATH", &bin)
            .env("BROWSER", "must-not-be-used")
            .env("SYNTHETIC_BROWSER_ENV", "inherited")
            .current_dir(home.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}: {}",
            case.name,
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        if case.selected.is_empty() {
            assert!(stdout.contains("result:not_found"));
            assert!(!home.path().join("invocation").exists());
        } else {
            let result = if case.exit == 0 {
                "result:success".into()
            } else {
                format!("result:exit:{}", case.exit)
            };
            assert!(stdout.contains(&result), "{stdout}");
            assert!(stdout.contains("provider-output\n"));
            assert_eq!(
                String::from_utf8(output.stderr).unwrap(),
                "provider-error\n"
            );
            let home = home.path().to_str().unwrap();
            let expected = [
                case.selected.as_str(),
                "1",
                &fixture.url,
                home,
                home,
                "inherited",
                "stdin-eof",
                "finished",
                "",
            ]
            .join("\n");
            assert_eq!(
                fs::read_to_string(PathBuf::from(home).join("invocation")).unwrap(),
                expected
            );
        }
    }
}

#[cfg(windows)]
#[test]
fn windows_nul_panics_like_go_before_calling_shell_execute() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/native_browser.json")).unwrap();
    let panic = std::panic::catch_unwind(|| SystemHostProcess.shell_open_url("synthetic\0url"))
        .unwrap_err();
    let message = panic
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
        .unwrap();
    assert_eq!(message, fixture["windows_nul"]["message"].as_str().unwrap());
}

#[cfg(not(windows))]
#[test]
fn native_windows_shell_is_explicitly_unavailable_on_nonwindows_hosts() {
    let error = SystemHostProcess
        .shell_open_url("synthetic://url")
        .unwrap_err();
    assert!(
        matches!(error,HostProcessError::Native(source) if source.kind()==io::ErrorKind::Unsupported)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn real_lookup_skips_nonexecutables_and_relative_path_and_does_not_retry_launch_failure() {
    use std::{fs, os::unix::fs::PermissionsExt, process::Command};
    for mode in ["nonexecutable", "relative", "launch_failure"] {
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let first = if mode == "relative" {
            home.path().join("xdg-open")
        } else {
            bin.join("xdg-open")
        };
        let script = if mode == "launch_failure" {
            "#!/synthetic/interpreter/does-not-exist\n"
        } else {
            "#!/bin/sh\nprintf 'wrong-provider' > \"$HOME/invocation\"\n"
        };
        fs::write(&first, script).unwrap();
        fs::set_permissions(
            &first,
            fs::Permissions::from_mode(if mode == "nonexecutable" {
                0o600
            } else {
                0o700
            }),
        )
        .unwrap();
        let fallback = bin.join("www-browser");
        fs::write(
            &fallback,
            "#!/bin/sh\nprintf 'fallback' > \"$HOME/invocation\"\n",
        )
        .unwrap();
        fs::set_permissions(&fallback, fs::Permissions::from_mode(0o700)).unwrap();
        let path = if mode == "relative" {
            OsString::from(format!(".:{}", bin.display()))
        } else {
            bin.clone().into_os_string()
        };
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "synthetic_host_process_child",
                "--ignored",
                "--nocapture",
            ])
            .env_clear()
            .env("PIXIV_SYNTHETIC_BROWSER_CHILD", "1")
            .env("HOME", home.path())
            .env("PATH", path)
            .current_dir(home.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        if mode == "launch_failure" {
            assert!(
                stdout.contains(&format!(
                    "result:spawn:fork/exec {}: no such file or directory",
                    first.display()
                )),
                "{stdout}"
            );
            assert!(!home.path().join("invocation").exists());
        } else {
            assert!(stdout.contains("result:success"), "{stdout}");
            assert_eq!(
                fs::read_to_string(home.path().join("invocation")).unwrap(),
                "fallback"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn captured_process_failures_retain_raw_status_and_output_without_adding_it_to_diagnostics() {
    use std::{fs, os::unix::fs::PermissionsExt};
    let home = tempfile::tempdir().unwrap();
    let provider = home.path().join("provider");
    fs::write(
        &provider,
        "#!/bin/sh\nprintf 'synthetic-stdout'\nprintf 'synthetic-stderr' >&2\nexit 19\n",
    )
    .unwrap();
    fs::set_permissions(&provider, fs::Permissions::from_mode(0o700)).unwrap();
    let error = SystemHostProcess
        .run(provider.as_os_str(), &[], ProcessStdio::Capture)
        .unwrap_err();
    assert_eq!(error.to_string(), "exit status 19");
    match error {
        HostProcessError::Exit { output, .. } => {
            assert_eq!(output.status.code(), Some(19));
            assert_eq!(output.stdout, b"synthetic-stdout");
            assert_eq!(output.stderr, b"synthetic-stderr");
        }
        other => panic!("unexpected process error: {other}"),
    }
}
