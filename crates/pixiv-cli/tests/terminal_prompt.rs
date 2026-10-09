#![cfg(unix)]

#[path = "support/auth_accounts.rs"]
mod auth_support;

use pixiv_cli_rs::{auth_accounts::AccountPrompts, terminal_prompt::TerminalPrompts};
use std::{
    fs::File,
    io::{Read, Write},
    os::fd::{AsRawFd, FromRawFd},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[test]
fn terminal_prompt_child() {
    let Ok(kind) = std::env::var("PIXIV_PROMPT_CONTRACT_CHILD") else {
        return;
    };
    let mut prompt = TerminalPrompts::new(std::io::stdin(), std::io::stdout(), std::io::stderr());
    assert!(prompt.can_prompt());
    let result = if kind == "select-page" {
        prompt.select(
            "Select account",
            &(1..=8).map(|value| value.to_string()).collect::<Vec<_>>(),
        )
    } else if kind == "select-unicode" {
        prompt.select("Select account", &["İpek".into(), "Bob".into()])
    } else if kind == "select" {
        prompt.select(
            "Select account",
            &["11 Alice".into(), "22 Bob".into(), "33 キャロル".into()],
        )
    } else {
        prompt
            .confirm("Remove?", false)
            .map(|answer| answer.to_string())
    };
    match result {
        Ok(answer) => eprintln!("ANSWER={answer}"),
        Err(error) => eprintln!("ERROR={error}"),
    }
}

fn terminal_case(kind: &str, input: &[u8], expected: &str) -> String {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "terminal_prompt_child", "--nocapture"])
        .env("PIXIV_PROMPT_CONTRACT_CHILD", kind)
        .env("CLICOLOR_FORCE", "1");
    run_terminal(command, input, expected, true)
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
        if !sent && (text.contains("Use arrows") || text.contains("(y/N)")) {
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

#[test]
fn select_preserves_frozen_survey_keyboard_contract() {
    for (input, answer) in [
        ("\r", "11 Alice"),
        ("\x1b[B\r", "22 Bob"),
        ("\x1b[A\r", "33 キャロル"),
        ("\t\r", "22 Bob"),
        ("\x1bj\r", "22 Bob"),
        ("\x1bk\r", "33 キャロル"),
        ("BOB\r", "22 Bob"),
        ("キャ\r", "33 キャロル"),
        ("キャx\x7f\r", "33 キャロル"),
        ("BOB\x17\r", "11 Alice"),
        ("BOB\x18\r", "11 Alice"),
        ("BOB\x15\r", "22 Bob"),
        ("\x1b[B\x04", "22 Bob"),
    ] {
        terminal_case("select", input.as_bytes(), &format!("ANSWER={answer}"));
    }
    terminal_case("select", b"\x03", "ERROR=interrupt");
}

#[test]
fn confirm_preserves_frozen_survey_keyboard_contract() {
    for (input, answer) in [
        ("\r", false),
        ("YeS\r", true),
        ("NO\r", false),
        (" yes \rY\r", true),
        ("maybe\rn\r", false),
        ("ys\x1b[De\r", true),
        ("yesx\x7f\r", true),
        ("xyes\x1b[H\x1b[3~\r", true),
        ("\x04", false),
    ] {
        terminal_case("confirm", input.as_bytes(), &format!("ANSWER={answer}"));
    }
    terminal_case("confirm", b"\x03", "ERROR=interrupt");
}

#[test]
fn simple_unicode_filter_and_initial_ansi_fragment_match_go() {
    let output = terminal_case("select-unicode", b"ipek\r", "ANSWER=İpek");
    let expected = "\x1b[1;92m? \x1b[0m\x1b[1;99mSelect account\x1b[0m  \x1b[36m[Use arrows to move, type to filter]\x1b[0m\r\n\x1b[1;36m> İpek\x1b[0m\r\n\x1b[39m  Bob\x1b[0m\r\n";
    assert!(
        output.contains(expected),
        "missing Go ANSI fragment: {output:?}"
    );
}

#[test]
fn pagination_preserves_seven_visible_options_and_navigation() {
    for input in ["\x1b[A\r".to_owned(), "\x1b[B".repeat(7) + "\r"] {
        let output = terminal_case("select-page", input.as_bytes(), "ANSWER=8");
        let mut expected = "\x1b[1;36m> 1\x1b[0m\r\n".to_owned();
        for option in 2..=7 {
            expected.push_str(&format!("\x1b[39m  {option}\x1b[0m\r\n"));
        }
        assert!(
            output.contains(&expected),
            "first page must contain seven options: {output:?}"
        );
        assert!(
            output.contains("\x1b[1;36m> 8\x1b[0m\r\n"),
            "last option must be rendered"
        );
    }
}
#[test]
fn end_transmission_without_matches_returns_bounded_error() {
    terminal_case("select", b"ZZZ\x04", "ERROR=no available options");
}

#[test]
fn genuine_cli_terminal_selection_and_confirmation_persist_synthetic_state() {
    for (args, input, expected, success) in [
        (vec!["auth", "use"], "\x1b[B\r", "default uid: 1", true),
        (
            vec!["auth", "remove", "2"],
            "\r",
            "account removal canceled",
            false,
        ),
        (
            vec!["auth", "remove", "2"],
            "yes\r",
            "account uid:2 removed",
            true,
        ),
    ] {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(".pixiv-cli/config.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let before = "# synthetic terminal fixture\n[pixiv.auth]\ndefault_user_id=2\n";
        std::fs::write(&path, before).unwrap();
        let mut case = serde_json::json!({"seed":true,"database":true,"name":"synthetic terminal persistence", "states":[{"id":2,"revision":1,"schedulable":0,"frozen":4102444800_i64,"marker":1},{"id":1,"revision":1,"schedulable":1,"frozen":null,"marker":0}]});
        auth_support::seed(&case, home.path());
        let mut command = Command::new(env!("CARGO_BIN_EXE_pixiv"));
        command
            .args(&args)
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .env("CLICOLOR_FORCE", "1");
        for key in [
            "HTTPS_PROXY",
            "https_proxy",
            "HTTP_PROXY",
            "http_proxy",
            "ALL_PROXY",
            "DOWNLOAD_PATH",
            "FILENAME_TEMPLATE",
            "DIRECTORY_TEMPLATE",
            "PIXIV_REQUEST_INTERVAL",
            "PIXIV_LOG_LEVEL",
            "PIXIV_LOG_FORMAT",
            "SAUCENAO_API_KEY",
        ] {
            command.env_remove(key);
        }
        let output = run_terminal(command, input.as_bytes(), expected, success);
        let database = pixiv_app::database::Database::open(home.path().join(".pixiv-cli")).unwrap();
        let accounts = database.list_pixiv().unwrap();
        let store = pixiv_app::config::Store::new(&path);
        if args[1] == "use" {
            assert_eq!(store.read_pixiv_default_user_id().unwrap(), Some(1));
            assert_eq!(accounts.len(), 2);
        } else if success {
            assert_eq!(store.read_pixiv_default_user_id().unwrap(), None);
            assert!(output.contains("default uid: 1"));
            assert_eq!(accounts.len(), 1);
            assert_eq!(accounts[0].user_id, 1);
            assert_eq!(accounts[0].refresh_token_copy(), b"synthetic-secret");
            case["states"] =
                serde_json::json!([{"id":1,"revision":1,"schedulable":1,"frozen":null,"marker":0}]);
        } else {
            assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
            assert_eq!(store.read_pixiv_default_user_id().unwrap(), Some(2));
            assert_eq!(accounts.len(), 2);
        }
        auth_support::assert_states(&case, home.path());
    }
}
