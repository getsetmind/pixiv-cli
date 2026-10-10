use std::process::{Command, Output, Stdio};

struct OwnedChild(std::process::Child);
impl std::ops::Deref for OwnedChild {
    type Target = std::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for OwnedChild {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

#[test]
fn saved_mcp_binary_connects_reverse_without_acquiring_an_account() {
    use std::io::{BufRead, Write};
    let home = tempfile::tempdir().unwrap();
    let directory = home.path().join(".pixiv-cli");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(
        directory.join("config.toml"),
        "update_check_enabled = false\n",
    )
    .unwrap();
    let mut child = OwnedChild(
        Command::new(env!("CARGO_BIN_EXE_pixiv"))
            .arg("mcp")
            .env_clear()
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut input = child.stdin.take().unwrap();
    let output = child.stdout.take().unwrap();
    let diagnostics = child.stderr.take().unwrap();
    let (send, receive) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in std::io::BufReader::new(output).lines() {
            if send.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let errors = std::thread::spawn(move || {
        use std::io::Read;
        let mut bytes = vec![];
        std::io::BufReader::new(diagnostics)
            .read_to_end(&mut bytes)
            .unwrap();
        bytes
    });
    for request in [
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"owned-reverse-test","version":"1"}}}),
        serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"reverse_search","arguments":{"source":"https://owned-image.invalid/unfetched"}}}),
    ] {
        writeln!(input, "{request}").unwrap();
        let response = match receive.recv_timeout(std::time::Duration::from_secs(10)) {
            Ok(response) => response,
            Err(error) => {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("owned MCP response timed out: {error}");
            }
        };
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["id"], request["id"]);
        if request["id"] == 2 {
            assert_eq!(response["result"]["isError"], true);
            assert_eq!(
                response["result"]["structuredContent"]["results"],
                serde_json::json!([])
            );
            assert!(!response.to_string().contains("owned-image.invalid"));
            assert!(response.to_string().contains("missing_credential"));
        }
    }
    drop(input);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("owned MCP close timed out");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    reader.join().unwrap();
    assert!(errors.join().unwrap().is_empty());
}

fn run(command: &mut Command) -> Output {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!("owned reverse process timed out: {:?}", output);
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn assembled_image_search_rejects_missing_credentials_without_opening_accounts_or_source() {
    let home = tempfile::tempdir().unwrap();
    let directory = home.path().join(".pixiv-cli");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(
        directory.join("config.toml"),
        "update_check_enabled = false\n",
    )
    .unwrap();
    let output = run(Command::new(env!("CARGO_BIN_EXE_pixiv"))
        .args(["search", "https://owned-image.invalid/unfetched", "--json"])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("HTTPS_PROXY", "")
        .env_remove("https_proxy")
        .env_remove("SAUCENAO_API_KEY")
        .env_remove("PIXIV_ACCESS_TOKEN"));
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        output.stderr,
        b"{\"error\":{\"code\":\"command_failed\",\"message\":\"SauceNAO API key is required\"}}\n"
    );
    assert!(!directory.join("pixiv-cli.db").exists());
}

#[test]
fn image_flag_validation_precedes_account_database_and_provider_requests() {
    let home = tempfile::tempdir().unwrap();
    let directory = home.path().join(".pixiv-cli");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(
        directory.join("config.toml"),
        "update_check_enabled = false\n",
    )
    .unwrap();
    for (flags, expected) in [
        (
            vec!["--sort=invalid"],
            "--sort is not supported for image sources",
        ),
        (
            vec!["--type=novel"],
            "--type is not supported for image sources",
        ),
        (
            vec!["--json=false", "--ndjson=false"],
            "--ndjson cannot be used with --json",
        ),
        (
            vec!["--provider=invalid"],
            "provider must be one of saucenao, ascii2d-color, ascii2d-bovw, all",
        ),
    ] {
        let output = run(Command::new(env!("CARGO_BIN_EXE_pixiv"))
            .args(["search", "HTTP://owned-image.invalid/source"])
            .args(flags)
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .env("XDG_CONFIG_HOME", home.path().join("config"))
            .env("XDG_DATA_HOME", home.path().join("data"))
            .env_remove("PIXIV_ACCESS_TOKEN")
            .env("HTTPS_PROXY", "")
            .env_remove("https_proxy"));
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            format!("error: {expected}\n")
        );
        assert!(!directory.join("pixiv-cli.db").exists());
    }
}

#[test]
fn reverse_error_uses_the_frozen_cli_non_sdk_diagnostic_code() {
    let error = pixiv_app::reverse_search::Error::new(
        pixiv_app::reverse_search::ErrorCode::MissingCredential,
        "SauceNAO API key is required",
        None,
    );
    let mut diagnostics = vec![];
    let exit = pixiv_cli_rs::finish_command(
        Err(pixiv_cli_rs::CommandError::ReverseSearch(error)),
        false,
        true,
        &mut diagnostics,
    );
    assert_eq!(exit, 1);
    assert_eq!(
        diagnostics,
        b"{\"error\":{\"code\":\"command_failed\",\"message\":\"SauceNAO API key is required\"}}\n"
    );
}
