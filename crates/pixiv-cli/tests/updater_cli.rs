#[path = "support/updater_cli_connected.rs"]
mod connected;

use connected::*;
use pixiv_app::update::BuildInfo;
use pixiv_cli_rs::{
    CommandError,
    update::{
        AutomaticCommand, UpdateCommand, UpdateStartup, finish_with_automatic_check,
        run_automatic_check, should_check,
    },
};
use serde_json::{Value, json};
use std::sync::Arc;

#[test]
fn frozen_rows_keep_their_original_evidence_boundaries() {
    let rows = fixture();
    assert_eq!(rows.len(), 170);
    assert_eq!(
        rows.iter()
            .filter(|row| row["evidence_class"] == "behavioral")
            .count(),
        158
    );
    assert_eq!(
        rows.iter()
            .filter(|row| row["evidence_class"] == "go-only")
            .count(),
        12
    );
}

#[tokio::test]
async fn parser_and_update_service_match_frozen_outputs_at_the_library_boundary() {
    let mut covered = Vec::new();
    for row in fixture().into_iter().filter(library_update_row) {
        let input = &row["input"];
        let args = arguments(input);
        let mut output = Sink::new(input, "stdout_writer");
        let mut diagnostics = Sink::new(input, "stderr_writer");
        let host = Host::new(input.clone());
        let machine = UpdateCommand::machine_output_requested(&args[1..]);
        let parsed = UpdateCommand::parse(&args[1..]);
        if let Ok(UpdateCommand::Run(options)) = &parsed {
            assert_flags(options, &row["observation"]["flags"], &row["name"]);
        }
        let result = match parsed {
            Ok(command) => {
                command
                    .execute_with_startup(
                        host.context.clone(),
                        build(input),
                        &host,
                        UpdateStartup {
                            context: &host.startup_context,
                            hooks: &host,
                            preparation: &host,
                        },
                        &mut output,
                        &mut diagnostics,
                    )
                    .await
            }
            Err(error) => Err(error),
        };
        let exit = pixiv_cli_rs::finish_command(result, false, machine, &mut diagnostics);
        assert_eq!(
            exit,
            row["observation"]["exit"].as_i64().unwrap() as i32,
            "{}",
            row["name"]
        );
        assert_eq!(
            output.text(),
            row["observation"]["stdout"],
            "{} stdout",
            row["name"]
        );
        assert_eq!(
            diagnostics.text(),
            row["observation"]["stderr"],
            "{} stderr",
            row["name"]
        );
        assert_eq!(
            json!(host.proxies()),
            row["observation"]["update_proxies"],
            "{} proxy",
            row["name"]
        );
        assert_eq!(
            json!(host.commands()),
            row["observation"]["commands"],
            "{} commands",
            row["name"]
        );
        assert_installs(
            &host.installs(),
            &row["observation"]["installs"],
            &row["name"],
        );
        let trace: Vec<String> = row["observation"]["trace"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry.as_str().unwrap())
            .filter(|entry| !entry.ends_with(".write"))
            .map(str::to_owned)
            .collect();
        assert_eq!(host.trace(), trace, "{} lifecycle", row["name"]);
        assert_eq!(
            json!(host.detections()),
            row["observation"]["detections"],
            "{} detection",
            row["name"]
        );
        assert_eq!(
            json!(host.requests()),
            row["observation"]["requests"],
            "{} requests",
            row["name"]
        );
        assert_eq!(
            host.cache_text(),
            row["observation"]["cache_after"],
            "{} cache",
            row["name"]
        );
        assert!(
            !host.directory.path().join("pixiv-cli.db").exists(),
            "{} opened accounts DB",
            row["name"]
        );
        assert_eq!(
            output.writes(),
            row["observation"]["output_writes"],
            "{} writes",
            row["name"]
        );
        covered.push(row["name"].as_str().unwrap().to_owned());
    }
    assert_eq!(
        covered.len(),
        83,
        "library update slice changed: {covered:?}"
    );
}

#[test]
fn changed_json_is_recorded_only_after_a_successful_flag_assignment() {
    for (args, expected) in [
        (vec!["--json=false"], true),
        (vec!["--json=bad"], false),
        (vec!["--json=false", "--json=bad"], true),
        (vec!["--unknown", "--json"], false),
        (vec!["--proxy", "--json"], false),
        (vec!["--", "--json"], false),
    ] {
        assert_eq!(
            UpdateCommand::machine_output_requested(&strings(&args)),
            expected,
            "{args:?}"
        );
    }
}

#[test]
fn exact_development_and_actual_command_metadata_control_automatic_policy() {
    let formal = BuildInfo {
        version: "v1.2.3".into(),
    };
    let eligible = AutomaticCommand::leaf(["pixiv", "config", "get"]);
    assert!(should_check(&eligible, &formal));
    for version in ["", "development", "DEV", "vdev"] {
        assert!(should_check(
            &eligible,
            &BuildInfo {
                version: version.into()
            }
        ));
    }
    assert!(!should_check(
        &eligible,
        &BuildInfo {
            version: "dev".into()
        }
    ));
    for names in [
        vec![],
        vec!["pixiv"],
        vec!["pixiv", "mcp"],
        vec!["pixiv", "fanbox", "mcp"],
        vec!["pixiv", "auth", "_callback"],
        vec!["pixiv", "update"],
        vec!["pixiv", "help", "config"],
        vec!["pixiv", "auth", "export"],
    ] {
        assert!(!should_check(&AutomaticCommand::leaf(names), &formal));
    }
    let mut changed = eligible.clone();
    changed.help_changed = true;
    assert!(!should_check(&changed, &formal));
    changed.help_changed = false;
    changed.has_subcommands = true;
    assert!(!should_check(&changed, &formal));
    let mut import = AutomaticCommand::leaf(["pixiv", "auth", "import"]);
    assert!(should_check(&import, &formal));
    import.skip_automatic_update = true;
    assert!(!should_check(&import, &formal));
}

#[tokio::test]
async fn automatic_checker_matches_frozen_notice_warning_proxy_and_cache_policy() {
    let mut covered = Vec::new();
    for row in fixture().into_iter().filter(library_automatic_row) {
        let input = &row["input"];
        let host = Host::new(input.clone());
        let mut diagnostics = Sink::new(input, "stderr_writer");
        let command = automatic_metadata(input);
        run_automatic_check(
            &command,
            host.context.clone(),
            build(input),
            &host,
            &mut diagnostics,
        )
        .await;
        let expected = row["observation"]["stderr"].as_str().unwrap();
        let expected = expected.split("{\"error\"").next().unwrap();
        assert_eq!(diagnostics.text(), expected, "{} diagnostics", row["name"]);
        assert_eq!(
            json!(host.automatic_proxies()),
            row["observation"]["automatic_proxies"],
            "{} proxy",
            row["name"]
        );
        assert_eq!(
            host.cache_text(),
            row["observation"]["cache_after"],
            "{} cache",
            row["name"]
        );
        assert!(host.commands().is_empty());
        assert!(host.installs().is_empty());
        covered.push(row["name"].as_str().unwrap().to_owned());
    }
    assert_eq!(
        covered.len(),
        26,
        "library automatic slice changed: {covered:?}"
    );
}

#[tokio::test]
async fn unsuccessful_business_results_do_not_construct_an_automatic_checker() {
    let input = fixture()
        .into_iter()
        .find(|row| row["name"] == "automatic/check-warning")
        .unwrap()["input"]
        .clone();
    let host = Host::new(input.clone());
    let mut diagnostics = Sink::new(&input, "stderr_writer");
    let result = finish_with_automatic_check(
        Err(CommandError::Message("owned business failure")),
        &AutomaticCommand::leaf(["pixiv", "detail"]),
        host.context.clone(),
        build(&input),
        &host,
        &mut diagnostics,
    )
    .await;
    assert_eq!(result.unwrap_err().to_string(), "owned business failure");
    assert!(host.automatic_proxies().is_empty());
    assert_eq!(diagnostics.text(), "");
}

#[tokio::test]
async fn automatic_notices_precede_resource_closing_and_do_not_hide_close_errors() {
    let input = fixture()
        .into_iter()
        .find(|row| row["name"] == "automatic/close-failure-after-notice")
        .unwrap()["input"]
        .clone();
    let host = Host::new(input.clone());
    let mut diagnostics = Sink::new(&input, "stderr_writer");
    let command = AutomaticCommand::leaf(["pixiv", "auth", "list"]);
    let result = finish_with_automatic_check(
        Ok(()),
        &command,
        host.context.clone(),
        build(&input),
        &host,
        &mut diagnostics,
    )
    .await;
    let result = pixiv_cli_rs::finish_with_cleanup(
        result,
        Err(CommandError::Message("owned database close failure")),
    );
    let exit = pixiv_cli_rs::finish_command(result, false, true, &mut diagnostics);
    let row = fixture()
        .into_iter()
        .find(|row| row["name"] == "automatic/close-failure-after-notice")
        .unwrap();
    assert_eq!(exit, 1);
    assert_eq!(diagnostics.text(), row["observation"]["stderr"]);
    assert!(!host.automatic_proxies().is_empty());
}

#[tokio::test]
async fn automatic_changed_proxy_conflicts_are_warnings_without_factory_or_transport() {
    let input = fixture()
        .into_iter()
        .find(|row| row["name"] == "automatic/config-disabled")
        .unwrap()["input"]
        .clone();
    let mut input = input;
    input["config_before"] = Value::String("[update]\ncheck_enabled=true\n".into());
    let host = Host::new(input.clone());
    let mut command = AutomaticCommand::leaf(["pixiv", "detail"]);
    command.proxy = Some("owned".into());
    command.no_proxy_changed = true;
    let mut diagnostics = Sink::new(&input, "stderr_writer");
    run_automatic_check(
        &command,
        host.context.clone(),
        build(&input),
        &host,
        &mut diagnostics,
    )
    .await;
    assert_eq!(
        diagnostics.text(),
        "warning: read automatic update proxy override: use either --proxy or --no-proxy, not both\n"
    );
    assert!(host.automatic_proxies().is_empty());
}

#[tokio::test]
async fn the_original_caller_context_reaches_the_update_release_port() {
    let input = fixture()
        .into_iter()
        .find(|row| row["name"] == "update/check-human")
        .unwrap()["input"]
        .clone();
    let host = Host::new(input.clone());
    let mut output = Vec::new();
    let mut diagnostics = Vec::new();
    UpdateCommand::parse(&strings(&["--check"]))
        .unwrap()
        .execute(
            host.context.clone(),
            build(&input),
            &host,
            &mut output,
            &mut diagnostics,
        )
        .await
        .unwrap();
    assert!(host.context_seen());
    assert_eq!(Arc::strong_count(&host.context), 2);
}

#[tokio::test]
async fn actual_config_owner_changes_feed_the_post_success_update_runtime() {
    use pixiv_cli_rs::config_commands::ConfigCommand;
    for name in [
        "automatic/eligible/config get request_interval",
        "automatic/eligible/config set request_interval 0",
        "automatic/eligible/config unset https_proxy",
    ] {
        let row = fixture()
            .into_iter()
            .find(|row| row["name"] == name)
            .unwrap();
        let input = &row["input"];
        let host = Host::new(input.clone());
        let mut reader = std::io::Cursor::new(Vec::<u8>::new());
        let mut output = Vec::new();
        let mut diagnostics = Vec::new();
        let args = arguments(input);
        let command = ConfigCommand::parse(&args[1..], &mut reader, false).unwrap();
        let result = command.execute(host.store(), &mut reader, &mut output, &mut diagnostics);
        finish_with_automatic_check(
            result,
            &automatic_metadata(input),
            host.context.clone(),
            build(input),
            &host,
            &mut diagnostics,
        )
        .await
        .unwrap();
        assert!(
            String::from_utf8(diagnostics)
                .unwrap()
                .ends_with("update available: v1.2.3 -> v1.3.0\nrun: pixiv update\n")
        );
        assert_eq!(
            json!(host.automatic_proxies()),
            row["observation"]["automatic_proxies"],
            "{name}"
        );
        assert_eq!(
            host.cache_text(),
            row["observation"]["cache_after"],
            "{name}"
        );
        assert!(!host.directory.path().join("pixiv-cli.db").exists());
    }
}
