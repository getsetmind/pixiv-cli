use pixiv_app::{
    config::Store,
    database::{Database, PixivAccount},
    download::{DownloadSaveClient, SaveFuture},
    execution::Execution,
    lifecycle::Context,
};
use pixiv_mcp::download::{DownloadInput, SaveClientFactory, saved_download};
use pixiv_sdk::{
    Client, Error, Reason,
    error::RetryAdvice,
    resource::{ResourceHeaders, ResourceRef, ResourceResponse, SaveOptions},
    transport::{Request, ResourceReadRequest, ResourceTransport, Response, Transport},
};
use serde_json::json;
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicI64, AtomicUsize, Ordering},
    },
};

#[derive(Default)]
struct Observed {
    opens: Mutex<Vec<i64>>,
    resources: Mutex<Vec<(i64, String)>>,
    closes: AtomicUsize,
}
struct PoolTransport {
    identity: AtomicI64,
    open_failure: bool,
    database: Arc<Mutex<Database>>,
    observed: Arc<Observed>,
}
impl Drop for PoolTransport {
    fn drop(&mut self) {
        self.observed.closes.fetch_add(1, Ordering::SeqCst);
    }
}
impl Transport for PoolTransport {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        assert_eq!(request.operation, "Open");
        let token = &request
            .parameters
            .iter()
            .find(|(key, _)| key == "refresh_token")
            .unwrap()
            .1;
        let id = token.rsplit('-').next().unwrap().parse::<i64>().unwrap();
        assert_eq!(
            self.database
                .lock()
                .unwrap()
                .get_pixiv(id)
                .unwrap()
                .refresh_token_copy(),
            token.as_bytes()
        );
        self.identity.store(id, Ordering::SeqCst);
        self.observed.opens.lock().unwrap().push(id);
        if self.open_failure {
            return Err(
                Error::new(Reason::RateLimited, "Open").with_retry(RetryAdvice {
                    safe: true,
                    after: Some(chrono::Utc::now() + chrono::TimeDelta::seconds(120)),
                }),
            );
        }
        Ok(Response {
            status: 200,
            retry_after: None,
            body: json!({"access_token":format!("fixture-access-{id}"),"refresh_token":format!("fixture-rotated-{id}"),"expires_in":3600,"user":{"id":id}}),
        })
    }
}
impl ResourceTransport for PoolTransport {
    type Body = std::io::Cursor<Vec<u8>>;
    async fn open_resource(
        &self,
        request: ResourceReadRequest,
    ) -> pixiv_sdk::Result<ResourceResponse<Self::Body>> {
        request.validate.as_ref().unwrap()(&request.url)?;
        assert_eq!(request.method, "GET");
        assert_eq!(
            request.headers.get("Referer").unwrap(),
            &vec!["https://app-api.pixiv.net/".to_owned()]
        );
        assert!(!request.headers.contains_key("Authorization"));
        assert!(!request.headers.contains_key("Cookie"));
        let id = self.identity.load(Ordering::SeqCst);
        assert_eq!(
            self.database
                .lock()
                .unwrap()
                .get_pixiv(id)
                .unwrap()
                .credential_revision,
            2
        );
        self.observed
            .resources
            .lock()
            .unwrap()
            .push((id, request.url.clone()));
        if request.url.ends_with("failure.png") {
            return Err(
                Error::new(Reason::RateLimited, "SaveResourceURL").with_retry(RetryAdvice {
                    safe: true,
                    after: Some(chrono::Utc::now() + chrono::TimeDelta::seconds(120)),
                }),
            );
        }
        Ok(ResourceResponse::new(
            200,
            &ResourceHeaders::from([("Content-Type".into(), vec!["image/png".into()])]),
            std::io::Cursor::new(b"\x89PNG\r\n\x1a\nfixture".to_vec()),
        ))
    }
}
struct PoolSaveClient(Arc<Client<PoolTransport>>);
impl DownloadSaveClient for PoolSaveClient {
    fn save_ref(&self, _: Context, _: ResourceRef, _: PathBuf) -> SaveFuture<'_> {
        Box::pin(async { panic!("direct-source regression cannot save opaque references") })
    }
    fn save_url(&self, context: Context, url: String, path: PathBuf) -> SaveFuture<'_> {
        Box::pin(async move {
            tokio::select! {
                biased;
                error = context.cancelled() => Err(error.into()),
                result = self.0.save_resource_url(&url, SaveOptions { path: path.to_string_lossy().into_owned(), ..Default::default() }) => result.map_err(Into::into),
            }
        })
    }
}

#[tokio::test]
async fn mcp_direct_source_failures_stay_in_reports_without_pool_replay_before_or_after_publication()
 {
    for published_prefix in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(
            &path,
            "[account_pool]\nenabled = true\nstrategy = 'round_robin'\n[pixiv.auth]\ndefault_user_id=43\n",
        )
        .unwrap();
        let destination = directory.path().join("downloads");
        std::fs::create_dir(&destination).unwrap();
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
        let observed = Arc::new(Observed::default());
        let factory_database = database.clone();
        let factory_observed = observed.clone();
        let execution = Execution::new(Store::new(path), database.clone(), move |_| {
            Ok(PoolTransport {
                identity: AtomicI64::new(0),
                open_failure: false,
                database: factory_database.clone(),
                observed: factory_observed.clone(),
            })
        });
        let success = "https://i.pximg.net/prefix.png".to_owned();
        let failure = "https://i.pximg.net/failure.png".to_owned();
        let sources = if published_prefix {
            vec![success.clone(), failure.clone()]
        } else {
            vec![failure.clone()]
        };
        let factory: Arc<SaveClientFactory<PoolTransport>> =
            Arc::new(|client| Arc::new(PoolSaveClient(client)));
        let result = saved_download(
            &execution,
            &Context::new(),
            destination.to_str().unwrap(),
            DownloadInput {
                srcs: sources.clone(),
                ..Default::default()
            },
            None,
            factory,
        )
        .await;
        assert!(result.is_error);
        assert_eq!(
            result.structured_content.items.len(),
            usize::from(published_prefix)
        );
        assert_eq!(
            result.structured_content.files.len(),
            usize::from(published_prefix)
        );
        assert_eq!(result.structured_content.failures.len(), 1);
        assert_eq!(
            result.structured_content.failures[0].url,
            "[redacted source]"
        );
        assert_eq!(
            result.structured_content.failures[0].message,
            "pixiv:SaveResourceURL: rate_limited"
        );
        assert!(!result.structured_content.text.contains("Download failed:"));
        assert_eq!(*observed.opens.lock().unwrap(), vec![43]);
        assert_eq!(
            *observed.resources.lock().unwrap(),
            sources
                .into_iter()
                .map(|source| (43, source))
                .collect::<Vec<_>>()
        );
        assert_eq!(observed.closes.load(Ordering::SeqCst), 1);
        let accounts = database.lock().unwrap();
        let first = accounts.get_pixiv(42).unwrap();
        let second = accounts.get_pixiv(43).unwrap();
        assert_eq!(first.credential_revision, 1);
        assert_eq!(second.credential_revision, 2);
        assert!(!first.pool_last_selected);
        assert!(!second.pool_last_selected);
        assert!(first.pool_frozen_until.is_none());
        assert!(second.pool_frozen_until.is_none());
        let files = std::fs::read_dir(&destination)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(files.len(), usize::from(published_prefix));
        if published_prefix {
            assert_eq!(
                std::fs::read(&files[0]).unwrap(),
                b"\x89PNG\r\n\x1a\nfixture"
            );
            assert_eq!(
                result.structured_content.files[0].path,
                files[0].to_str().unwrap()
            );
        }
    }
}

#[tokio::test]
async fn mcp_download_default_selection_and_retryable_open_errors_match_frozen_single_lease_go() {
    let cases: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("fixtures/download_single_lease.json")).unwrap();
    for row in cases
        .into_iter()
        .filter(|row| row["name"] != "ignored-deferred-close-error")
    {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(&path,"[account_pool]\nenabled=true\nstrategy='round_robin'\n[pixiv.auth]\ndefault_user_id=43\n").unwrap();
        let destination = directory.path().join("downloads");
        std::fs::create_dir(&destination).unwrap();
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
        let observed = Arc::new(Observed::default());
        let db = database.clone();
        let capture = observed.clone();
        let open_failure = row["name"] == "retryable-open-failure";
        let execution = Execution::new(Store::new(path), database.clone(), move |_| {
            Ok(PoolTransport {
                identity: AtomicI64::new(0),
                open_failure,
                database: db.clone(),
                observed: capture.clone(),
            })
        });
        let factory: Arc<SaveClientFactory<PoolTransport>> =
            Arc::new(|client| Arc::new(PoolSaveClient(client)));
        let result = saved_download(
            &execution,
            &Context::new(),
            destination.to_str().unwrap(),
            DownloadInput {
                src: "https://i.pximg.net/asset.png".into(),
                ..Default::default()
            },
            None,
            factory,
        )
        .await;
        let mut actual = serde_json::to_value(result).unwrap();
        fn normalize(value: &mut serde_json::Value, root: &str) {
            match value {
                serde_json::Value::String(text) => *text = text.replace(root, "<ROOT>"),
                serde_json::Value::Array(values) => {
                    values.iter_mut().for_each(|value| normalize(value, root))
                }
                serde_json::Value::Object(values) => {
                    values.values_mut().for_each(|value| normalize(value, root))
                }
                _ => {}
            }
        }
        normalize(&mut actual, destination.to_str().unwrap());
        assert_eq!(
            serde_json::json!(*observed.opens.lock().unwrap()),
            row["opens"],
            "{}",
            row["name"]
        );
        assert_eq!(actual, row["result"], "{}", row["name"]);
        assert_eq!(
            observed.resources.lock().unwrap().len(),
            row["resources"].as_u64().unwrap() as usize
        );
        let accounts = database.lock().unwrap();
        let states=[42,43].into_iter().map(|id|{let account=accounts.get_pixiv(id).unwrap();serde_json::json!({"id":id,"revision":account.credential_revision,"frozen":account.pool_frozen_until.is_some(),"selected":account.pool_last_selected})}).collect::<Vec<_>>();
        assert_eq!(serde_json::json!(states), row["states"]);
        assert_eq!(observed.closes.load(Ordering::SeqCst), 1);
    }
}
