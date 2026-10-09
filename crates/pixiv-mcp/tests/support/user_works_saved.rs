use pixiv_app::{
    config::Store,
    database::{Database, PixivAccount},
    execution::Execution,
};
use pixiv_sdk::transport::{JsonResponse, Request, Response, Transport};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct SavedTransport<T> {
    inner: T,
    database: Arc<Mutex<Database>>,
    opens: Arc<std::sync::atomic::AtomicUsize>,
}

impl<T: Transport> Transport for SavedTransport<T> {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        if request.operation == "Open" {
            self.opens.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let account = self.database.lock().unwrap().get_pixiv(42).unwrap();
            assert!(
                request
                    .parameters
                    .iter()
                    .any(|(key, value)| key == "refresh_token"
                        && value.as_bytes() == account.refresh_token_copy())
            );
            return Ok(Response {
                status: 200,
                retry_after: None,
                body: serde_json::json!({"access_token":"fixture-access-42","refresh_token":"fixture-rotated-42","expires_in":3600,"user":{"id":42}}),
            });
        }
        self.assert_saved_credentials(&request);
        self.inner.send(request).await
    }

    async fn send_json(&self, request: Request) -> pixiv_sdk::Result<JsonResponse> {
        if request.operation == "Open" {
            let response = self.send(request).await?;
            return Ok(JsonResponse {
                status: response.status,
                retry_after: response.retry_after,
                body: serde_json::to_vec(&response.body).unwrap(),
            });
        }
        self.assert_saved_credentials(&request);
        self.inner.send_json(request).await
    }
}

impl<T> SavedTransport<T> {
    fn assert_saved_credentials(&self, request: &Request) {
        let account = self.database.lock().unwrap().get_pixiv(42).unwrap();
        assert!(account.credential_revision >= 2);
        assert_eq!(account.refresh_token_copy(), b"fixture-rotated-42");
        assert!(
            request
                .headers
                .iter()
                .any(|(key, value)| key.eq_ignore_ascii_case("authorization")
                    && value == "Bearer fixture-access-42")
        );
    }
}

pub struct SavedExecution<T> {
    pub execution: Execution<SavedTransport<T>>,
    pub _opens: Arc<std::sync::atomic::AtomicUsize>,
    _directory: tempfile::TempDir,
}

pub fn saved_execution<T: Transport + Clone + 'static>(inner: T) -> SavedExecution<T> {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    std::fs::write(&path, "").unwrap();
    let mut database = Database::open(directory.path()).unwrap();
    database
        .save_pixiv_credential(&PixivAccount::new(42, "fixture", b"fixture-refresh-42"))
        .unwrap();
    let database = Arc::new(Mutex::new(database));
    let factory_database = database.clone();
    let opens = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let factory_opens = opens.clone();
    let execution = Execution::new(Store::new(path), database, move |_| {
        Ok(SavedTransport {
            inner: inner.clone(),
            database: factory_database.clone(),
            opens: factory_opens.clone(),
        })
    });
    SavedExecution {
        execution,
        _opens: opens,
        _directory: directory,
    }
}
