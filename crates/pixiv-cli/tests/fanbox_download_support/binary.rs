use super::{isolation, schema::Input, state};
use std::{
    os::unix::process::CommandExt,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub struct Observation {
    pub exit: i32,
    pub stdout: String,
    pub stderr: String,
}
pub fn observe(home: &Path, mut input: Input) -> Observation {
    input.saved = "none".into();
    assert!(input.seed_files.is_empty());
    let directory = home.join(".pixiv-cli");
    state::seed(&directory, &input);
    let database = directory.join("pixiv-cli.db");
    let before = state::rows(&database);
    let config_before = std::fs::read(directory.join("config.toml")).unwrap();
    let temp = home.join("temp");
    std::fs::create_dir(&temp).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_pixiv"));
    command
        .args(&input.args)
        .env_clear()
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("TMPDIR", &temp)
        .env("TMP", &temp)
        .env("TEMP", &temp)
        .env("XDG_CONFIG_HOME", home.join("xdg-config"))
        .env("XDG_DATA_HOME", home.join("xdg-data"))
        .env("XDG_CACHE_HOME", home.join("xdg-cache"))
        .env("XDG_STATE_HOME", home.join("xdg-state"))
        .env("XDG_RUNTIME_DIR", home.join("xdg-runtime"))
        .env("APPDATA", home.join("appdata"))
        .env("LOCALAPPDATA", home.join("local-appdata"))
        .env("PATH", "")
        .env("TZ", "Asia/Tokyo")
        .current_dir(home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for variable in ["LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH", "SYSTEMROOT"] {
        if let Some(value) = std::env::var_os(variable) {
            command.env(variable, value);
        }
    }
    let filter = isolation::network_filter();
    unsafe {
        command.pre_exec(move || isolation::apply_network_filter(&filter));
    }
    let mut child = command.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!(
                "owned download binary timed out: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert_eq!(
        state::rows(&database),
        before,
        "actual binary changed saved account/schema rows"
    );
    assert_eq!(
        std::fs::read(directory.join("config.toml")).unwrap(),
        config_before,
        "actual binary changed config"
    );
    assert!(
        state::files(home, &input.output_root).is_empty(),
        "actual parser/help/no-account binary created media output"
    );
    assert_eq!(
        std::fs::read_dir(&temp).unwrap().count(),
        0,
        "actual binary left temp files"
    );
    assert!(
        std::fs::read_dir(home).unwrap().all(|entry| matches!(
            entry.unwrap().file_name().to_str().unwrap(),
            ".pixiv-cli" | "temp"
        )),
        "actual binary touched native startup directories"
    );
    Observation {
        exit: output
            .status
            .code()
            .expect("actual binary stopped without exit code"),
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: String::from_utf8(output.stderr).unwrap(),
    }
}
