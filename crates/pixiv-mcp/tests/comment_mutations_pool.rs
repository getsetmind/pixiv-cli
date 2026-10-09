use pixiv_app::{
    config::Store,
    database::{Database, PixivAccount},
    execution::Execution,
};
use pixiv_sdk::transport::{Request, Response, Transport};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Deserialize)]
struct Case {
    tool: String,
    arguments: Value,
    result: Value,
    opens: Vec<i64>,
    requests: Vec<i64>,
    closes: usize,
    states: Vec<State>,
    reusable: bool,
}
#[derive(Debug, Deserialize, PartialEq)]
struct State {
    id: i64,
    revision: i64,
    token: String,
    frozen: bool,
    selected: bool,
}
#[derive(Default)]
struct Observed {
    opens: Mutex<Vec<i64>>,
    requests: Mutex<Vec<i64>>,
    closes: AtomicUsize,
}
struct Fixture {
    database: Arc<Mutex<Database>>,
    observed: Arc<Observed>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.observed.closes.fetch_add(1, Ordering::SeqCst);
    }
}
impl Transport for Fixture {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        let oauth = request.operation == "Open";
        let token = if oauth {
            &request
                .parameters
                .iter()
                .find(|(k, _)| k == "refresh_token")
                .unwrap()
                .1
        } else {
            &request
                .headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
                .unwrap()
                .1
        };
        let id = token.rsplit('-').next().unwrap().parse::<i64>().unwrap();
        let account = self.database.lock().unwrap().get_pixiv(id).unwrap();
        if oauth {
            assert_eq!(token.as_bytes(), account.refresh_token_copy());
            self.observed.opens.lock().unwrap().push(id);
            return Ok(Response {
                status: 200,
                retry_after: None,
                body: json!({"access_token":format!("fixture-access-{id}"),"refresh_token":format!("fixture-rotated-{id}"),"expires_in":3600,"user":{"id":id}}),
            });
        }
        assert_eq!(account.credential_revision, 2);
        assert_eq!(
            account.refresh_token_copy(),
            format!("fixture-rotated-{id}").as_bytes()
        );
        assert_eq!(request.method.as_str(), "POST");
        self.observed.requests.lock().unwrap().push(id);
        Ok(Response {
            status: 429,
            retry_after: Some(chrono::TimeDelta::seconds(120)),
            body: json!({}),
        })
    }
}
#[tokio::test]
async fn comment_mutations_pool_matches_go_committed_failures_without_second_account_replay() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/mcp-comment-mutations-pool.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 8);
    for case in cases {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(
            &path,
            "[account_pool]\nenabled = true\nstrategy = 'round_robin'\n",
        )
        .unwrap();
        let mut db = Database::open(directory.path()).unwrap();
        for id in [42, 43] {
            db.save_pixiv_credential(&PixivAccount::new(
                id,
                "fixture",
                format!("fixture-refresh-{id}").as_bytes(),
            ))
            .unwrap();
        }
        db.set_all_pixiv_schedulable(true).unwrap();
        let database = Arc::new(Mutex::new(db));
        let observed = Arc::new(Observed::default());
        let fd = database.clone();
        let fo = observed.clone();
        let execution = Execution::new(Store::new(path), database.clone(), move |_| {
            Ok(Fixture {
                database: fd.clone(),
                observed: fo.clone(),
            })
        });
        let messages = [
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":case.tool,"arguments":case.arguments}}),
        ];
        let input = messages
            .iter()
            .map(|m| m.to_string() + "\n")
            .collect::<String>();
        let mut output = vec![];
        pixiv_mcp::stdio::serve_saved(&execution, input.as_bytes(), &mut output)
            .await
            .unwrap();
        let responses = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            responses.iter().find(|r| r["id"] == 2).unwrap()["result"],
            case.result,
            "{}",
            case.tool
        );
        assert_eq!(*observed.opens.lock().unwrap(), case.opens);
        assert_eq!(*observed.requests.lock().unwrap(), case.requests);
        assert_eq!(observed.closes.load(Ordering::SeqCst), case.closes);
        let states = [42, 43]
            .into_iter()
            .map(|id| {
                let a = database.lock().unwrap().get_pixiv(id).unwrap();
                State {
                    id,
                    revision: a.credential_revision,
                    token: String::from_utf8(a.refresh_token_copy()).unwrap(),
                    frozen: a
                        .pool_frozen_until
                        .is_some_and(|t| t > chrono::Utc::now().timestamp()),
                    selected: a.pool_last_selected,
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(states, case.states);
        assert!(case.reusable);
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            execution.read(
                &pixiv_app::lifecycle::Context::new(),
                0,
                None,
                |_, _| async { Ok(()) },
            ),
        )
        .await
        .unwrap()
        .unwrap();
    }
}
