#[path = "support/auth_accounts.rs"]
mod auth_support;
use pixiv_cli_rs::{auth_accounts::AuthCommand, finish_command};
use std::io::{self, Read, Write};

struct Reader {
    body: io::Cursor<Vec<u8>>,
    failed: bool,
}
impl Read for Reader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.failed {
            Err(io::Error::other("fixture read failed"))
        } else {
            self.body.read(buffer)
        }
    }
}
struct Writer {
    body: Vec<u8>,
    failed: bool,
    short: bool,
}
impl Write for Writer {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if self.short {
            Ok(0)
        } else if self.failed {
            Err(io::Error::other("fixture write failed"))
        } else {
            self.body.extend_from_slice(buffer);
            Ok(buffer.len())
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[test]
fn auth_selection_preserves_go_input_and_writer_failure_stages() {
    let cases: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/cli-auth-selection.json"
    ))
    .unwrap();
    for case in cases {
        if case["short_write"].as_bool() != Some(true)
            && case["read_error"].as_bool() != Some(true)
            && case["write_error"].as_bool() != Some(true)
            && case["diagnostics_error"].as_bool() != Some(true)
        {
            continue;
        }
        let home = tempfile::tempdir().unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child.args(["--exact", "auth_selection_isolated_boundary", "--nocapture"]);
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
            child.env_remove(key);
        }
        for (key, value) in case["environment"].as_object().unwrap() {
            child.env(key, value.as_str().unwrap());
        }
        let output = child
            .env("AUTH_BOUNDARY_CASE", case.to_string())
            .env("AUTH_BOUNDARY_HOME", home.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}: {} {}",
            case["name"],
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
#[test]
fn auth_selection_isolated_boundary() {
    let Ok(encoded) = std::env::var("AUTH_BOUNDARY_CASE") else {
        return;
    };
    let case: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    let path = std::path::PathBuf::from(std::env::var_os("AUTH_BOUNDARY_HOME").unwrap())
        .join(".pixiv-cli/config.toml");
    if let Some(before) = case["before"].as_str() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, before).unwrap();
    }
    auth_support::seed(&case, path.parent().unwrap().parent().unwrap());
    let store = pixiv_app::config::Store::new(&path);
    let args: Vec<String> = case["args"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|value| value.as_str().unwrap().into())
        .collect();
    let mut input = Reader {
        body: io::Cursor::new(case["input"].as_str().unwrap().as_bytes().to_vec()),
        failed: case["read_error"].as_bool() == Some(true),
    };
    let mut output = Writer {
        body: Vec::new(),
        failed: case["write_error"].as_bool() == Some(true),
        short: case["short_write"].as_bool() == Some(true),
    };
    let mut diagnostics = Writer {
        body: Vec::new(),
        failed: case["diagnostics_error"].as_bool() == Some(true),
        short: false,
    };
    let machine = pixiv_cli_rs::auth_accounts::machine_output_requested(&args);
    let result = AuthCommand::parse(&args, &mut input, false).and_then(|command| {
        command.execute(&store, &pixiv_app::lifecycle::Context::new(), &mut output)
    });
    let exit = finish_command(result, false, machine, &mut diagnostics);
    assert_eq!(
        exit,
        case["exit"].as_i64().unwrap() as i32,
        "{}",
        case["name"]
    );
    assert_eq!(
        output.body,
        case["stdout"].as_str().unwrap().as_bytes(),
        "{}",
        case["name"]
    );
    assert_eq!(
        diagnostics.body,
        case["stderr"].as_str().unwrap().as_bytes(),
        "{}",
        case["name"]
    );
    auth_support::assert_states(&case, path.parent().unwrap().parent().unwrap());
    assert_eq!(
        path.exists(),
        case["config"].as_bool().unwrap(),
        "{}",
        case["name"]
    );
    if path.exists() {
        assert_eq!(
            std::fs::read(&path).unwrap(),
            case["after"].as_str().unwrap().as_bytes(),
            "{}",
            case["name"]
        );
    }
}
