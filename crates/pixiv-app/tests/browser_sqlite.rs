#[path = "support/browser_sqlite.rs"]
mod support;

use pixiv_app::{
    browser_cookies::BrowserCookieError,
    browser_sqlite::SqliteCommand,
    lifecycle::{Context, ContextError},
};
use std::{collections::BTreeMap, path::Path, sync::Arc};
use support::{RecordedHost, fixture, normalized_command, rows_hex};

#[test]
fn every_frozen_case_has_an_explicit_process_or_official_shell_boundary() {
    use sha2::{Digest, Sha256};
    let fixture = fixture();
    assert_eq!(
        fixture.reference,
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    assert_eq!(fixture.cases.len(), 42);
    assert_eq!(
        fixture.sources["internal/browsercookies/sqliteio/sqliteio.go"],
        "d922ed106d3a70cd4c0fa79297edb57cbe9adff051816aa3cc8218aaf4b2c902"
    );
    assert_eq!(
        support::encode_hex(&Sha256::digest(include_bytes!(
            "../../../internal/browsercookies/sqliteio/sqliteio.go"
        ))),
        fixture.sources["internal/browsercookies/sqliteio/sqliteio.go"]
    );
    let mut names = std::collections::BTreeSet::new();
    let mut counts = [0; 2];
    for case in fixture.cases {
        assert!(names.insert(case.name));
        match case.operation.as_str() {
            "query" => counts[0] += 1,
            "query_sqlite" => {
                assert_eq!(case.input.boundary, "official_sqlite_shell");
                counts[1] += 1;
            }
            other => panic!("unclassified fixture operation: {other}"),
        }
    }
    assert_eq!(counts, [33, 9]);
}

#[test]
fn source_driven_windows_process_cases_match_all_33_frozen_helper_contracts() {
    let root = "/owned-synthetic-root";
    for case in fixture()
        .cases
        .into_iter()
        .filter(|case| case.operation == "query")
    {
        let context = Context::new();
        if case.input.pre_cancel {
            context.cancel();
        }
        let host = Arc::new(RecordedHost::new(case.input.clone()));
        let path = case.input.db_path.replace("$root", root);
        let result = SqliteCommand::new(host.clone()).query(
            &context,
            Path::new(&path),
            &case.input.sql,
            &case.input.params.clone().unwrap_or_default(),
        );
        match result {
            Ok(rows) => {
                assert_eq!(case.output.error, "", "{}", case.name);
                assert_eq!(rows_hex(&rows), case.output.rows_hex, "{}", case.name);
            }
            Err(error) => {
                assert_eq!(error.to_string(), case.output.error, "{}", case.name);
                assert_eq!(case.output.rows_hex, None, "{}", case.name);
            }
        }
        let calls = host.calls.lock().unwrap();
        let command = calls.first().map(|args| normalized_command(args, root));
        assert!(calls.len() <= 1, "{}", case.name);
        assert_eq!(command, case.output.command, "{}", case.name);
    }
}

#[test]
fn context_only_overrides_process_failures_and_validation_stays_first() {
    let context = Context::new();
    let mut host = RecordedHost::new(support::Input {
        stdout_hex: "76616c75650a".into(),
        ..Default::default()
    });
    host.cancel_success = true;
    let command = SqliteCommand::new(Arc::new(host));
    let rows = command
        .query(
            &context,
            Path::new("owned.db"),
            "SELECT 1;",
            &BTreeMap::new(),
        )
        .unwrap();
    assert_eq!(rows, vec![vec![b"value".to_vec()]]);
    assert_eq!(context.error(), Some(ContextError::Canceled));

    let missing = Arc::new(RecordedHost::new(support::Input {
        missing_command: true,
        ..Default::default()
    }));
    let command = SqliteCommand::new(missing);
    assert_eq!(
        command
            .query(
                &context,
                Path::new("owned.db"),
                "SELECT 1;",
                &BTreeMap::new()
            )
            .unwrap_err(),
        BrowserCookieError::Context(ContextError::Canceled),
    );
    assert_eq!(
        command
            .query(
                &context,
                Path::new("\u{0085}\u{3000}"),
                "SELECT 1;",
                &BTreeMap::new()
            )
            .unwrap_err(),
        BrowserCookieError::QueryFailed,
    );
}

#[test]
fn operating_system_launch_permissions_do_not_invent_sqlite_stderr() {
    let mut host = RecordedHost::new(Default::default());
    host.spawn_failure = true;
    let error = SqliteCommand::new(Arc::new(host))
        .query(
            &Context::new(),
            Path::new("owned-secret-path.db"),
            "SELECT synthetic_secret;",
            &BTreeMap::new(),
        )
        .unwrap_err();
    assert_eq!(error, BrowserCookieError::QueryFailed);
    assert_eq!(
        error.to_string(),
        "browsercookies: cookie database query failed"
    );
}

#[test]
fn elapsed_deadline_overrides_lookup_failure_without_exposing_process_details() {
    use std::time::{Duration, Instant};
    let host = Arc::new(RecordedHost::new(support::Input {
        missing_command: true,
        ..Default::default()
    }));
    let context = Context::with_deadline(Instant::now() - Duration::from_secs(1));
    let error = SqliteCommand::new(host)
        .query(
            &context,
            Path::new("owned-secret-path.db"),
            "SELECT synthetic_secret;",
            &BTreeMap::new(),
        )
        .unwrap_err();
    assert_eq!(
        error,
        BrowserCookieError::Context(ContextError::DeadlineExceeded)
    );
    assert_eq!(error.to_string(), "context deadline exceeded");
}

#[test]
fn csv_eof_cr_normalization_and_strict_quotes_use_byte_fields() {
    for (bytes, expected) in [
        (b"tail\r".as_slice(), Some(vec![vec![b"tail".to_vec()]])),
        (b"\r".as_slice(), Some(Vec::new())),
        (b"\"tail\"\r".as_slice(), Some(vec![vec![b"tail".to_vec()]])),
        (b"\"\n\n\"\n".as_slice(), Some(vec![vec![b"\n\n".to_vec()]])),
        (
            b"a,".as_slice(),
            Some(vec![vec![b"a".to_vec(), Vec::new()]]),
        ),
        (b"\"a\" \n".as_slice(), None),
        (b"ok\n\"bad".as_slice(), None),
    ] {
        let host = Arc::new(RecordedHost::new(support::Input {
            stdout_hex: support::encode_hex(bytes),
            ..Default::default()
        }));
        let result = SqliteCommand::new(host).query(
            &Context::new(),
            Path::new("owned.db"),
            "SELECT 1;",
            &BTreeMap::new(),
        );
        match expected {
            Some(rows) => assert_eq!(result.unwrap(), rows, "{bytes:?}"),
            None => assert_eq!(
                result.unwrap_err(),
                BrowserCookieError::QueryFailed,
                "{bytes:?}"
            ),
        }
    }
}

#[cfg(unix)]
#[test]
fn actual_owned_helper_processes_replay_all_33_argv_csv_error_and_cancellation_cases() {
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        thread,
        time::{Duration, Instant},
    };
    for case in fixture()
        .cases
        .into_iter()
        .filter(|case| case.operation == "query")
    {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("argv");
        let helper = directory.path().join("sqlite3");
        let octals = |text: &str| {
            support::decode_hex(text)
                .into_iter()
                .map(|byte| format!("\\{byte:03o}"))
                .collect::<String>()
        };
        let script = format!(
            "#!/bin/sh\nif read value; then exit 90; fi\nprintf '%s\\0' \"$@\" > '{}'\n{}\nprintf '{}'\nprintf '{}' >&2\nexit {}\n",
            marker.to_str().unwrap().replace('\'', "'\\''"),
            if case.input.active_cancel {
                "while :; do :; done"
            } else {
                ":"
            },
            octals(&case.input.stdout_hex),
            octals(&case.input.stderr_hex),
            case.input.exit,
        );
        fs::write(&helper, script).unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
        let context = Context::with_deadline(Instant::now() + Duration::from_secs(10));
        if case.input.pre_cancel {
            context.cancel();
        }
        let canceler = if case.input.active_cancel {
            let context = context.clone();
            let marker = marker.clone();
            Some(thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(5);
                while !marker.exists() && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(1));
                }
                let started = marker.exists();
                context.cancel();
                started
            }))
        } else {
            None
        };
        let host = support::PinnedHost(if case.input.missing_command {
            directory.path().join("absent-sqlite3")
        } else {
            helper
        });
        let root = directory.path().to_str().unwrap();
        let path = case.input.db_path.replace("$root", root);
        let result = SqliteCommand::new(Arc::new(host)).query(
            &context,
            Path::new(&path),
            &case.input.sql,
            &case.input.params.unwrap_or_default(),
        );
        if let Some(canceler) = canceler {
            assert!(canceler.join().unwrap(), "{}", case.name);
        }
        match result {
            Ok(rows) => {
                assert_eq!(case.output.error, "", "{}", case.name);
                assert_eq!(rows_hex(&rows), case.output.rows_hex, "{}", case.name);
            }
            Err(error) => assert_eq!(error.to_string(), case.output.error, "{}", case.name),
        }
        let command = fs::read(&marker).ok().map(|bytes| {
            let args: Vec<_> = bytes
                .strip_suffix(&[0])
                .unwrap()
                .split(|byte| *byte == 0)
                .map(|arg| std::ffi::OsString::from(std::str::from_utf8(arg).unwrap()))
                .collect();
            normalized_command(&args, root)
        });
        assert_eq!(command, case.output.command, "{}", case.name);
    }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
fn official_sqlite_shell_replays_all_nine_cases_on_genuine_owned_databases() {
    use pixiv_app::host_process::{HostProcess, ProcessStdio, SystemHostProcess};
    use sha2::{Digest, Sha256};
    use std::{ffi::OsString, fs, path::PathBuf};
    let fixture = fixture();
    let executable = std::env::var_os("PIXIV_MIGRATION_SQLITE3")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from("/workspace/scratch/cd334a12dde3/sqlite-toolchain/sqlite3")
        });
    let binary = fs::read(&executable).expect("the pinned official SQLite shell is required");
    assert_eq!(
        support::encode_hex(&Sha256::digest(binary)),
        fixture.sqlite_shell["sha256"]
    );
    let version = SystemHostProcess
        .run(
            executable.as_os_str(),
            &[OsString::from("-version")],
            ProcessStdio::Capture,
        )
        .unwrap();
    assert_eq!(
        std::str::from_utf8(&version.stdout).unwrap().trim(),
        fixture.sqlite_shell["version"]
    );
    let command = SqliteCommand::new(Arc::new(support::PinnedHost(executable)));
    let mut compared = 0;
    for case in fixture
        .cases
        .into_iter()
        .filter(|case| case.operation == "query_sqlite")
    {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("owned.db");
        let mut locked = None;
        match case.input.database_state.as_str() {
            "invalid" => fs::write(&path, b"owned invalid database").unwrap(),
            "missing" => {}
            "" => {
                let database = rusqlite::Connection::open(&path).unwrap();
                database.execute_batch(&case.input.schema).unwrap();
                for sql in case.input.statements.as_deref().unwrap_or_default() {
                    database.execute_batch(sql).unwrap();
                }
                if case.input.exclusive_lock {
                    locked = Some(database);
                } else {
                    database.close().unwrap();
                }
            }
            state => panic!("unsupported owned database state: {state}"),
        }
        // Closing another database descriptor would release this process's POSIX record locks.
        let before = fs::read(&path).ok();
        if let Some(database) = &locked {
            database.execute_batch("BEGIN EXCLUSIVE;").unwrap();
        }
        let result = command.query(
            &Context::new(),
            &path,
            &case.input.sql,
            &case.input.params.unwrap_or_default(),
        );
        match result {
            Ok(rows) => {
                assert_eq!(case.output.error, "", "{}", case.name);
                assert_eq!(rows_hex(&rows), case.output.rows_hex, "{}", case.name);
            }
            Err(error) => assert_eq!(error.to_string(), case.output.error, "{}", case.name),
        }
        assert_eq!(
            fs::read(&path).ok(),
            before,
            "{} changed owned database",
            case.name
        );
        if let Some(database) = locked {
            database.execute_batch("ROLLBACK;").unwrap();
            database.close().unwrap();
        }
        compared += 1;
    }
    assert_eq!(compared, 9);
}
