use pixiv_app::lifecycle::{Context, ContextError};
use pixiv_cli_rs::dictionary::service::{
    self, Article, ArticleRequest, Client, Error, ErrorCode, Response, SearchRequest, SearchResult,
    Transport, TransportError,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    error::Error as StdError,
    fmt,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const FIXTURE: &str = include_str!("fixtures/dictionary-service.json");

#[derive(Debug)]
struct FixtureCause;
impl fmt::Display for FixtureCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("synthetic dictionary transport failure")
    }
}
impl StdError for FixtureCause {}

struct State {
    responses: VecDeque<Value>,
    requests: Vec<Value>,
}
#[derive(Clone)]
struct FiniteTransport(Arc<Mutex<State>>);
impl Transport for FiniteTransport {
    async fn get(
        &self,
        context: Option<&Context>,
        url: &str,
        accept: &str,
    ) -> Result<Response, TransportError> {
        let context =
            context.expect("client validated context before its genuine transport boundary");
        let mut state = self.0.lock().unwrap();
        state.requests.push(json!({
            "url":url, "accept":accept,
            "context_canceled":context.error()==Some(ContextError::Canceled),
            "context_deadline":context.error()==Some(ContextError::DeadlineExceeded),
        }));
        let response = state
            .responses
            .pop_front()
            .expect("dictionary exceeded the finite Go responses");
        if response["cancel_after"].as_bool().unwrap() {
            context.cancel();
        }
        match response["error"].as_str().unwrap() {
            "" => Ok(Response {
                status: response["status"].as_u64().unwrap() as u16,
                body: response["body"].as_str().unwrap().as_bytes().to_vec(),
            }),
            "fixture" => Err(Box::new(FixtureCause)),
            "canceled" => Err(Box::new(ContextError::Canceled)),
            "deadline" => Err(Box::new(ContextError::DeadlineExceeded)),
            "context" => Err(Box::new(
                context
                    .error()
                    .expect("context fixture requires an ended context"),
            )),
            other => panic!("unknown Go response cause {other}"),
        }
    }
}
fn context(name: &str) -> Option<Context> {
    match name {
        "background" => Some(Context::new()),
        "nil" => None,
        "canceled" => {
            let context = Context::new();
            context.cancel();
            Some(context)
        }
        "deadline" => Some(Context::with_deadline(
            Instant::now() - Duration::from_secs(1),
        )),
        other => panic!("unknown Go context {other}"),
    }
}
fn article(value: Article) -> Value {
    json!({
        "id":value.id,"title":value.title,"yomigana":value.yomigana,"translation":value.translation,
        "categories":value.categories,"abstract":value.abstract_,"related":value.related,"body":value.body,
        "views":value.views,"works":value.works,"comments":value.comments,"checklists":value.checklists,"url":value.url,
    })
}
fn search(value: SearchResult) -> Value {
    json!({
        "title":value.title,"summary":value.summary,"updated":value.updated,"views":value.views,
        "works":value.works,"checklists":value.checklists,"related":value.related,"url":value.url,"thumbnail":value.thumbnail,
    })
}
fn observed_error(error: &Error) -> Value {
    let cause = error.source();
    json!({
        "code":error.code().as_str(),"message":error.to_string(),"status":error.status_code(),"typed":true,
        "cause":cause.map(ToString::to_string).unwrap_or_default(),
        "is_canceled":cause.is_some_and(|value| value.downcast_ref::<ContextError>()==Some(&ContextError::Canceled)),
        "is_deadline":cause.is_some_and(|value| value.downcast_ref::<ContextError>()==Some(&ContextError::DeadlineExceeded)),
        "is_fixture_cause":cause.is_some_and(|value| value.is::<FixtureCause>()),
    })
}

#[test]
fn service_reference_sources_and_fixture_identity_are_frozen() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    assert_eq!(fixture["schema"], 1);
    assert_eq!(
        fixture["reference"],
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(FIXTURE.as_bytes())),
        "1dc8eb7ccbb34a6cbd1019db1ad046fe49a8afdbd3dfad6560d9b41c8113b057"
    );
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for (path, expected) in fixture["sources"].as_object().unwrap() {
        assert_eq!(
            format!(
                "{:x}",
                Sha256::digest(std::fs::read(root.join(path)).unwrap())
            ),
            expected.as_str().unwrap(),
            "source {path}"
        );
    }
}

#[tokio::test]
async fn anonymous_article_and_selected_page_search_match_every_go_service_row() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    assert_eq!(fixture["articles"].as_array().unwrap().len(), 93);
    assert_eq!(fixture["searches"].as_array().unwrap().len(), 46);
    for (operation, rows) in [
        ("article", &fixture["articles"]),
        ("search", &fixture["searches"]),
    ] {
        for row in rows.as_array().unwrap() {
            let state = Arc::new(Mutex::new(State {
                responses: row["responses"].as_array().unwrap().clone().into(),
                requests: Vec::new(),
            }));
            let transport = FiniteTransport(Arc::clone(&state));
            let client = match row["client"].as_str().unwrap() {
                "normal" => Client::new(Some(transport)),
                "nil" | "nil-transport" => Client::new(None),
                other => panic!("unknown Go client {other}"),
            };
            let context = context(row["context"].as_str().unwrap());
            let result = if operation == "article" {
                client
                    .article(
                        context.as_ref(),
                        ArticleRequest {
                            reference: row["ref"].as_str().unwrap().to_owned(),
                            language: row["language"].as_str().unwrap().to_owned(),
                            skip_counters: row["skip_counters"].as_bool().unwrap(),
                        },
                    )
                    .await
                    .map(article)
            } else {
                client
                    .search(
                        context.as_ref(),
                        SearchRequest {
                            query: row["query"].as_str().unwrap().to_owned(),
                            page: row["page"].as_i64().unwrap(),
                        },
                    )
                    .await
                    .map(|values| Value::Array(values.into_iter().map(search).collect()))
            };
            let name = row["name"].as_str().unwrap();
            match result {
                Ok(value) => {
                    assert_eq!(row["error"], Value::Null, "{operation}/{name}");
                    assert_eq!(
                        value,
                        row[if operation == "article" {
                            "article"
                        } else {
                            "results"
                        }],
                        "{operation}/{name}"
                    );
                }
                Err(error) => {
                    assert_eq!(observed_error(&error), row["error"], "{operation}/{name}");
                    assert_eq!(service::code_of(Some(&error)), error.code());
                }
            }
            let state = state.lock().unwrap();
            assert_eq!(
                state.requests,
                *row["requests"].as_array().unwrap(),
                "{operation}/{name} requests"
            );
            assert!(
                state.responses.is_empty(),
                "{operation}/{name} left finite responses unused"
            );
        }
    }
}

#[derive(Debug)]
struct Wrapped(Error);
impl fmt::Display for Wrapped {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "outer: {}", self.0)
    }
}
impl StdError for Wrapped {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(&self.0)
    }
}

#[test]
fn dictionary_error_codes_stay_independent_and_preserve_the_source_chain() {
    assert_eq!(service::code_of(None), ErrorCode::Unknown);
    assert_eq!(service::code_of(Some(&FixtureCause)), ErrorCode::Unknown);
    let error = Error::new(
        ErrorCode::Transport,
        "controlled",
        Some(Box::new(ContextError::Canceled)),
    );
    let wrapped = Wrapped(error);
    assert_eq!(service::code_of(Some(&wrapped)), ErrorCode::Transport);
    assert_eq!(wrapped.0.status_code(), 0);
    assert_eq!(wrapped.0.to_string(), "controlled");
    assert_eq!(
        wrapped.0.source().unwrap().downcast_ref::<ContextError>(),
        Some(&ContextError::Canceled)
    );
    assert!(wrapped.source().unwrap().is::<Error>());
}

#[path = "support/dictionary_native_http.rs"]
mod native_http;
