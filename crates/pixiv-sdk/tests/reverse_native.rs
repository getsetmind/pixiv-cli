use pixiv_sdk::fanbox::native::{HeaderPolicy, NativeOptions, NativeTransport};
use sha2::{Digest, Sha256};

const NATIVE: &str = include_str!("../src/fanbox/native.rs");
const ASCII2D_GO: &str =
    include_str!("../../../internal/services/reversesearch/ascii2d/transport.go");

#[test]
fn ascii2d_can_select_the_existing_native_transport_without_network_io() {
    let transport = NativeTransport::with_options(NativeOptions {
        proxy_url: String::new(),
        header_policy: HeaderPolicy::Ascii2d,
    });
    assert!(transport.is_ok());
    assert!(NativeTransport::new("").is_ok());
    assert_eq!(NativeOptions::default().header_policy, HeaderPolicy::Fanbox);
}

#[test]
fn both_native_policies_reject_an_invalid_proxy_without_network_io() {
    for header_policy in [HeaderPolicy::Fanbox, HeaderPolicy::Ascii2d] {
        assert!(
            NativeTransport::with_options(NativeOptions {
                proxy_url: "http://[".into(),
                header_policy,
            })
            .is_err()
        );
    }
}

#[test]
fn native_options_debug_omits_the_proxy_url_and_credentials() {
    let options = NativeOptions {
        proxy_url: "http://synthetic-user:synthetic-password@proxy.invalid:3128".into(),
        header_policy: HeaderPolicy::Ascii2d,
    };
    let debug = format!("{options:?}");
    assert!(debug.contains("Ascii2d"));
    for private in ["synthetic-user", "synthetic-password", "proxy.invalid"] {
        assert!(!debug.contains(private));
    }
}

#[test]
fn ascii2d_header_order_is_source_anchored_to_the_sealed_go_policy() {
    assert_source_digest(
        ASCII2D_GO,
        "a489d18b93012f2a1ef186bb570a0d295197919f1557c90aa2384cfb267ddfa5",
    );
    let policy = function_body(NATIVE, "fn ascii2d_header_order(");
    assert_eq!(
        scoped_header_literals(policy),
        scoped_header_literals(function_body(ASCII2D_GO, "func browserHeaderOrder(")),
        "header names, order and conditional nesting are source evidence only"
    );
    assert_eq!(policy.matches("if ").count(), 6);
    assert_eq!(policy.matches("if header_nonempty(headers, ").count(), 6);
    assert_eq!(
        words(function_body(NATIVE, "fn header_nonempty(")),
        "headers .get(name) .is_some_and(|value| !value.as_bytes().is_empty())"
    );
    let policy_selection = between(
        NATIVE,
        "let mut order = match",
        "for name in headers.keys()",
    );
    assert!(policy_selection.contains("self.header_policy"));
    assert!(policy_selection.contains("HeaderPolicy::Fanbox => fanbox_header_order(&headers)"));
    assert!(policy_selection.contains("HeaderPolicy::Ascii2d => ascii2d_header_order(&headers)"));
}

#[test]
fn native_reuse_preserves_the_reviewed_profile_verification_and_owned_lifecycle_sources() {
    for (source, digest) in [
        (
            include_str!("../src/fanbox/profile.rs"),
            "ba4fad8edaa90f2dfc5d0c1e04679f8218c6570ec797ba0cae7f3ead58724295",
        ),
        (
            include_str!("../src/fanbox/decoded_body.rs"),
            "1c496a673c8226da19be1f8c0017428c63f9cba304a8b31cbcc4acde2f620047",
        ),
    ] {
        assert_source_digest(source, digest);
    }
    for (source, digest) in [
        (
            between(NATIVE, "let roots =", "Ok(Self {"),
            "822c73b8854d4ecf00d039497817c15cb23f7b541982dc64fc14686c57d8e7ce",
        ),
        (
            between(NATIVE, "async fn execute(", "let mut order = match"),
            "98b7d2356c1d7db37443e0edfc2984122a2c5a4827fd97159123df7363d96035",
        ),
        (
            between(
                NATIVE,
                "pub async fn close_idle_connections(",
                "async fn execute(",
            ),
            "156bfb7a5ac89065a5b73169007819d46671c9069db4acb578450f700038fdc9",
        ),
        (
            between(NATIVE, "for name in headers.keys() {", "type BodyStream ="),
            "8cf5e715bfa91fe998a8618b0a0ef4a85ff57540fe0791bab72b17c7be201694",
        ),
        (
            between(NATIVE, "type BodyStream =", "fn fanbox_header_order("),
            "58043fe94841734c5a51cf65ea3213afdd8095392eb527a1fcf10de3e71bf514",
        ),
        (
            function_body(NATIVE, "fn fanbox_header_order(")
                .trim_end()
                .strip_suffix("order")
                .unwrap(),
            "edb419c499bf0b677c80f6f2d8936cb3d7e3d80e3395822e636dd78cd0d6cffb",
        ),
    ] {
        assert_source_digest(&words(source), digest);
    }
    let default_constructor = between(NATIVE, "pub fn new(", "pub fn with_options(");
    assert!(default_constructor.contains("Self::with_options(NativeOptions"));
    assert!(default_constructor.contains("header_policy: HeaderPolicy::Fanbox"));
}

fn assert_source_digest(source: &str, expected: &str) {
    let source = source.replace("\r\n", "\n");
    assert_eq!(format!("{:x}", Sha256::digest(source.as_bytes())), expected);
}

fn words(source: &str) -> String {
    source.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn between<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let start = source.find(start).expect("source start anchor");
    let end = source[start..].find(end).expect("source end anchor") + start;
    &source[start..end]
}

fn function_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let tail = source.split_once(signature).expect("source signature").1;
    let body = tail.split_once('{').expect("source function body").1;
    body.split_once("\n}").expect("source function end").0
}

fn scoped_header_literals(source: &str) -> Vec<(String, usize)> {
    let mut literals = Vec::new();
    let mut depth = 0_usize;
    let mut quote = None;
    for (offset, byte) in source.bytes().enumerate() {
        match (byte, quote) {
            (b'"', Some(start)) => {
                if start != offset {
                    literals.push((source[start..offset].to_ascii_lowercase(), depth));
                }
                quote = None;
            }
            (b'"', None) => quote = Some(offset + 1),
            (b'{', None) => depth += 1,
            (b'}', None) => depth = depth.checked_sub(1).expect("source nesting"),
            _ => {}
        }
    }
    assert!(quote.is_none());
    assert_eq!(depth, 0);
    literals
}
