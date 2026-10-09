#![cfg(unix)]
use pixiv_cli_rs::{auth_accounts::AccountPrompts, terminal_prompt::TerminalPrompts};
use std::{
    fs::File,
    io::{Read, Write},
    os::fd::{AsRawFd, FromRawFd},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[test]
#[ignore = "child-only PTY helper"]
fn login_visible_input_child() {
    let Ok(default) = std::env::var("PIXIV_LOGIN_INPUT_CHILD") else {
        return;
    };
    let mut prompt = TerminalPrompts::new(std::io::stdin(), std::io::stdout(), std::io::stderr());
    match prompt.input("Sign-in value", &default) {
        Ok(answer) => eprintln!("\nANSWER={answer}"),
        Err(error) => eprintln!("\nERROR={error}"),
    }
}

#[test]
fn visible_login_input_uses_tty_editing_validation_defaults_and_restores_terminal() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/cli_login_flow.json")).unwrap();
    for case in fixture["terminal_input"].as_array().unwrap() {
        // A synthetic reader EOF is not a native TTY keystroke.
        if case["name"] == "EOF" {
            continue;
        }
        let default = case["default_value"].as_str().unwrap();
        let input = case["input"].as_str().unwrap();
        let error = case["error"].as_str().unwrap();
        let expected = if error.is_empty() {
            format!("ANSWER={}", case["value"].as_str().unwrap())
        } else {
            format!("ERROR={error}")
        };
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "login_visible_input_child",
                "--nocapture",
                "--ignored",
            ])
            .env("PIXIV_LOGIN_INPUT_CHILD", default)
            .env("CLICOLOR_FORCE", "1");
        let output = run_terminal(command, input.as_bytes(), &expected, true);
        if case["name"].as_str().unwrap().ends_with("retry") {
            assert!(output.contains("value cannot be empty"));
        }
        if error.is_empty() {
            assert!(output.contains(case["value"].as_str().unwrap()));
        }
    }
}

#[test]
fn visible_login_input_rejects_non_tty_without_consuming_stdin() {
    let mut prompt = TerminalPrompts::new(std::io::stdin(), std::io::stdout(), std::io::stderr());
    if !prompt.can_prompt() {
        assert_eq!(
            prompt.input("Sign-in value", "").unwrap_err().to_string(),
            "interactive prompt is only available on a TTY"
        );
    }
}
fn run_terminal(mut command: Command, input: &[u8], expected: &str, success: bool) -> String {
    let mut master = -1;
    let mut slave = -1;
    let size = libc::winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                &size,
            )
        },
        0
    );
    let mut master = unsafe { File::from_raw_fd(master) };
    let slave = unsafe { File::from_raw_fd(slave) };
    let mut previous = std::mem::MaybeUninit::<libc::termios>::uninit();
    assert_eq!(
        unsafe { libc::tcgetattr(slave.as_raw_fd(), previous.as_mut_ptr()) },
        0
    );
    let previous = unsafe { previous.assume_init() };
    let mut child = command
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::from(slave.try_clone().unwrap()))
        .stderr(Stdio::from(slave.try_clone().unwrap()))
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut output = Vec::new();
    let mut sent = false;
    loop {
        let mut poll = libc::pollfd {
            fd: master.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        if unsafe { libc::poll(&mut poll, 1, 20) } > 0 {
            let mut buffer = [0; 4096];
            match master.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => output.extend_from_slice(&buffer[..count]),
                Err(error) if error.raw_os_error() == Some(libc::EIO) => break,
                Err(error) => panic!("terminal read: {error}"),
            }
        }
        let text = String::from_utf8_lossy(&output);
        if !sent
            && (text.contains("Use arrows")
                || text.contains("(y/N)")
                || text.contains("Refresh token")
                || text.contains("Sign-in value"))
        {
            if input.starts_with(b"\x1bj") || input.starts_with(b"\x1bk") {
                master.write_all(&input[..1]).unwrap();
                std::thread::sleep(Duration::from_millis(60));
                master.write_all(&input[1..]).unwrap();
            } else {
                master.write_all(input).unwrap();
            }
            sent = true;
        }
        if let Some(status) = child.try_wait().unwrap() {
            loop {
                let mut ready = libc::pollfd {
                    fd: master.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                };
                if unsafe { libc::poll(&mut ready, 1, 0) } <= 0 || ready.revents & libc::POLLIN == 0
                {
                    break;
                }
                let mut buffer = [0; 4096];
                match master.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => output.extend_from_slice(&buffer[..count]),
                    Err(error) if error.raw_os_error() == Some(libc::EIO) => break,
                    Err(error) => panic!("terminal read after exit: {error}"),
                }
            }
            let text = String::from_utf8_lossy(&output);
            assert_eq!(status.success(), success, "{text}");
            break;
        }
        if Instant::now() > deadline {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("prompt did not finish: {text}");
        }
    }
    let mut restored = std::mem::MaybeUninit::<libc::termios>::uninit();
    assert_eq!(
        unsafe { libc::tcgetattr(slave.as_raw_fd(), restored.as_mut_ptr()) },
        0
    );
    let restored = unsafe { restored.assume_init() };
    assert_eq!(
        restored.c_lflag, previous.c_lflag,
        "terminal flags must be restored"
    );
    assert_eq!(
        restored.c_cc, previous.c_cc,
        "terminal control bytes must be restored"
    );
    let text = String::from_utf8(output).unwrap();
    assert!(
        text.contains(expected),
        "expected {expected:?}, got {text:?}"
    );
    assert!(text.contains("\x1b[?25h"), "cursor must be restored");
    text
}
