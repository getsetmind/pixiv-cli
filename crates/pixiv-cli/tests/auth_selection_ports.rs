use pixiv_app::{
    account_management::AccountDefaultStore,
    account_service::{AccountRepository, AccountService},
    database::{PixivAccount, PoolStatus},
    lifecycle::Context,
    scheduler::SchedulerError,
};
use pixiv_cli_rs::{
    CommandError,
    auth_accounts::{AccountPrompts, AccountSelection, SelectionOptions},
};
use std::sync::{Arc, Mutex};
struct Repository {
    calls: Mutex<Vec<String>>,
    fail_list: usize,
    removed: Mutex<bool>,
}
impl AccountRepository for Repository {
    fn pool_status(&self, _: &Context, _: i64) -> Result<PoolStatus, SchedulerError> {
        let mut calls = self.calls.lock().unwrap();
        calls.push("list".into());
        let n = calls.iter().filter(|s| *s == "list").count();
        if n == self.fail_list {
            return Err(SchedulerError::Message(format!("fixture list {n} failed")));
        }
        Ok(PoolStatus {
            accounts: vec![],
            earliest_frozen_until: None,
        })
    }
    fn list(&self, _: &Context) -> Result<Vec<PixivAccount>, SchedulerError> {
        Ok(vec![
            PixivAccount::new(2, "合成-user", b"synthetic"),
            PixivAccount::new(1, "", b"synthetic"),
        ])
    }
    fn get(&self, _: &Context, id: i64) -> Result<PixivAccount, SchedulerError> {
        self.calls.lock().unwrap().push(format!("use:{id}"));
        Ok(PixivAccount::new(id, "", b"synthetic"))
    }
    fn remove(&self, _: &Context, id: i64) -> Result<(), SchedulerError> {
        self.calls.lock().unwrap().push(format!("remove:{id}"));
        *self.removed.lock().unwrap() = true;
        Ok(())
    }
    fn rotate(&self, _: &Context, _: i64, _: i64, _: &[u8]) -> Result<(), SchedulerError> {
        panic!("selection does not authenticate")
    }
}
struct Defaults;
impl AccountDefaultStore for Defaults {
    fn read(&self) -> Result<Option<i64>, SchedulerError> {
        Ok(Some(1))
    }
    fn set(&self, _: i64) -> Result<(), SchedulerError> {
        Ok(())
    }
    fn clear(&self) -> Result<(), SchedulerError> {
        Ok(())
    }
}
struct Prompts {
    selected: &'static str,
    confirmed: bool,
    calls: Vec<&'static str>,
    terminal: bool,
    remove: bool,
}
impl AccountPrompts for Prompts {
    fn can_prompt(&self) -> bool {
        self.terminal
    }
    fn select(&mut self, message: &str, options: &[String]) -> Result<String, CommandError> {
        assert_eq!(
            message,
            if self.remove {
                "Select account to remove"
            } else {
                "Select default account"
            }
        );
        assert_eq!(options, ["2 合成-user", "1"]);
        self.calls.push("select");
        Ok(self.selected.into())
    }
    fn confirm(&mut self, message: &str, default: bool) -> Result<bool, CommandError> {
        assert_eq!(message, "Remove uid 2?");
        assert!(!default);
        self.calls.push("confirm");
        Ok(self.confirmed)
    }
}
#[test]
fn selection_ports_preserve_go_labels_cancellation_yes_and_wrapper_list_order() {
    for (remove, selected, yes, confirmed, error, calls, out) in [
        (
            false,
            "2 合成-user",
            false,
            false,
            "",
            "list,use:2",
            "default uid: 2\n",
        ),
        (false, "", false, false, "uid cannot be empty", "list", ""),
        (
            true,
            "2 合成-user",
            false,
            false,
            "account removal canceled",
            "list",
            "",
        ),
        (
            true,
            "2 合成-user",
            false,
            true,
            "",
            "list,list,remove:2,list",
            "account uid:2 removed\ndefault uid: 1\n",
        ),
        (
            true,
            "1",
            true,
            false,
            "",
            "list,list,remove:1,list",
            "account uid:1 removed\ndefault uid: 1\n",
        ),
    ] {
        let repository = Arc::new(Repository {
            calls: Mutex::new(vec![]),
            fail_list: 0,
            removed: Mutex::new(false),
        });
        let service = AccountService {
            repository: repository.clone(),
            defaults: Some(Arc::new(|| Ok(Some(1)))),
        };
        let mut output = vec![];
        let mut prompts = Prompts {
            selected,
            confirmed,
            calls: vec![],
            terminal: true,
            remove,
        };
        let result = AccountSelection {
            service: &service,
            defaults: &Defaults,
        }
        .execute(
            None,
            SelectionOptions {
                remove,
                json: false,
                yes,
            },
            &mut output,
            &mut prompts,
        );
        assert_eq!(
            result.err().map(|e| e.to_string()).unwrap_or_default(),
            error
        );
        assert_eq!(repository.calls.lock().unwrap().join(","), calls);
        assert_eq!(String::from_utf8(output).unwrap(), out);
        if remove && !yes {
            assert_eq!(prompts.calls, ["select", "confirm"])
        } else {
            assert_eq!(prompts.calls, ["select"])
        }
    }
}
#[test]
fn selection_lists_fail_before_mutation_or_report_committed_delete_without_output() {
    for fail_list in 1..=3 {
        let repository = Arc::new(Repository {
            calls: Mutex::new(vec![]),
            fail_list,
            removed: Mutex::new(false),
        });
        let service = AccountService {
            repository: repository.clone(),
            defaults: Some(Arc::new(|| Ok(Some(1)))),
        };
        let mut output = vec![];
        let mut prompts = Prompts {
            selected: "",
            confirmed: false,
            calls: vec![],
            terminal: false,
            remove: true,
        };
        let result = AccountSelection {
            service: &service,
            defaults: &Defaults,
        }
        .execute(
            Some("2"),
            SelectionOptions {
                remove: true,
                json: true,
                yes: false,
            },
            &mut output,
            &mut prompts,
        );
        assert_eq!(
            result.unwrap_err().to_string(),
            format!("fixture list {fail_list} failed")
        );
        assert!(output.is_empty());
        assert_eq!(
            repository.calls.lock().unwrap().join(","),
            ["list", "list,list", "list,list,remove:2,list"][fail_list - 1]
        );
        assert_eq!(*repository.removed.lock().unwrap(), fail_list == 3);
    }
}
