use pixiv_app::{
    config::Store,
    database::{Database, PixivAccount},
    execution::Execution,
};
use pixiv_sdk::transport::{Request, Response, Transport};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct SavedTransport<T> {
    inner: T,
    database: Arc<Mutex<Database>>,
}

impl<T: Transport> Transport for SavedTransport<T> {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        if request.operation == "Open" {
            assert!(
                request
                    .parameters
                    .iter()
                    .any(|(key, value)| key == "refresh_token" && value == "fixture-refresh-42")
            );
            return Ok(Response {
                status: 200,
                retry_after: None,
                body: serde_json::json!({"access_token":"fixture-access-42","refresh_token":"fixture-rotated-42","expires_in":3600,"user":{"id":42}}),
            });
        }
        let account = self.database.lock().unwrap().get_pixiv(42).unwrap();
        assert_eq!(account.credential_revision, 2);
        assert_eq!(account.refresh_token_copy(), b"fixture-rotated-42");
        assert!(
            request
                .headers
                .iter()
                .any(|(key, value)| key.eq_ignore_ascii_case("authorization")
                    && value == "Bearer fixture-access-42")
        );
        self.inner.send(request).await
    }
}

pub struct SavedExecution<T> {
    pub execution: Execution<SavedTransport<T>>,
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
    let execution = Execution::new(Store::new(path), database, move |_| {
        Ok(SavedTransport {
            inner: inner.clone(),
            database: factory_database.clone(),
        })
    });
    SavedExecution {
        execution,
        _directory: directory,
    }
}
