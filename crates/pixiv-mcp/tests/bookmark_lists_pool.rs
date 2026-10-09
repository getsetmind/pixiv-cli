use pixiv_app::{
    config::Store,
    database::{Database, PixivAccount},
    execution::Execution,
};
use pixiv_sdk::{
    Result,
    transport::{Request, Response, Transport},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[derive(Deserialize)]
struct Case {
    explicit: bool,
    targets: Vec<i64>,
    tool_name: String,
    mode: String,
    body: Value,
    results: Vec<Value>,
    opens: Vec<i64>,
    requests: Vec<i64>,
    paths: Vec<String>,
    queries: Vec<String>,
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

struct Observed {
    targets: Mutex<Vec<i64>>,
    opens: Mutex<Vec<i64>>,
    requests: Mutex<Vec<i64>>,
    paths: Mutex<Vec<String>>,
    queries: Mutex<Vec<String>>,
    closes: AtomicUsize,
}

struct Fixture {
    explicit: bool,
    tool_name: String,
    mode: String,
    body: Value,
    database: Arc<Mutex<Database>>,
    observed: Arc<Observed>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.observed.closes.fetch_add(1, Ordering::SeqCst);
    }
}

impl Transport for Fixture {
    async fn send(&self, request: Request) -> Result<Response> {
        let oauth = request.operation == "Open";
        let token = if oauth {
            &request
                .parameters
                .iter()
                .find(|(key, _)| key == "refresh_token")
                .unwrap()
                .1
        } else {
            &request
                .headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case("authorization"))
                .unwrap()
                .1
        };
        let id: i64 = token.rsplit('-').next().unwrap().parse().unwrap();
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
        assert!(account.credential_revision >= 2);
        assert_eq!(
            account.refresh_token_copy(),
            format!("fixture-rotated-{id}").as_bytes()
        );
        let count = {
            let mut requests = self.observed.requests.lock().unwrap();
            requests.push(id);
            requests
                .iter()
                .filter(|requested| **requested == id)
                .count()
        };
        let throttled = if self.explicit { 42 } else { 43 };
        let replayed = if self.explicit { 43 } else { 42 };
        let target = request
            .parameters
            .iter()
            .find(|(key, _)| key == "user_id")
            .unwrap()
            .1
            .parse()
            .unwrap();
        self.observed.targets.lock().unwrap().push(target);
        let status = if id == throttled
            && count > 1
            && (self.tool_name != "bookmark_list_all" || request.url.ends_with("/novel"))
        {
            429
        } else {
            200
        };
        let body = if self.tool_name == "bookmark_list_all" && request.url.ends_with("/novel") {
            if self.mode == "replay_malformed" && id == replayed {
                json!({})
            } else {
                json!({"novels":[{"id":22,"title":"novel","user":{"id":7},"create_date":"2024-01-02T03:04:05+00:00"}],"next_url":null})
            }
        } else if request
            .parameters
            .iter()
            .any(|(key, _)| key == "max_bookmark_id")
        {
            if self.tool_name != "bookmark_list_all"
                && self.mode == "replay_malformed"
                && id == replayed
            {
                json!({})
            } else if self.tool_name != "user_novel_bookmarks" {
                json!({"illusts":[{"id":23,"type":"illust","user":{"id":7},"create_date":"2024-01-02T03:04:05+00:00"}],"next_url":null})
            } else {
                json!({"novels":[{"id":23,"user":{"id":7},"create_date":"2024-01-02T03:04:05+00:00"}],"next_url":null})
            }
        } else if id == throttled {
            let mut body = self.body.clone();
            body[if self.tool_name != "user_novel_bookmarks" {
                "illusts"
            } else {
                "novels"
            }][0]["title"] = json!("discarded account metadata");
            body
        } else {
            self.body.clone()
        };
        self.observed
            .paths
            .lock()
            .unwrap()
            .push(url::Url::parse(&request.url).unwrap().path().into());
        let mut parameters = request.parameters.clone();
        parameters.sort();
        self.observed.queries.lock().unwrap().push(
            url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs(parameters)
                .finish(),
        );
        Ok(Response {
            status,
            retry_after: if status == 429 {
                Some(chrono::TimeDelta::seconds(
                    if (self.tool_name != "bookmark_list_all" && count % 2 == 0)
                        || (self.tool_name == "bookmark_list_all" && count % 2 == 1)
                    {
                        0
                    } else {
                        120
                    },
                ))
            } else {
                None
            },
            body,
        })
    }
}

#[tokio::test]
async fn bookmark_lists_pool_preserves_separate_identity_selection_resolved_target_and_replay() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/mcp-bookmark-lists-pool.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 12);
    for case in cases {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(
            &path,
            "[account_pool]\nenabled = true\nstrategy = 'round_robin'\n",
        )
        .unwrap();
        let mut database = Database::open(directory.path()).unwrap();
        for id in [42, 43] {
            database
                .save_pixiv_credential(&PixivAccount::new(
                    id,
                    "fixture",
                    format!("fixture-refresh-{id}").as_bytes(),
                ))
                .unwrap();
        }
        database.set_all_pixiv_schedulable(true).unwrap();
        let database = Arc::new(Mutex::new(database));
        let observed = Arc::new(Observed {
            targets: Mutex::new(vec![]),
            opens: Mutex::new(vec![]),
            requests: Mutex::new(vec![]),
            paths: Mutex::new(vec![]),
            queries: Mutex::new(vec![]),
            closes: AtomicUsize::new(0),
        });
        let factory_database = database.clone();
        let factory_observed = observed.clone();
        let tool_name = case.tool_name.clone();
        let mode = case.mode.clone();
        let body = case.body;
        let explicit = case.explicit;
        let execution = Execution::new(Store::new(path), database.clone(), move |_| {
            Ok(Fixture {
                explicit,
                tool_name: tool_name.clone(),
                mode: mode.clone(),
                body: body.clone(),
                database: factory_database.clone(),
                observed: factory_observed.clone(),
            })
        });
        let (server, peer) = tokio::io::duplex(4096);
        let (server_read, mut server_write) = tokio::io::split(server);
        let (peer_read, mut peer_write) = tokio::io::split(peer);
        let server = pixiv_mcp::stdio::serve_saved(&execution, server_read, &mut server_write);
        let peer = async {
            let mut lines = BufReader::new(peer_read).lines();
            let init = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"migration","version":"0"}}});
            peer_write
                .write_all((init.to_string() + "\n").as_bytes())
                .await
                .unwrap();
            let _ = lines.next_line().await.unwrap().unwrap();
            for (index, expected) in case.results.iter().enumerate() {
                let args = if case.explicit {
                    json!({"user_id":7,"limit":0})
                } else {
                    json!({"limit":0})
                };
                let call = json!({"jsonrpc":"2.0","id":index+7,"method":"tools/call","params":{"name":case.tool_name,"arguments":args}});
                peer_write
                    .write_all((call.to_string() + "\n").as_bytes())
                    .await
                    .unwrap();
                let response: Value =
                    serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
                assert_eq!(response["result"], *expected, "{} / {index}", case.mode);
                assert_eq!(response["id"], index + 7);
            }
            peer_write
                .write_all(
                    b"{\"jsonrpc\":\"2.0\",\"id\":\"probe\",\"method\":\"ping\",\"params\":{}}\n",
                )
                .await
                .unwrap();
            let response: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(response, json!({"jsonrpc":"2.0","id":"probe","result":{}}));
            peer_write.shutdown().await.unwrap();
        };
        let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            tokio::join!(server, peer)
        })
        .await
        .unwrap();
        result.unwrap();
        assert!(case.reusable);
        assert_eq!(
            *observed.targets.lock().unwrap(),
            case.targets,
            "{} resolved targets",
            case.mode
        );
        assert_eq!(*observed.opens.lock().unwrap(), case.opens, "{}", case.mode);
        assert_eq!(*observed.paths.lock().unwrap(), case.paths);
        assert_eq!(*observed.queries.lock().unwrap(), case.queries);
        assert_eq!(
            *observed.requests.lock().unwrap(),
            case.requests,
            "{}",
            case.mode
        );
        assert_eq!(
            observed.closes.load(Ordering::SeqCst),
            case.closes,
            "{}",
            case.mode
        );
        let states: Vec<_> = [42, 43]
            .into_iter()
            .map(|id| {
                let account = database.lock().unwrap().get_pixiv(id).unwrap();
                State {
                    id,
                    revision: account.credential_revision,
                    token: String::from_utf8(account.refresh_token_copy()).unwrap(),
                    frozen: account
                        .pool_frozen_until
                        .is_some_and(|until| until > chrono::Utc::now().timestamp()),
                    selected: account.pool_last_selected,
                }
            })
            .collect();
        assert_eq!(states, case.states, "{}", case.mode);
    }
}
