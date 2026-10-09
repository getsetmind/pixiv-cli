use pixiv_cli_rs::{auth_hidden::HiddenAuthCommand, finish_command};

fn fixture() -> serde_json::Value {
    serde_json::from_str(include_str!("fixtures/cli_login_hidden_startup.json")).unwrap()
}

#[test]
fn hidden_parser_preserves_go_help_flag_and_exact_argument_contracts() {
    for case in fixture()["cli"].as_array().unwrap() {
        let args: Vec<String> = case["args"]
            .as_array()
            .unwrap()
            .iter()
            .skip(1)
            .map(|value| value.as_str().unwrap().to_owned())
            .collect();
        if !args
            .iter()
            .any(|arg| matches!(arg.as_str(), "_callback" | "_install-handler"))
        {
            continue;
        }
        let parsed = HiddenAuthCommand::parse(&args);
        let name = case["name"].as_str().unwrap();
        if case["stdout"]
            .as_str()
            .unwrap()
            .starts_with("Manage local Pixiv authentication\n")
        {
            assert!(
                parsed.unwrap().is_none(),
                "{name}: group help must stay with the public auth owner"
            );
            continue;
        }
        let expected_error = case["stderr"].as_str().unwrap();
        if expected_error.starts_with("error: unknown option ")
            || expected_error.starts_with("error: usage: ")
            || expected_error.starts_with("error: invalid argument ")
        {
            let mut diagnostics = Vec::new();
            let error = parsed
                .err()
                .unwrap_or_else(|| panic!("{name}: parser accepted invalid flags or arity"));
            let exit = finish_command(Err(error), false, false, &mut diagnostics);
            assert_eq!(exit, case["exit"].as_i64().unwrap() as i32, "{name}");
            assert_eq!(
                diagnostics,
                case["stderr"].as_str().unwrap().as_bytes(),
                "{name}"
            );
            continue;
        }
        let command = parsed.unwrap_or_else(|error| panic!("{name}: {error}"));
        let command = command.unwrap_or_else(|| panic!("{name}: hidden route was not discovered"));
        match command {
            HiddenAuthCommand::Help(body) => {
                assert_eq!(body, case["stdout"].as_str().unwrap(), "{name}");
                assert_eq!(case["exit"], 0, "{name}");
            }
            HiddenAuthCommand::Install => {
                assert!(args.iter().any(|arg| arg == "_install-handler"), "{name}");
            }
            HiddenAuthCommand::Callback(raw) => {
                let operation = args.iter().position(|arg| arg == "_callback").unwrap();
                assert_eq!(raw, args[operation + 1], "{name}");
                assert!(case["stdout"].as_str().unwrap().is_empty(), "{name}");
            }
        }
        assert_eq!(case["stdin_reads"], 0, "{name}");
    }
}

#[test]
fn hidden_parser_leaves_public_auth_commands_to_their_existing_owner() {
    for args in [
        vec![],
        vec!["--help"],
        vec!["login"],
        vec!["list"],
        vec!["_other"],
    ] {
        let args = args.into_iter().map(str::to_owned).collect::<Vec<_>>();
        assert!(
            HiddenAuthCommand::parse(&args).unwrap().is_none(),
            "{args:?}"
        );
    }
}
