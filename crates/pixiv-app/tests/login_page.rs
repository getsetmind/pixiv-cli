use pixiv_app::login_page::{write_callback_relay, write_manual, write_result};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::{self, Write};

#[derive(Deserialize)]
struct Contract {
    name: String,
    url: String,
    href: String,
    sha256: String,
    writes: Vec<usize>,
}
#[derive(Default)]
struct Capture {
    bytes: Vec<u8>,
    writes: Vec<usize>,
}
impl Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.writes.push(bytes.len());
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn render(w: &mut impl Write, name: &str, url: &str) -> io::Result<()> {
    match name {
        "callback" => write_callback_relay(w),
        "success" => write_result(w, true),
        "failure" => write_result(w, false),
        _ => write_manual(w, url),
    }
}
#[test]
fn embedded_pages_and_href_context_match_frozen_go_bytes_and_writes() {
    let contracts: Vec<Contract> =
        serde_json::from_str(include_str!("fixtures/login_page.json")).unwrap();
    for contract in contracts {
        let mut capture = Capture::default();
        render(&mut capture, &contract.name, &contract.url).unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(&capture.bytes)),
            contract.sha256,
            "{} {:?}",
            contract.name,
            contract.url
        );
        assert_eq!(
            capture.writes, contract.writes,
            "{} {:?}",
            contract.name, contract.url
        );
        if contract.name == "manual" {
            let body = String::from_utf8(capture.bytes).unwrap();
            let (_, tail) = body.split_once("id=\"pixiv-login-link\" href=\"").unwrap();
            assert_eq!(tail.split_once('"').unwrap().0, contract.href);
        }
    }
}
struct FailingWriter;
impl Write for FailingWriter {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "page writer failed",
        ))
    }
    fn flush(&mut self) -> io::Result<()> {
        panic!("render must not flush")
    }
}
struct ShortWriter;
impl Write for ShortWriter {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Ok(0)
    }
    fn flush(&mut self) -> io::Result<()> {
        panic!("render must not flush")
    }
}
#[test]
fn page_writers_propagate_errors_and_preserve_go_short_write_behavior() {
    for name in ["manual", "callback", "success", "failure"] {
        let error = render(&mut FailingWriter, name, "https://example.test/").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(error.to_string(), "page writer failed");
        render(&mut ShortWriter, name, "https://example.test/").unwrap();
    }
}
#[test]
fn relay_clears_fragment_before_submission_and_result_failure_is_generic() {
    let mut page = Vec::new();
    write_callback_relay(&mut page).unwrap();
    let body = String::from_utf8(page).unwrap();
    assert!(
        body.find("window.history.replaceState").unwrap()
            < body.find("if (!completionURL)").unwrap()
    );
    for required in [
        "window.location.hash.slice(1)",
        "form.method = \"post\"",
        "form.action = \"/manual\"",
        "input.name = \"login_result\"",
        "input.value = completionURL",
        "form.submit()",
    ] {
        assert!(body.contains(required));
    }
    let mut page = Vec::new();
    write_result(&mut page, false).unwrap();
    let body = String::from_utf8(page).unwrap().to_lowercase();
    for forbidden in ["token", "refresh", "bearer", "code=", "password", "secret"] {
        assert!(!body.contains(forbidden));
    }
}

struct PartialFailure {
    calls: usize,
    fail_at: usize,
}
impl Write for PartialFailure {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.calls += 1;
        if self.calls == self.fail_at {
            Err(io::Error::other(PageWriteFailure))
        } else {
            Ok(bytes.len())
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        panic!("render must not flush")
    }
}
#[derive(Deserialize)]
struct WriterErrorContract {
    name: String,
    write: usize,
    error: String,
}
#[test]
fn every_go_write_boundary_preserves_original_error_display_and_stops() {
    let contracts: Vec<WriterErrorContract> =
        serde_json::from_str(include_str!("fixtures/login_page_writer_errors.json")).unwrap();
    for contract in contracts {
        let mut writer = PartialFailure {
            calls: 0,
            fail_at: contract.write,
        };
        let error = render(
            &mut writer,
            &contract.name,
            "https://example.test/?x=a+b&y=c",
        )
        .unwrap_err();
        assert_eq!(error.to_string(), contract.error);
        assert_eq!(writer.calls, contract.write);
        assert!(
            error
                .get_ref()
                .unwrap()
                .downcast_ref::<PageWriteFailure>()
                .is_some()
        );
    }
}
#[derive(Debug)]
struct PageWriteFailure;
impl std::fmt::Display for PageWriteFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("partial page write failed")
    }
}
impl std::error::Error for PageWriteFailure {}
