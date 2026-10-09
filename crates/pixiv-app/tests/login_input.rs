use pixiv_app::login_input::{
    LoginAddressParseError, classify_login_input, is_browser_callback_url, login_code_from_input,
    login_input_from_text, login_ssh_tunnel_command, pixiv_auth_start_matches_challenge,
    pixiv_login_challenge, pixiv_post_redirect_return_to, validate_login_addr,
};
use std::{cell::Cell, error::Error};

#[test]
fn cli_input_preserves_raw_codes_url_errors_and_accepter_order() {
    let cases = [
        (" \t\n", "", "sign-in result cannot be empty", 0),
        (" bare-code ", "bare-code", "", 0),
        ("code?state=bad", "code?state=bad", "", 0),
        ("code#fragment", "code#fragment", "", 0),
        ("code&field=value", "code&field=value", "", 0),
        (
            "pixiv:account/login?code=one",
            "pixiv:account/login?code=one",
            "",
            0,
        ),
        (
            "https://example.test/callback",
            "",
            "sign-in address did not include required details",
            0,
        ),
        (
            "https://example.test/callback?",
            "",
            "sign-in address did not include required details",
            0,
        ),
        (
            "https://example.test/callback#?code=one",
            "",
            "sign-in address did not include required details",
            0,
        ),
        (
            "https://example.test/callback?code=%zz",
            "https://example.test/callback?code=%zz",
            "",
            1,
        ),
        ("/callback?code=one", "/callback?code=one", "", 1),
        (
            "//example.test/callback?code=one",
            "//example.test/callback?code=one",
            "",
            1,
        ),
        (
            "https://example.test/%zz?code=one",
            "",
            "invalid sign-in address",
            0,
        ),
        (
            "https://example.test/callback?code=one#%zz",
            "",
            "invalid sign-in address",
            0,
        ),
        (
            "https://example.test/callback?code=one\0",
            "",
            "invalid sign-in address",
            0,
        ),
    ];
    for (input, code, error, calls) in cases {
        let called = Cell::new(0);
        let accepter = |raw: &str| {
            called.set(called.get() + 1);
            assert_eq!(raw, input.trim());
            true
        };
        let result = login_code_from_input(input, Some(&accepter));
        assert_eq!(result.code, code, "{input:?}");
        assert_eq!(
            result
                .error
                .as_ref()
                .map(ToString::to_string)
                .as_deref()
                .unwrap_or_default(),
            error,
            "{input:?}"
        );
        assert_eq!(called.get(), calls, "{input:?}");
    }
    for accepter in [None, Some(&(|_: &str| false) as &dyn Fn(&str) -> bool)] {
        let result = login_code_from_input("pixiv://account/login?code=secret", accepter);
        assert_eq!(
            result.error.as_ref().map(ToString::to_string).as_deref(),
            Some("sign-in address does not match this login session")
        );
        assert!(result.code.is_empty());
    }
}

fn bridge(target: &str) -> String {
    format!(
        "https://accounts.pixiv.net/post-redirect?return_to={}",
        url::form_urlencoded::byte_serialize(target.as_bytes()).collect::<String>()
    )
}

#[test]
fn browser_relay_classification_and_terminal_opener_remain_distinct() {
    let start = "https://app-api.pixiv.net/web/v1/users/auth/pixiv/start?code_challenge=current";
    let valid = bridge(start);
    let stale = start.replace("current", "stale");
    let cases = [
        (format!(" {valid} "), start.to_owned(), "", true),
        (
            valid.replace("accounts.pixiv.net", "accountſ.pixiv.net"),
            start.to_owned(),
            "",
            true,
        ),
        (
            valid.replace("accounts.pixiv.net", "ACCOUNTS.PIXIV.NET"),
            start.to_owned(),
            "",
            true,
        ),
        (format!("{valid}&return_to=bad"), start.to_owned(), "", true),
        (
            valid.replace("return_to=", "return_to=%zz&return_to="),
            start.to_owned(),
            "",
            true,
        ),
        (
            valid.replace("return_to=", "return_to=bad&return_to="),
            String::new(),
            "invalid Pixiv authorization relay URL",
            true,
        ),
        (
            format!("{valid};ignored=yes"),
            String::new(),
            "invalid Pixiv authorization relay URL",
            true,
        ),
        (
            bridge(&stale),
            stale,
            "Pixiv authorization relay URL does not match this login attempt",
            true,
        ),
        (
            valid.replace("accounts.pixiv.net", "accounts.pixiv.net:443"),
            String::new(),
            "",
            false,
        ),
        (
            valid.replace("accounts.pixiv.net", "user@accounts.pixiv.net"),
            start.to_owned(),
            "",
            true,
        ),
    ];
    for (input, target, error, relay) in cases {
        assert_eq!(
            pixiv_post_redirect_return_to(&input).unwrap_or_default(),
            target,
            "{input}"
        );
        let accepted = Cell::new(0);
        let accepter = |_: &str| {
            accepted.set(accepted.get() + 1);
            true
        };
        let result = classify_login_input(&input, Some(&accepter), "current");
        assert_eq!(result.relayed, relay, "{input}");
        assert_eq!(
            result
                .result
                .error
                .as_ref()
                .map(ToString::to_string)
                .as_deref()
                .unwrap_or_default(),
            error,
            "{input}"
        );
        if relay {
            assert_eq!(accepted.get(), 0);
        }
        let mut opened = 0;
        let mut opener = |raw: &str| {
            opened += 1;
            assert_eq!(raw, input.trim());
            Ok(())
        };
        let result = login_input_from_text(&input, Some(&accepter), "current", Some(&mut opener));
        assert_eq!(opened, usize::from(relay && error.is_empty()));
        if relay && error.is_empty() {
            assert_eq!(result.relay_url, input.trim());
        }
    }
    let result = login_input_from_text(&valid, None, "current", None);
    assert_eq!(
        result
            .result
            .error
            .as_ref()
            .map(ToString::to_string)
            .as_deref(),
        Some("browser opener is not configured")
    );
    assert!(result.relay_url.is_empty());
    let result = login_input_from_text(
        &valid,
        None,
        "current",
        Some(&mut |_| Err(std::io::Error::other("opener failure").into())),
    );
    assert_eq!(
        result
            .result
            .error
            .as_ref()
            .map(ToString::to_string)
            .as_deref(),
        Some("could not open Pixiv authorization relay URL: opener failure")
    );
    assert!(result.relay_url.is_empty());
    let source = result.result.error.as_ref().unwrap().source().unwrap();
    assert!(source.downcast_ref::<std::io::Error>().is_some());
    assert_eq!(source.to_string(), "opener failure");
    assert!(pixiv_auth_start_matches_challenge("invalid\0", ""));
    assert!(pixiv_auth_start_matches_challenge(
        &format!("{start}&code_challenge=stale"),
        "current"
    ));
    assert!(!pixiv_auth_start_matches_challenge(
        &format!("{start};bad=x"),
        "current"
    ));
    assert_eq!(pixiv_login_challenge(&format!("{start}%20")), "current");
}

#[test]
fn loopback_address_validation_does_not_restrict_bound_listener_ssh_hints() {
    let cases = [
        (
            "",
            "--addr cannot be empty",
            "parse login listener address: missing port in address",
        ),
        ("localhoſt:80", "", "ssh -N -L 80:localhoſt:80 USER@SERVER"),
        ("127.0.0.1:0", "", "ssh -N -L 0:127.0.0.1:0 USER@SERVER"),
        ("LOCALHOST:", "", "login listener address is incomplete"),
        (
            "127.1.2.3:abc",
            "",
            "ssh -N -L abc:127.1.2.3:abc USER@SERVER",
        ),
        ("[::1]:41871", "", "ssh -N -L 41871:::1:41871 USER@SERVER"),
        (
            "[::ffff:127.0.0.1]:80",
            "",
            "ssh -N -L 80:::ffff:127.0.0.1:80 USER@SERVER",
        ),
        (
            "[::1%lo]:80",
            "--addr must bind to a loopback address, got \"[::1%lo]:80\"",
            "ssh -N -L 80:::1%lo:80 USER@SERVER",
        ),
        (
            ":80",
            "--addr must bind to a loopback address, got \":80\"",
            "login listener address is incomplete",
        ),
        (
            "0.0.0.0:80",
            "--addr must bind to a loopback address, got \"0.0.0.0:80\"",
            "ssh -N -L 80:0.0.0.0:80 USER@SERVER",
        ),
        (
            "bad",
            "invalid --addr \"bad\": address bad: missing port in address",
            "parse login listener address: address bad: missing port in address",
        ),
        (
            "::1:80",
            "invalid --addr \"::1:80\": address ::1:80: too many colons in address",
            "parse login listener address: address ::1:80: too many colons in address",
        ),
        (
            "[::1:80",
            "invalid --addr \"[::1:80\": address [::1:80: missing ']' in address",
            "parse login listener address: address [::1:80: missing ']' in address",
        ),
    ];
    for (addr, validation, hint) in cases {
        assert_eq!(
            validate_login_addr(addr)
                .err()
                .map(|error| error.to_string())
                .unwrap_or_default(),
            validation,
            "{addr}"
        );
        let actual = login_ssh_tunnel_command(addr).unwrap_or_else(|error| error.to_string());
        assert_eq!(actual, hint, "{addr}");
    }
    for error in [
        validate_login_addr("bad").unwrap_err(),
        login_ssh_tunnel_command("bad").unwrap_err(),
    ] {
        let source = error
            .source()
            .unwrap()
            .downcast_ref::<LoginAddressParseError>()
            .unwrap();
        assert_eq!(source.address, "bad");
        assert_eq!(source.reason, "missing port in address");
    }
    assert_eq!(
        validate_login_addr(" \t").unwrap_err().to_string(),
        "--addr cannot be empty"
    );
}

#[test]
fn browser_callback_predicate_preserves_official_endpoint_and_deep_link_rules() {
    for input in [
        "pixiv://account/login?code=one",
        "PIXIV://ACCOUNT/login",
        "https://user@app-api.pixiv.net/web/v1/users/auth/pixiv/callback",
    ] {
        assert!(is_browser_callback_url(input), "{input}");
    }
    for input in [
        "pixiv://account/Login",
        "https://app-api.pixiv.net:443/web/v1/users/auth/pixiv/callback",
        "http://app-api.pixiv.net/web/v1/users/auth/pixiv/callback",
        "http://127.0.0.1:80/callback",
    ] {
        assert!(!is_browser_callback_url(input), "{input}");
    }
}
