use std::io::{self, Write};
use std::sync::OnceLock;

const LAYOUT: &str =
    include_str!("../../../internal/cli/commands/pixiv/auth/loginpage/templates/layout.html");
const MANUAL: &str =
    include_str!("../../../internal/cli/commands/pixiv/auth/loginpage/templates/manual.html");
const CALLBACK: &str = include_str!(
    "../../../internal/cli/commands/pixiv/auth/loginpage/templates/callback-relay.html"
);
const RESULT: &str =
    include_str!("../../../internal/cli/commands/pixiv/auth/loginpage/templates/result.html");
const STYLESHEET: &str =
    include_str!("../../../internal/cli/commands/pixiv/auth/loginpage/assets/page.css");

pub fn write_manual(w: &mut (impl Write + ?Sized), login_url: &str) -> io::Result<()> {
    write_layout(w, "pv-form-page", |w| {
        let (before, after) = content(MANUAL)
            .split_once("{{.LoginURL}}")
            .expect("embedded manual URL");
        write_part(w, before)?;
        write_part(w, &href(login_url))?;
        write_part(w, after)
    })
}

pub fn write_callback_relay(w: &mut (impl Write + ?Sized)) -> io::Result<()> {
    static PAGE: OnceLock<Vec<u8>> = OnceLock::new();
    let page = PAGE.get_or_init(|| {
        let mut out = Vec::new();
        write_layout(&mut out, "pv-bridge", |w| write_part(w, content(CALLBACK)))
            .expect("embedded callback page");
        out
    });
    w.write(page).map(|_| ())
}

pub fn write_result(w: &mut (impl Write + ?Sized), success: bool) -> io::Result<()> {
    static SUCCESS: OnceLock<Vec<u8>> = OnceLock::new();
    static FAILURE: OnceLock<Vec<u8>> = OnceLock::new();
    let page = if success { &SUCCESS } else { &FAILURE }.get_or_init(|| {
        let mut out = Vec::new();
        write_layout(&mut out, if success { "pv-final" } else { "pv-final pv-bad" }, |w| {
            let (before, branches) = content(RESULT).split_once("{{if .Success}}").expect("embedded result condition");
            let (yes, branches) = branches.split_once("{{else}}").expect("embedded result branch");
            let (no, after) = branches.split_once("{{end}}").expect("embedded result end");
            write_part(w, before)?;
            write_part(w, if success { yes } else { no })?;
            let (before, after) = after.split_once("{{.Heading}}").expect("embedded result heading");
            write_part(w, before)?;
            write_part(w, if success { "Login successful" } else { "Login failed" })?;
            let (before, after) = after.split_once("{{.Message}}").expect("embedded result message");
            write_part(w, before)?;
            write_part(w, if success { "Login completed. You can close this page and return to the terminal." } else { "Login could not be completed. Return to the terminal to view details or try again." })?;
            write_part(w, after)
        }).expect("embedded result page");
        out
    });
    w.write(page).map(|_| ())
}

fn content(template: &str) -> &str {
    template
        .strip_prefix("{{define \"content\"}}")
        .expect("embedded content definition")
        .strip_suffix("{{end}}\n")
        .expect("embedded content end")
}

fn write_part(w: &mut (impl Write + ?Sized), text: &str) -> io::Result<()> {
    // Go template execution accepts short writes without an accompanying error.
    w.write(text.as_bytes()).map(|_| ())
}

fn write_layout<W: Write + ?Sized>(
    w: &mut W,
    body_class: &str,
    render_content: impl FnOnce(&mut W) -> io::Result<()>,
) -> io::Result<()> {
    let (before, after) = LAYOUT
        .split_once("{{.Title}}")
        .expect("embedded page title");
    write_part(w, before)?;
    write_part(w, "pixiv-cli")?;
    let (before, after) = after
        .split_once("{{.Stylesheet}}")
        .expect("embedded stylesheet");
    write_part(w, before)?;
    write_part(w, STYLESHEET)?;
    let (before, after) = after
        .split_once("{{.BodyClass}}")
        .expect("embedded body class");
    write_part(w, before)?;
    write_part(w, body_class)?;
    let (before, after) = after
        .split_once("{{template \"content\" .}}")
        .expect("embedded page content");
    write_part(w, before)?;
    render_content(w)?;
    write_part(w, after)
}

fn href(url: &str) -> String {
    if let Some((protocol, _)) = url.split_once(':')
        && !protocol.contains('/')
        && !["http", "https", "mailto"]
            .iter()
            .any(|allowed| scheme_eq(protocol, allowed))
    {
        return "#ZgotmplZ".into();
    }
    let bytes = url.as_bytes();
    let mut out = String::with_capacity(bytes.len());
    const HEX: &[u8] = b"0123456789abcdef";
    for (i, &byte) in bytes.iter().enumerate() {
        match byte {
            b'&' => out.push_str("&amp;"),
            b'+' => out.push_str("&#43;"),
            b'%' if bytes.get(i + 1).is_some_and(u8::is_ascii_hexdigit)
                && bytes.get(i + 2).is_some_and(u8::is_ascii_hexdigit) =>
            {
                out.push('%')
            }
            b'!' | b'#' | b'$' | b'*' | b',' | b'/' | b':' | b';' | b'=' | b'?' | b'@' | b'['
            | b']' | b'-' | b'.' | b'_' | b'~' => out.push(char::from(byte)),
            byte if byte.is_ascii_alphanumeric() => out.push(char::from(byte)),
            byte => {
                out.push('%');
                out.push(char::from(HEX[usize::from(byte >> 4)]));
                out.push(char::from(HEX[usize::from(byte & 15)]));
            }
        }
    }
    out
}

fn scheme_eq(protocol: &str, allowed: &str) -> bool {
    let mut chars = protocol.chars();
    allowed.chars().all(|expected| {
        chars.next().is_some_and(|actual| {
            actual.eq_ignore_ascii_case(&expected) || (expected == 's' && actual == 'ſ')
        })
    }) && chars.next().is_none()
}
