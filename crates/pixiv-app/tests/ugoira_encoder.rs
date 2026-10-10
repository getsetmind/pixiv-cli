use pixiv_app::{
    download::{AnimationEncoder, EncoderInput, NativeAnimationEncoder},
    lifecycle::Context,
};
use pixiv_sdk::models::UgoiraFrame;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

#[cfg(target_os = "linux")]
#[path = "support/native_encoder_process.rs"]
mod native_encoder_process;

#[derive(Deserialize)]
struct Frame {
    file: String,
    delay: i64,
}
#[derive(Deserialize)]
struct Case {
    name: String,
    format: String,
    max_edge: u32,
    frames: Option<Vec<Frame>>,
    zip_hex: String,
    output_name: String,
    error: String,
    output_sha256: Option<String>,
}
#[derive(Deserialize)]
struct Artifact {
    path: String,
    sha256: String,
}
#[derive(Deserialize)]
struct Fixture {
    pinned_go: String,
    source_digest: String,
    source_sha256: BTreeMap<String, String>,
    artifacts: BTreeMap<String, Artifact>,
    native_execution_platform: String,
    cases: Vec<Case>,
}
fn fixture() -> Fixture {
    serde_json::from_str(include_str!("fixtures/ugoira_encoder.json")).unwrap()
}
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn hex_bytes(hex: &str) -> Vec<u8> {
    assert_eq!(hex.len() % 2, 0);
    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}
fn temporary_sinks(directory: &Path) -> Vec<PathBuf> {
    fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".ugoira-")
        })
        .collect()
}

fn source_digest(root: &Path) -> String {
    fn collect(directory: &Path, base: &Path, lock: bool, out: &mut Vec<String>) {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let kind = entry.file_type().unwrap();
            assert!(!kind.is_symlink(), "source digest cannot contain symlinks");
            if kind.is_dir() {
                if entry.file_name() != "target" {
                    collect(&entry.path(), base, lock, out)
                };
                continue;
            }
            let relative = entry
                .path()
                .strip_prefix(base)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if relative == "Cargo.toml"
                || (lock && relative == "Cargo.lock")
                || relative == "build.rs"
                || (lock && (relative.starts_with(".cargo/") || relative.starts_with("vendor/")))
                || (relative.starts_with("src/") && relative.ends_with(".rs"))
            {
                out.push(relative)
            }
        }
    }
    let mut digest = Sha256::new();
    for (logical, path, lock) in [
        ("ugoira_rs", "internal/media/ugoira/rust", true),
        ("quantette-0.6.0", "third_party/rust/quantette-0.6.0", false),
    ] {
        let base = root.join(path);
        let mut paths = Vec::new();
        collect(&base, &base, lock, &mut paths);
        paths.sort();
        for relative in paths {
            digest.update(
                format!(
                    "{logical}/{relative}\0{}\n",
                    hash(&fs::read(base.join(&relative)).unwrap())
                )
                .as_bytes(),
            )
        }
    }
    format!("{:x}", digest.finalize())
}

#[test]
fn frozen_go_native_source_and_artifact_identity_is_intact() {
    assert_eq!(
        hash(include_bytes!("fixtures/ugoira_encoder.json")),
        "88df4a179e7f5790953ac7507407bb9faa277bd736e16f5757928c63a303d662"
    );
    let f = fixture();
    let root = root();
    assert_eq!(f.pinned_go, "4b4426487ef18bed276706daec385e0d0a6979f9");
    assert_eq!(
        f.source_digest,
        "4318464a846c6a6701f1868adb48c0ccab67e0e4eb1a5a21e6f8c4fde89bbb02"
    );
    assert_eq!(source_digest(&root), f.source_digest);
    for (path, want) in f.source_sha256 {
        assert_eq!(
            hash(&fs::read(root.join(&path)).unwrap()),
            want,
            "source {path}"
        )
    }
    assert_eq!(f.artifacts.len(), 6);
    for (platform, artifact) in f.artifacts {
        assert_eq!(
            hash(
                &fs::read(
                    root.join("internal/media/ugoira/rust/staticlib")
                        .join(artifact.path)
                )
                .unwrap()
            ),
            artifact.sha256,
            "artifact {platform}"
        )
    }
}

#[tokio::test]
async fn genuine_native_encoder_matches_frozen_go_outputs_errors_and_cleanup() {
    let f = fixture();
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        return;
    }
    assert_eq!(f.native_execution_platform, "linux/amd64");
    for case in f.cases {
        let directory = tempfile::tempdir().unwrap();
        let zip_path = directory.path().join("frames.zip");
        if !case.zip_hex.is_empty() {
            fs::write(&zip_path, hex_bytes(&case.zip_hex)).unwrap()
        }
        let output_path = directory.path().join(&case.output_name);
        fs::write(&output_path, b"existing-output-must-survive-errors").unwrap();
        let input = EncoderInput {
            zip_path,
            output_path: output_path.clone(),
            work_dir: directory.path().join("unused"),
            frames: case.frames.map(|frames| {
                frames
                    .into_iter()
                    .map(|frame| UgoiraFrame {
                        filename: frame.file,
                        delay_milliseconds: frame.delay,
                    })
                    .collect()
            }),
            format: case.format,
            max_edge: case.max_edge,
        };
        let actual = NativeAnimationEncoder.encode(Context::new(), input).await;
        let error = actual
            .as_ref()
            .err()
            .map(ToString::to_string)
            .unwrap_or_default()
            .replace(&output_path.to_string_lossy().into_owned(), "${OUTPUT}");
        assert_eq!(error, case.error, "case {}", case.name);
        let output = fs::read(&output_path).unwrap();
        if let Some(want) = case.output_sha256 {
            assert_eq!(
                hash(&output),
                want,
                "case {}: native bytes bind the Go-decoded timing/dimensions/loop/alpha observations",
                case.name
            )
        } else {
            assert_eq!(
                output, b"existing-output-must-survive-errors",
                "case {}: existing output",
                case.name
            )
        }
        assert!(
            temporary_sinks(directory.path()).is_empty(),
            "case {}: temporary cleanup",
            case.name
        );
    }
}

#[tokio::test]
async fn canceled_context_preserves_existing_output_before_native_work() {
    let directory = tempfile::tempdir().unwrap();
    let out = directory.path().join("existing.gif");
    fs::write(&out, b"old").unwrap();
    let context = Context::new();
    context.cancel();
    let err = NativeAnimationEncoder
        .encode(
            context,
            EncoderInput {
                zip_path: directory.path().join("absent.zip"),
                output_path: out.clone(),
                work_dir: directory.path().to_path_buf(),
                frames: None,
                format: String::new(),
                max_edge: 0,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "context canceled");
    assert_eq!(fs::read(out).unwrap(), b"old");
    assert!(temporary_sinks(directory.path()).is_empty());
}

#[tokio::test]
async fn invalid_format_precedes_canceled_context() {
    let directory = tempfile::tempdir().unwrap();
    let context = Context::new();
    context.cancel();
    let err = NativeAnimationEncoder
        .encode(
            context,
            EncoderInput {
                zip_path: directory.path().join("absent.zip"),
                output_path: directory.path().join("out.gif"),
                work_dir: directory.path().to_path_buf(),
                frames: None,
                format: "invalid".into(),
                max_edge: 0,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "invalid ugoira animation format \"invalid\"; expected gif or apng"
    );
    assert!(temporary_sinks(directory.path()).is_empty());
}

#[tokio::test]
async fn completed_encoding_cannot_replace_an_existing_directory() {
    let f = fixture();
    let first = f.cases.into_iter().find(|case| case.name == "gif").unwrap();
    let directory = tempfile::tempdir().unwrap();
    let zip_path = directory.path().join("frames.zip");
    fs::write(&zip_path, hex_bytes(&first.zip_hex)).unwrap();
    let out = directory.path().join("existing.gif");
    fs::create_dir(&out).unwrap();
    fs::write(out.join("sentinel"), b"preserve").unwrap();
    let result = NativeAnimationEncoder
        .encode(
            Context::new(),
            EncoderInput {
                zip_path,
                output_path: out.clone(),
                work_dir: directory.path().to_path_buf(),
                frames: first.frames.map(|frames| {
                    frames
                        .into_iter()
                        .map(|frame| UgoiraFrame {
                            filename: frame.file,
                            delay_milliseconds: frame.delay,
                        })
                        .collect()
                }),
                format: "gif".into(),
                max_edge: 0,
            },
        )
        .await;
    assert!(result.is_err());
    assert!(out.is_dir());
    assert_eq!(fs::read(out.join("sentinel")).unwrap(), b"preserve");
    assert!(temporary_sinks(directory.path()).is_empty());
}

#[test]
fn genuine_core_canceled_token_retains_the_native_canceled_error() {
    let token = ugoira_rs::CancellationToken::default();
    token.cancel();
    let frames = [ugoira_rs::UgoiraFrame {
        file: "first.png".into(),
        delay: 80,
    }];
    let err = ugoira_rs::encode_gif(
        Path::new("absent.zip"),
        &frames,
        Path::new("unused.gif"),
        0,
        &token,
    )
    .unwrap_err();
    assert_eq!(err.to_string(), "ugoira encoding canceled");
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "current_thread")]
async fn genuine_blocking_native_work_owns_the_gate_until_cancel_and_cleanup_finish() {
    if std::env::var_os("PIXIV_UGOIRA_FIFO_CHILD").is_none() {
        let root = tempfile::tempdir().unwrap();
        let outcome = native_encoder_process::run(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "genuine_blocking_native_work_owns_the_gate_until_cancel_and_cleanup_finish",
                    "--nocapture",
                ])
                .env("PIXIV_UGOIRA_FIFO_CHILD", "1")
                .env("PIXIV_UGOIRA_FIFO_ROOT", root.path())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped()),
            std::time::Duration::from_secs(35),
        )
        .await
        .unwrap();
        assert!(
            !outcome.timed_out && outcome.output.status.success(),
            "owned FIFO child timed_out={}: {}\n{}",
            outcome.timed_out,
            String::from_utf8_lossy(&outcome.output.stdout),
            String::from_utf8_lossy(&outcome.output.stderr)
        );
        return;
    }
    use std::{
        ffi::CString,
        os::unix::{ffi::OsStrExt, fs::OpenOptionsExt},
        time::Duration,
    };
    let directory =
        tempfile::tempdir_in(std::env::var_os("PIXIV_UGOIRA_FIFO_ROOT").unwrap()).unwrap();
    let fifo = directory.path().join("blocking-owned.zip");
    let fifo_c = CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) }, 0);
    let first_out = directory.path().join("first.gif");
    let second_out = directory.path().join("second.gif");
    fs::write(&first_out, b"first-old").unwrap();
    fs::write(&second_out, b"second-old").unwrap();
    let first_context = Context::new();
    let first_task_context = first_context.clone();
    let first_path = first_out.clone();
    let first_zip = fifo.clone();
    let work = directory.path().to_path_buf();
    let mut first = tokio::spawn(async move {
        NativeAnimationEncoder
            .encode(
                first_task_context,
                EncoderInput {
                    zip_path: first_zip,
                    output_path: first_path,
                    work_dir: work,
                    frames: Some(vec![UgoiraFrame {
                        filename: "first.png".into(),
                        delay_milliseconds: 80,
                    }]),
                    format: "gif".into(),
                    max_edge: 0,
                },
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while temporary_sinks(directory.path()).len() != 1 {
            tokio::task::yield_now().await
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !native_encoder_process::fifo_reader_is_blocked().unwrap() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the owned native worker must reach its blocking FIFO open");
    let second_context = Context::new();
    let second_task_context = second_context.clone();
    let second_path = second_out.clone();
    let second_zip = directory.path().join("never-opened.zip");
    let work = directory.path().to_path_buf();
    let second = tokio::spawn(async move {
        NativeAnimationEncoder
            .encode(
                second_task_context,
                EncoderInput {
                    zip_path: second_zip,
                    output_path: second_path,
                    work_dir: work,
                    frames: Some(vec![UgoiraFrame {
                        filename: "first.png".into(),
                        delay_milliseconds: 80,
                    }]),
                    format: "gif".into(),
                    max_edge: 0,
                },
            )
            .await
    });
    let second_staged = tokio::time::timeout(Duration::from_secs(5), async {
        while temporary_sinks(directory.path()).len() != 2 {
            tokio::task::yield_now().await
        }
    })
    .await;
    second_context.cancel();
    let second_result = tokio::time::timeout(Duration::from_secs(5), second).await;
    first_context.cancel();
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut first)
            .await
            .is_err(),
        "cancellation must await the genuine blocked native worker"
    );
    assert_eq!(temporary_sinks(directory.path()).len(), 1);
    assert_eq!(fs::read(&first_out).unwrap(), b"first-old");
    let writer = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match fs::OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&fifo)
            {
                Ok(writer) => break writer,
                Err(error) if error.raw_os_error() == Some(libc::ENXIO) => {
                    tokio::task::yield_now().await
                }
                Err(error) => panic!("FIFO native-reader handshake: {error}"),
            }
        }
    })
    .await
    .unwrap();
    drop(writer);
    let first_result = tokio::time::timeout(Duration::from_secs(5), first)
        .await
        .unwrap()
        .unwrap();
    assert!(
        second_staged.is_ok(),
        "second encoder must stage before waiting on the native gate"
    );
    assert_eq!(
        second_result.unwrap().unwrap().unwrap_err().to_string(),
        "context canceled"
    );
    let first_error = first_result.unwrap_err().to_string();
    assert!(
        first_error.starts_with("rust ugoira encoder failed: "),
        "{first_error}"
    );
    assert!(
        first_error.ends_with("\ncontext canceled"),
        "native error and context must be joined: {first_error}"
    );
    assert_eq!(fs::read(first_out).unwrap(), b"first-old");
    assert_eq!(fs::read(second_out).unwrap(), b"second-old");
    assert!(temporary_sinks(directory.path()).is_empty());
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "current_thread")]
async fn dropping_a_caller_cleans_its_waiting_output_and_preserves_parent_reuse() {
    use std::{
        ffi::CString,
        os::unix::{ffi::OsStrExt, fs::OpenOptionsExt},
        time::Duration,
    };

    if std::env::var_os("PIXIV_UGOIRA_FIFO_DROP_CHILD").is_none() {
        let root = tempfile::tempdir().unwrap();
        let outcome = native_encoder_process::run(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "dropping_a_caller_cleans_its_waiting_output_and_preserves_parent_reuse",
                    "--nocapture",
                ])
                .env("PIXIV_UGOIRA_FIFO_DROP_CHILD", "1")
                .env("PIXIV_UGOIRA_FIFO_ROOT", root.path())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped()),
            Duration::from_secs(35),
        )
        .await
        .unwrap();
        assert!(
            !outcome.timed_out && outcome.output.status.success(),
            "owned FIFO child timed_out={}: {}\n{}",
            outcome.timed_out,
            String::from_utf8_lossy(&outcome.output.stdout),
            String::from_utf8_lossy(&outcome.output.stderr)
        );
        return;
    }

    let case = fixture()
        .cases
        .into_iter()
        .find(|case| case.name == "gif")
        .unwrap();
    let directory =
        tempfile::tempdir_in(std::env::var_os("PIXIV_UGOIRA_FIFO_ROOT").unwrap()).unwrap();
    let fifo = directory.path().join("blocking-owned.zip");
    let fifo_c = CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) }, 0);
    let archive = directory.path().join("valid-owned.zip");
    fs::write(&archive, hex_bytes(&case.zip_hex)).unwrap();
    let first_output = directory.path().join("first.gif");
    let abandoned_output = directory.path().join("abandoned.gif");
    let reused_output = directory.path().join("reused.gif");
    fs::write(&first_output, b"first-old").unwrap();
    fs::write(&abandoned_output, b"abandoned-old").unwrap();
    let input = |zip_path, output_path| EncoderInput {
        zip_path,
        output_path,
        work_dir: directory.path().to_path_buf(),
        frames: case.frames.as_ref().map(|frames| {
            frames
                .iter()
                .map(|frame| UgoiraFrame {
                    filename: frame.file.clone(),
                    delay_milliseconds: frame.delay,
                })
                .collect()
        }),
        format: "gif".into(),
        max_edge: 0,
    };
    let parent = Context::new();
    let context = parent.clone();
    let first_input = input(fifo.clone(), first_output.clone());
    let first =
        tokio::spawn(async move { NativeAnimationEncoder.encode(context, first_input).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while !native_encoder_process::fifo_reader_is_blocked().unwrap() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let context = parent.clone();
    let abandoned_input = input(archive.clone(), abandoned_output.clone());
    let abandoned = tokio::spawn(async move {
        NativeAnimationEncoder
            .encode(context, abandoned_input)
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while temporary_sinks(directory.path()).len() != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    abandoned.abort();
    assert!(abandoned.await.unwrap_err().is_cancelled());
    let abandoned_cleanup = tokio::time::timeout(Duration::from_secs(1), async {
        while temporary_sinks(directory.path()).len() != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await;
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    let context = parent.clone();
    let reused_input = input(archive, reused_output.clone());
    let mut reused =
        tokio::spawn(async move { NativeAnimationEncoder.encode(context, reused_input).await });
    let completed_while_native_blocked =
        tokio::time::timeout(Duration::from_millis(50), &mut reused).await;
    let writer = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match fs::OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&fifo)
            {
                Ok(writer) => break writer,
                Err(error) if error.raw_os_error() == Some(libc::ENXIO) => {
                    tokio::task::yield_now().await;
                }
                Err(error) => panic!("FIFO native-reader handshake: {error}"),
            }
        }
    })
    .await
    .unwrap();
    drop(writer);
    if completed_while_native_blocked.is_err() {
        tokio::time::timeout(Duration::from_secs(5), reused)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        while !temporary_sinks(directory.path()).is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        abandoned_cleanup.is_ok(),
        "the dropped caller must clean its waiting staging file"
    );
    assert!(
        completed_while_native_blocked.is_err(),
        "the abandoned native worker must retain the gate until completion"
    );
    assert_eq!(
        parent.error(),
        None,
        "caller drop must not cancel the parent"
    );
    assert_eq!(fs::read(first_output).unwrap(), b"first-old");
    assert_eq!(fs::read(abandoned_output).unwrap(), b"abandoned-old");
    assert_eq!(
        hash(&fs::read(reused_output).unwrap()),
        case.output_sha256.unwrap()
    );
}
