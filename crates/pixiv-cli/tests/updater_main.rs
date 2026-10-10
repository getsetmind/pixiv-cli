#[path = "support/updater_main_owned.rs"]
mod owned;
use owned::*;

#[test]
fn actual_root_version_uses_the_compile_time_build_identity() {
    let home = OwnedHome::new(None);
    for args in [vec!["--version"], vec!["--version=false", "--version"]] {
        let output = home.run(&args, "");
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(
            text(&output.stdout),
            format!(
                "pixiv {}\n",
                pixiv_app::update::BuildInfo::current().version
            )
        );
        assert!(output.stderr.is_empty());
        home.assert_no_config_or_database();
    }
}

#[test]
fn actual_update_help_and_parser_rejections_precede_startup_and_config() {
    let cases = fixture();
    let selected: Vec<_> = cases
        .iter()
        .filter(|row| {
            let name = row["name"].as_str().unwrap();
            name == "update/help"
                || name == "update/help-with-unknown"
                || name.starts_with("parse/") && row["observation"]["exit"] != 0
                || name.starts_with("update/json-requires-check")
        })
        .collect();
    assert_eq!(selected.len(), 18);
    for row in selected {
        let home = OwnedHome::new(Some("[owned-malformed\n"));
        let args = args(row);
        let output = home.run(&args, "owned stdin must not be consumed\n");
        assert_eq!(
            output.status.code(),
            row["observation"]["exit"].as_i64().map(|n| n as i32),
            "{}",
            row["name"]
        );
        assert_eq!(
            text(&output.stdout),
            row["observation"]["stdout"],
            "{} stdout",
            row["name"]
        );
        assert_eq!(
            text(&output.stderr),
            row["observation"]["stderr"],
            "{} stderr",
            row["name"]
        );
        home.assert_config("[owned-malformed\n");
        home.assert_no_database_or_cache();
    }
}

#[test]
fn actual_update_invalid_production_proxy_fails_closed_without_accounts_or_cache() {
    let row = fixture()
        .into_iter()
        .find(|row| row["name"] == "update/proxy-production-invalid")
        .unwrap();
    let home = OwnedHome::new(Some("[update]\ncheck_enabled=true\n"));
    let output = home.run(&args(&row), "");
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(text(&output.stderr), row["observation"]["stderr"]);
    assert!(output.stdout.is_empty());
    home.assert_no_database_or_cache();
}

#[test]
fn actual_update_development_path_constructs_the_real_factory_before_rejecting_installation() {
    assert_eq!(
        pixiv_app::update::BuildInfo::current().version,
        "dev",
        "This owned process proof requires the ordinary default-dev build"
    );
    for proxy in [
        "",
        "http://owned.invalid:8080",
        "https://owned.invalid:8080",
        "socks5://owned.invalid:1080",
        "socks5h://owned.invalid:1080",
    ] {
        let home = OwnedHome::new(None);
        let proxy = format!("--proxy={proxy}");
        let output = home.run(&["update", "--check", &proxy], "owned stdin\n");
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(
            text(&output.stderr),
            "error: development builds cannot update themselves\n"
        );
        assert!(output.stdout.is_empty());
        assert!(home.config().exists());
        home.assert_no_database_or_cache();
    }
}

#[test]
fn actual_changed_json_false_selects_machine_errors() {
    let home = OwnedHome::new(Some("[update]\ncheck_enabled=true\n"));
    let output = home.run(
        &[
            "update",
            "--check",
            "--json=false",
            "--proxy=ftp://owned.invalid",
        ],
        "",
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        text(&output.stderr),
        "{\"error\":{\"code\":\"command_failed\",\"message\":\"parse update proxy URL: proxy URL must use http, https, socks5, or socks5h: invalid proxy configuration\"}}\n"
    );
    home.assert_no_database_or_cache();
}

#[test]
fn actual_default_dev_config_success_does_not_attempt_an_automatic_network_check() {
    let home = OwnedHome::new(Some(
        "[update]\ncheck_enabled=true\n[network]\nhttps_proxy='ftp://owned.invalid'\n",
    ));
    let output = home.run(&["config", "get", "request_interval"], "");
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    home.assert_no_database_or_cache();
}
