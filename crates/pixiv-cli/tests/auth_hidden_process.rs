#![cfg(target_os = "linux")]

use std::{
    io::Write,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

fn fixture() -> serde_json::Value {
    serde_json::from_str(include_str!("fixtures/cli_login_hidden_startup.json")).unwrap()
}

#[test]
fn hidden_and_normal_process_routes_preserve_go_stdio_and_saved_bytes() {
    for case in fixture()["cli"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let calls = case["calls"].as_array().unwrap();
        let is_native_callback = calls
            .iter()
            .any(|call| !matches!(call.as_str().unwrap(), "cleanup" | "supported" | "ensure"));
        if is_native_callback
            || (!calls.is_empty()
                && (!case["cleanup_error"].as_str().unwrap().is_empty()
                    || !case["ensure_error"].as_str().unwrap().is_empty()))
            || !case["callback_kind"].as_str().unwrap().is_empty()
        {
            continue;
        }
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(".pixiv-cli/config.toml");
        if case["config_blocked"] == true {
            std::fs::write(
                home.path().join(".pixiv-cli"),
                b"synthetic blocked directory",
            )
            .unwrap();
        }
        if let Some(before) = case["before"].as_str() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, before).unwrap();
        }
        let mut command = Command::new(env!("CARGO_BIN_EXE_pixiv"));
        command.args(
            case["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap()),
        );
        for key in [
            "HTTPS_PROXY",
            "https_proxy",
            "HTTP_PROXY",
            "http_proxy",
            "ALL_PROXY",
            "PIXIV_ACCESS_TOKEN",
            "DOWNLOAD_PATH",
            "FILENAME_TEMPLATE",
            "DIRECTORY_TEMPLATE",
            "PIXIV_REQUEST_INTERVAL",
            "PIXIV_LOG_LEVEL",
            "PIXIV_LOG_FORMAT",
            "SAUCENAO_API_KEY",
            "PIXIV_CLIENT_DIR",
            "PIXIV_CONFIG_DIR",
            "PIXIV_DATA_DIR",
            "XDG_DATA_HOME",
            "XDG_CONFIG_HOME",
        ] {
            command.env_remove(key);
        }
        let mut child = command
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .env("PATH", home.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        let _ = input.write_all(case["input"].as_str().unwrap().as_bytes());
        let deadline = Instant::now() + Duration::from_secs(5);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("{name}: command waited for stdin despite the no-input contract");
            }
            thread::sleep(Duration::from_millis(5));
        }
        drop(input);
        let output = child.wait_with_output().unwrap();
        let normalize = |bytes: &[u8]| {
            String::from_utf8(bytes.to_vec())
                .unwrap()
                .replace(home.path().to_str().unwrap(), "<HOME>")
        };
        assert_eq!(
            output.status.code(),
            Some(case["exit"].as_i64().unwrap() as i32),
            "{name}"
        );
        if name == "root-help-hides-internals" {
            let body = normalize(&output.stdout);
            assert!(!body.contains("_callback"), "{name}");
            assert!(!body.contains("_install-handler"), "{name}");
        } else {
            assert_eq!(
                normalize(&output.stdout),
                case["stdout"].as_str().unwrap(),
                "{name}"
            );
        }
        if case["config_blocked"] == true {
            assert_eq!(
                case["stderr"].as_str().unwrap(),
                "error: mkdir <HOME>/.pixiv-cli: not a directory\n",
                "{name}: retain the frozen Go native-path diagnostic"
            );
            assert_eq!(
                normalize(&output.stderr),
                "error: File exists (os error 17)\n",
                "{name}: documented Rust native-IO diagnostic difference"
            );
        } else {
            assert_eq!(
                normalize(&output.stderr),
                case["stderr"].as_str().unwrap(),
                "{name}"
            );
        }
        assert_eq!(path.exists(), case["config"].as_bool().unwrap(), "{name}");
        if path.exists() {
            assert_eq!(
                std::fs::read(&path).unwrap(),
                case["after"].as_str().unwrap().as_bytes(),
                "{name}"
            );
        }
        assert_eq!(
            home.path().join(".pixiv-cli/pixiv-cli.db").exists(),
            case["database"].as_bool().unwrap(),
            "{name}"
        );
        assert!(
            !home
                .path()
                .join(".pixiv-cli/remote-login-session.json")
                .exists(),
            "{name}"
        );
        assert!(
            !home
                .path()
                .join(".pixiv-cli/url-handler/handler-manifest.json")
                .exists(),
            "{name}"
        );
    }
}

#[test]
fn default_hidden_callback_validates_link_before_resolving_home_and_local_state() {
    for (raw, expected) in [
        (
            "https://invalid.example/?code=private",
            "error: invalid Pixiv login link\n",
        ),
        (
            "pixiv://account/login?code=synthetic",
            "error: $HOME is not defined\n",
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_pixiv"))
            .args(["auth", "_callback", raw])
            .env("HOME", "")
            .env("USERPROFILE", "")
            .env("PATH", directory.path())
            .current_dir(directory.path())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{raw}");
        assert!(output.stdout.is_empty(), "{raw}");
        assert_eq!(output.stderr, expected.as_bytes(), "{raw}");
        assert!(
            std::fs::read_dir(directory.path())
                .unwrap()
                .next()
                .is_none(),
            "{raw}"
        );
    }
}

#[test]
fn default_remote_start_posts_before_resolving_home_and_never_opens_browser_on_state_failure() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };
    let directory = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "remote start did not request the relay before resolving HOME"
                    );
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("relay accept failed: {error}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut received = Vec::new();
        let mut buffer = [0; 4096];
        let (header_end, content_length) = loop {
            let count = stream.read(&mut buffer).unwrap();
            assert!(count > 0, "relay request ended before its headers");
            received.extend_from_slice(&buffer[..count]);
            assert!(
                received.len() <= 64 * 1024,
                "relay request exceeded the expected size"
            );
            if let Some(end) = received.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                let header = std::str::from_utf8(&received[..end]).unwrap();
                let length = header
                    .lines()
                    .find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .expect("relay request omitted Content-Length");
                break (end + 4, length);
            }
        };
        while received.len() < header_end + content_length {
            let count = stream.read(&mut buffer).unwrap();
            assert!(count > 0, "relay request ended before its JSON body");
            received.extend_from_slice(&buffer[..count]);
        }
        let header = std::str::from_utf8(&received[..header_end]).unwrap();
        assert!(
            header.starts_with("POST /start/synthetic HTTP/1.1\r\n"),
            "{header}"
        );
        assert!(
            header.lines().any(|line| {
                line.split_once(':').is_some_and(|(key, value)| {
                    key.eq_ignore_ascii_case("content-type") && value.trim() == "application/json"
                })
            }),
            "{header}"
        );
        assert_eq!(
            &received[header_end..header_end + content_length],
            b"{\"proof\":\"synthetic\"}"
        );
        let body = r#"{"authorization_url":"https://app-api.pixiv.net/web/v1/login?client=pixiv-android&code_challenge_method=S256&code_challenge=synthetic&state=synthetic"}"#;
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    });
    let raw = format!(
        "pixiv://account/remote-login?origin=http%3A%2F%2F127.0.0.1%3A{}&session=synthetic&access=synthetic",
        address.port()
    );
    let mut command = Command::new(env!("CARGO_BIN_EXE_pixiv"));
    command.args(["auth", "_callback", &raw]);
    for key in [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
        "NO_PROXY",
        "no_proxy",
    ] {
        command.env_remove(key);
    }
    let output = command
        .env("HOME", "")
        .env("USERPROFILE", "")
        .env("PATH", directory.path())
        .current_dir(directory.path())
        .output()
        .unwrap();
    server.join().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(output.stderr, b"error: $HOME is not defined\n");
    assert!(
        std::fs::read_dir(directory.path())
            .unwrap()
            .next()
            .is_none()
    );
}
