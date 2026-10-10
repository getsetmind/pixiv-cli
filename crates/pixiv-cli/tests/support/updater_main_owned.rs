use serde_json::Value;
use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Output, Stdio},
};

pub fn fixture() -> Vec<Value> {
    serde_json::from_str::<Value>(include_str!("../fixtures/updater-cli-owner.json")).unwrap()["cases"].as_array().unwrap().clone()
}
pub fn args(row: &Value) -> Vec<&str> {
    row["input"]["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|arg| arg.as_str().unwrap())
        .collect()
}
pub fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).unwrap()
}
pub struct OwnedHome {
    directory: tempfile::TempDir,
}
impl OwnedHome {
    pub fn new(config: Option<&str>) -> Self {
        let directory = tempfile::tempdir().unwrap();
        if let Some(config) = config {
            std::fs::create_dir(directory.path().join(".pixiv-cli")).unwrap();
            std::fs::write(directory.path().join(".pixiv-cli/config.toml"), config).unwrap();
        }
        Self { directory }
    }
    pub fn config(&self) -> PathBuf {
        self.directory.path().join(".pixiv-cli/config.toml")
    }
    pub fn run(&self, args: &[&str], input: &str) -> Output {
        self.run_with_environment(args, input, &[])
    }
    pub fn run_with_environment(
        &self,
        args: &[&str],
        input: &str,
        environment: &[(&str, &str)],
    ) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_pixiv"));
        command
            .args(args)
            .current_dir(self.directory.path())
            .env_clear()
            .env("HOME", self.directory.path())
            .env("USERPROFILE", self.directory.path())
            .env("XDG_CONFIG_HOME", self.directory.path())
            .env("XDG_DATA_HOME", self.directory.path())
            .env("PIXIV_BUILD_VERSION", "v99.98.97-runtime-must-be-ignored")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command.envs(environment.iter().copied());
        deny_network(&mut command);
        let mut child = command.spawn().unwrap();
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(input.as_bytes());
        }
        child.wait_with_output().unwrap()
    }
    pub fn assert_config(&self, expected: &str) {
        assert_eq!(std::fs::read_to_string(self.config()).unwrap(), expected);
    }
    pub fn assert_no_database_or_cache(&self) {
        assert!(
            !self
                .directory
                .path()
                .join(".pixiv-cli/pixiv-cli.db")
                .exists()
        );
        assert!(
            !self
                .directory
                .path()
                .join(".pixiv-cli/cache/github-releases.json")
                .exists()
        );
    }
    pub fn assert_no_config_or_database(&self) {
        assert!(!self.config().exists());
        self.assert_no_database_or_cache();
    }
}

#[cfg(target_os = "linux")]
fn deny_network(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        command.pre_exec(|| {
            let mut filters = [
                libc::sock_filter {
                    code: (libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16,
                    jt: 0,
                    jf: 0,
                    k: 0,
                },
                libc::sock_filter {
                    code: (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16,
                    jt: 0,
                    jf: 1,
                    k: libc::SYS_socket as u32,
                },
                libc::sock_filter {
                    code: (libc::BPF_RET | libc::BPF_K) as u16,
                    jt: 0,
                    jf: 0,
                    k: libc::SECCOMP_RET_ERRNO | libc::EPERM as u32,
                },
                libc::sock_filter {
                    code: (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16,
                    jt: 0,
                    jf: 1,
                    k: libc::SYS_connect as u32,
                },
                libc::sock_filter {
                    code: (libc::BPF_RET | libc::BPF_K) as u16,
                    jt: 0,
                    jf: 0,
                    k: libc::SECCOMP_RET_ERRNO | libc::EPERM as u32,
                },
                libc::sock_filter {
                    code: (libc::BPF_RET | libc::BPF_K) as u16,
                    jt: 0,
                    jf: 0,
                    k: libc::SECCOMP_RET_ALLOW,
                },
            ];
            let program = libc::sock_fprog {
                len: filters.len() as u16,
                filter: filters.as_mut_ptr(),
            };
            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0
                || libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER, &program) != 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}
#[cfg(not(target_os = "linux"))]
fn deny_network(_: &mut Command) {
    panic!("Owned updater process proof requires the verified Linux network-denial boundary");
}
