use std::{
    io::Write,
    process::{Command, Stdio},
};

pub fn assert_startup(case: &serde_json::Value) {
    let home = tempfile::tempdir().unwrap();
    let directory = home.path().join(".pixiv-cli");
    let path = directory.join("config.toml");
    if let Some(before) = case["before"].as_str() {
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(&path, before).unwrap();
    }
    assert_ne!(
        case["read_error"], true,
        "reader failures are covered at the public input boundary"
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_pixiv"))
        .arg("user")
        .args(
            case["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|arg| arg.as_str().unwrap()),
        )
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("PIXIV_ACCESS_TOKEN", "")
        .env("HTTPS_PROXY", "")
        .env_remove("https_proxy")
        .env_remove("HTTP_PROXY")
        .env_remove("http_proxy")
        .env_remove("ALL_PROXY")
        .env("REQUEST_INTERVAL", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(case["input"].as_str().unwrap().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let label = format!("{} {:?} {:?}", case["name"], case["args"], case["input"]);
    assert_eq!(
        output.status.code(),
        Some(case["exit"].as_i64().unwrap() as i32),
        "{label}"
    );
    let stdout = case["stdout"].as_str().unwrap();
    if stdout.starts_with('{') {
        assert_eq!(
            super::json_object_order::canonicalize_ndjson(&output.stdout),
            super::json_object_order::canonicalize_ndjson(stdout.as_bytes()),
            "{label}"
        );
    } else {
        assert_eq!(output.stdout, stdout.as_bytes(), "{label}");
    }
    let expected = case["stderr"].as_str().unwrap();
    if expected.starts_with('{') {
        assert_eq!(
            super::json_object_order::canonicalize(&output.stderr),
            super::json_object_order::canonicalize(expected.as_bytes()),
            "{label}"
        );
    } else {
        assert_eq!(output.stderr, expected.as_bytes(), "{label}");
    }
    assert_eq!(path.exists(), case["config"].as_bool().unwrap(), "{label}");
    if path.exists() {
        assert_eq!(
            std::fs::read(&path).unwrap(),
            case["after"].as_str().unwrap().as_bytes(),
            "{label}"
        );
    }
    assert_eq!(
        directory.join("pixiv-cli.db").exists(),
        case["database"].as_bool().unwrap(),
        "{label}"
    );
}
