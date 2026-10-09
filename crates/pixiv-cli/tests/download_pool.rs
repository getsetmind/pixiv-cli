use pixiv_app::{
    config::Store,
    database::{Database, PixivAccount},
    download::{DownloadSaveClient, SaveFuture},
    execution::Execution,
    lifecycle::Context,
};
use pixiv_cli_rs::{
    CommandError,
    download::{DownloadCommand, DownloadRuntime, DownloadSinks},
};
use pixiv_sdk::{
    Client,
    resource::{ResourceHeaders, ResourceRef, ResourceResponse, SaveOptions},
    transport::{Request, ResourceReadRequest, ResourceTransport, Response, Transport},
};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicI64, Ordering},
    },
};

#[derive(Default)]
struct Observed {
    opens: Vec<i64>,
    metadata: Vec<(i64, i64)>,
    downloads: Vec<(i64, String)>,
}
#[derive(Clone)]
struct Fixture {
    id: Arc<AtomicI64>,
    observed: Arc<Mutex<Observed>>,
}
impl Transport for Fixture {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        if request.operation == "Open" {
            let token = request
                .parameters
                .iter()
                .find(|(key, _)| key == "refresh_token")
                .unwrap()
                .1
                .as_str();
            let id = token.rsplit('-').next().unwrap().parse::<i64>().unwrap();
            self.id.store(id, Ordering::SeqCst);
            self.observed.lock().unwrap().opens.push(id);
            return Ok(Response {
                status: 200,
                retry_after: None,
                body: serde_json::json!({"access_token":format!("fixture-access-{id}"),"refresh_token":format!("fixture-rotated-{id}"),"expires_in":3600,"user":{"id":id}}),
            });
        }
        assert_eq!(request.url, "https://app-api.pixiv.net/v1/ugoira/metadata");
        let artwork = request
            .parameters
            .iter()
            .find(|(key, _)| key == "illust_id")
            .unwrap()
            .1
            .parse::<i64>()
            .unwrap();
        let id = self.id.load(Ordering::SeqCst);
        let count = {
            let mut observed = self.observed.lock().unwrap();
            observed.metadata.push((id, artwork));
            observed
                .metadata
                .iter()
                .filter(|value| **value == (id, artwork))
                .count()
        };
        let rate_limited = id == 42 && artwork == 22;
        Ok(Response {
            status: if rate_limited { 429 } else { 200 },
            retry_after: rate_limited
                .then(|| chrono::TimeDelta::seconds(if count % 2 == 1 { 0 } else { 120 })),
            body: serde_json::json!({"ugoira_metadata":{"zip_urls":{"original":format!("https://i.pximg.net/{artwork}.zip")},"frames":[{"file":"0.jpg","delay":10}]}}),
        })
    }
}
impl ResourceTransport for Fixture {
    type Body = std::io::Cursor<Vec<u8>>;
    async fn open_resource(
        &self,
        request: ResourceReadRequest,
    ) -> pixiv_sdk::Result<ResourceResponse<Self::Body>> {
        assert!(!request.headers.contains_key("Authorization"));
        self.observed
            .lock()
            .unwrap()
            .downloads
            .push((self.id.load(Ordering::SeqCst), request.url));
        Ok(ResourceResponse::new(
            200,
            &ResourceHeaders::from([("Content-Length".into(), vec!["7".into()])]),
            std::io::Cursor::new(b"payload".to_vec()),
        ))
    }
}
struct SaveClient(Arc<Client<Fixture>>);
impl DownloadSaveClient for SaveClient {
    fn save_ref(&self, _context: Context, reference: ResourceRef, path: PathBuf) -> SaveFuture<'_> {
        Box::pin(async move {
            self.0
                .save_resource(
                    reference,
                    SaveOptions {
                        path: path.to_string_lossy().into_owned(),
                        progress: None,
                    },
                )
                .await
                .map_err(Into::into)
        })
    }
    fn save_url(&self, _context: Context, url: String, path: PathBuf) -> SaveFuture<'_> {
        Box::pin(async move {
            self.0
                .save_resource_url(
                    &url,
                    SaveOptions {
                        path: path.to_string_lossy().into_owned(),
                        progress: None,
                    },
                )
                .await
                .map_err(Into::into)
        })
    }
}
fn reference(id: i64) -> String {
    ResourceRef::new(
        "pixiv",
        &serde_json::to_vec(
            &serde_json::json!({"k":"ugoira_archive","id":id,"p":-1,"v":"original"}),
        )
        .unwrap(),
    )
    .unwrap()
    .to_string()
}

#[tokio::test]
async fn saved_cli_download_replays_only_a_typed_text_failure_before_publication() {
    for (prefix, json) in [(false, false), (true, false), (false, true), (true, true)] {
        let account_dir = tempfile::tempdir().unwrap();
        let download_dir = tempfile::tempdir().unwrap();
        let path = account_dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[account_pool]\nenabled=true\nstrategy='round_robin'\n",
        )
        .unwrap();
        let mut database = Database::open(account_dir.path()).unwrap();
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
        let observed = Arc::new(Mutex::new(Observed::default()));
        let transport_observed = observed.clone();
        let execution = Execution::new(Store::new(path), database.clone(), move |_| {
            Ok(Fixture {
                id: Arc::new(AtomicI64::new(0)),
                observed: transport_observed.clone(),
            })
        });
        let mut args = vec!["download".into()];
        if prefix {
            args.push(reference(11));
        }
        args.push(reference(22));
        if json {
            args.push("--json".into());
        }
        let command = DownloadCommand::parse(&args, &mut &b""[..], false).unwrap();
        let out = Arc::new(Mutex::new(Vec::new()));
        let err = Arc::new(Mutex::new(Vec::new()));
        let result = command
            .execute_with_factory(
                &Context::new(),
                || {
                    Ok(DownloadRuntime {
                        download_path: download_dir.path().to_string_lossy().into_owned(),
                        ..Default::default()
                    })
                },
                &mut &b""[..],
                DownloadSinks {
                    output: out.clone(),
                    error: err.clone(),
                },
                || Ok(execution),
                |client| Arc::new(SaveClient(client)),
            )
            .await;
        let observed = observed.lock().unwrap();
        let replay = !prefix && !json;
        assert_eq!(
            observed.opens,
            if replay { vec![42, 43] } else { vec![42] },
            "prefix={prefix} json={json}"
        );
        assert_eq!(
            observed.metadata,
            if replay {
                vec![(42, 22), (42, 22), (43, 22)]
            } else if prefix {
                vec![(42, 11), (42, 22), (42, 22)]
            } else {
                vec![(42, 22), (42, 22)]
            }
        );
        assert_eq!(observed.downloads.len(), usize::from(prefix || replay));
        let database = database.lock().unwrap();
        let first = database.get_pixiv(42).unwrap();
        let second = database.get_pixiv(43).unwrap();
        assert_eq!(first.credential_revision, 2);
        assert_eq!(second.credential_revision, if replay { 2 } else { 1 });
        assert_eq!(first.pool_frozen_until.is_some(), replay);
        assert!(second.pool_frozen_until.is_none());
        let files = std::fs::read_dir(download_dir.path())
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(files.len(), usize::from(prefix || replay));
        for file in files {
            assert_eq!(std::fs::read(file.path()).unwrap(), b"payload");
        }
        assert!(err.lock().unwrap().is_empty());
        if replay {
            assert!(result.is_ok());
            assert!(out.lock().unwrap().is_empty());
        } else if json {
            assert!(matches!(result, Err(CommandError::Pipeline)));
            let records: serde_json::Value = serde_json::from_slice(&out.lock().unwrap()).unwrap();
            assert_eq!(records.as_array().unwrap().len(), 1 + usize::from(prefix));
            assert_eq!(
                records.as_array().unwrap().last().unwrap()["error"]["code"],
                "rate_limited"
            );
        } else {
            let Err(CommandError::App(error)) = result else {
                panic!("typed text failure is required")
            };
            assert_eq!(
                error.classified().unwrap().code,
                pixiv_sdk::Reason::RateLimited
            );
            assert!(
                error
                    .to_string()
                    .starts_with("download completed with 1 failures:")
            );
            assert!(out.lock().unwrap().is_empty());
        }
    }
}
