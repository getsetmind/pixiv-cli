use pixiv_app::{
    config::Store,
    database::{Database, PixivAccount},
    execution::Execution,
};
use pixiv_sdk::transport::{JsonResponse, Request, Response, Transport};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicI64, Ordering},
};

#[derive(Clone)]
pub struct SavedTransport<T> {
    inner: T,
    user_id: i64,
    database: Arc<Mutex<Database>>,
    opens: Arc<AtomicI64>,
}

impl<T: Transport> Transport for SavedTransport<T> {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        if request.operation == "Open" {
            let opens = self.opens.fetch_add(1, Ordering::SeqCst);
            let account = self
                .database
                .lock()
                .unwrap()
                .get_pixiv(self.user_id)
                .unwrap();
            assert_eq!(account.credential_revision, opens + 1);
            let expected = if opens == 0 {
                format!("fixture-refresh-{}", self.user_id)
            } else {
                format!("fixture-rotated-{}", self.user_id)
            };
            assert_eq!(account.refresh_token_copy(), expected.as_bytes());
            assert!(
                request
                    .parameters
                    .iter()
                    .any(|(key, value)| key == "refresh_token" && value == &expected)
            );
            return Ok(Response {
                status: 200,
                retry_after: None,
                body: serde_json::json!({"access_token":format!("fixture-access-{}",self.user_id),"refresh_token":format!("fixture-rotated-{}",self.user_id),"expires_in":3600,"user":{"id":self.user_id}}),
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
        let account = self
            .database
            .lock()
            .unwrap()
            .get_pixiv(self.user_id)
            .unwrap();
        assert_eq!(
            account.credential_revision,
            self.opens.load(Ordering::SeqCst) + 1
        );
        assert_eq!(
            account.refresh_token_copy(),
            format!("fixture-rotated-{}", self.user_id).as_bytes()
        );
        assert!(
            request
                .headers
                .iter()
                .any(|(key, value)| key.eq_ignore_ascii_case("authorization")
                    && value == &format!("Bearer fixture-access-{}", self.user_id))
        );
    }
}

pub struct SavedExecution<T> {
    pub execution: Execution<SavedTransport<T>>,
    _directory: tempfile::TempDir,
}

pub fn saved_execution<T: Transport + Clone + 'static>(inner: T) -> SavedExecution<T> {
    saved_execution_for_user(inner, 42)
}

pub fn saved_execution_for_user<T: Transport + Clone + 'static>(
    inner: T,
    user_id: i64,
) -> SavedExecution<T> {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    std::fs::write(&path, "").unwrap();
    let mut database = Database::open(directory.path()).unwrap();
    database
        .save_pixiv_credential(&PixivAccount::new(
            user_id,
            "fixture",
            format!("fixture-refresh-{user_id}").as_bytes(),
        ))
        .unwrap();
    let database = Arc::new(Mutex::new(database));
    let factory_database = database.clone();
    let opens = Arc::new(AtomicI64::new(0));
    let execution = Execution::new(Store::new(path), database, move |_| {
        Ok(SavedTransport {
            inner: inner.clone(),
            user_id,
            database: factory_database.clone(),
            opens: opens.clone(),
        })
    });
    SavedExecution {
        execution,
        _directory: directory,
    }
}
