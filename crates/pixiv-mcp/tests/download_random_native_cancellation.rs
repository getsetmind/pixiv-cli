use pixiv_app::{
    config::Store,
    database::{Database, PixivAccount},
    execution::Execution,
    lifecycle::Context,
};
use pixiv_mcp::{
    download::{
        DownloadDefaults, DownloadRandomInput, SaveClientFactory,
        saved_download_random_with_account,
    },
    runtime::Account,
};
use pixiv_sdk::transport::{HttpTransport, JsonResponse, Request, Response, Transport};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{io::AsyncReadExt, net::TcpListener};

#[derive(Default)]
struct Observed {
    opens: AtomicUsize,
    recommendations: Mutex<Vec<Value>>,
}

struct NativeRecommendationTransport {
    native: HttpTransport,
    observed: Arc<Observed>,
}

impl NativeRecommendationTransport {
    fn open(&self, request: &Request) -> Response {
        assert_eq!(request.operation, "Open");
        assert_eq!(request.method.as_str(), "POST");
        assert_eq!(request.url, "https://oauth.secure.pixiv.net/auth/token");
        assert!(
            request
                .parameters
                .iter()
                .any(|(key, value)| key == "refresh_token" && value == "fixture-refresh-43")
        );
        self.observed.opens.fetch_add(1, Ordering::SeqCst);
        Response {
            status: 200,
            retry_after: None,
            body: json!({"access_token":"fixture-access-43","refresh_token":"fixture-rotated-43","expires_in":3600,"user":{"id":43}}),
        }
    }
    fn recommendation(&self, request: &Request) {
        assert_eq!(request.operation, "RecommendedArtworks");
        assert_eq!(request.method.as_str(), "GET");
        assert_eq!(
            request.url,
            "https://app-api.pixiv.net/v1/illust/recommended"
        );
        assert!(request.parameters.is_empty());
        let authorization = request
            .headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case("Authorization"))
            .map(|(_, value)| value.as_str())
            .unwrap_or("");
        self.observed.recommendations.lock().unwrap().push(json!({"method":request.method.as_str(),"url":request.url,"authorization":authorization}));
    }
}

impl Transport for NativeRecommendationTransport {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        if request.operation == "Open" {
            return Ok(self.open(&request));
        }
        self.recommendation(&request);
        self.native.send(request).await
    }
    async fn send_json(&self, request: Request) -> pixiv_sdk::Result<JsonResponse> {
        if request.operation == "Open" {
            let response = self.open(&request);
            return Ok(JsonResponse {
                status: response.status,
                retry_after: response.retry_after,
                body: serde_json::to_vec(&response.body).unwrap(),
            });
        }
        self.recommendation(&request);
        self.native.send_json(request).await
    }
}

#[tokio::test(flavor = "current_thread")]
async fn saved_random_pending_native_connect_preserves_frozen_handler_results() {
    let bytes = include_bytes!("fixtures/download_random_native_cancellation.json");
    assert_eq!(
        format!("{:x}", Sha256::digest(bytes)),
        "ffee43b038521aaaf1798a3648dd0231cdb03645cfd8891f8c9bbe41e704a9f7"
    );
    let fixture: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        fixture["reference"],
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 2);
    for case in fixture["cases"].as_array().unwrap() {
        tokio::time::timeout(Duration::from_secs(5), compare_case(case))
            .await
            .expect("owned native recommendation cancellation exceeded bounded wait");
    }
}

async fn compare_case(case: &Value) {
    let mode = case["mode"].as_str().unwrap();
    assert!(matches!(mode, "cancel" | "deadline"));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = format!("http://{}", listener.local_addr().unwrap());
    let native = HttpTransport::new(Some(&proxy)).unwrap();
    let observed = Arc::new(Observed::default());
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.toml");
    std::fs::write(
        &config,
        "[pixiv.auth]\ndefault_user_id=43\n[account_pool]\nenabled=true\nstrategy='round_robin'\n",
    )
    .unwrap();
    let database = Arc::new(Mutex::new(Database::open(directory.path()).unwrap()));
    database
        .lock()
        .unwrap()
        .save_pixiv_credential(&PixivAccount::new(43, "fixture", b"fixture-refresh-43"))
        .unwrap();
    let factory_observed = Arc::clone(&observed);
    let execution = Execution::new(Store::new(config), Arc::clone(&database), move |_| {
        Ok(NativeRecommendationTransport {
            native: native.clone(),
            observed: Arc::clone(&factory_observed),
        })
    });
    let context = if mode == "deadline" {
        Context::with_deadline(Instant::now() + Duration::from_millis(500))
    } else {
        Context::new()
    };
    let defaults = DownloadDefaults {
        download_path: directory
            .path()
            .join("unreachable-media")
            .to_string_lossy()
            .into_owned(),
        ..Default::default()
    };
    let factory: Arc<SaveClientFactory<NativeRecommendationTransport>> =
        Arc::new(|_| panic!("media factory reached while recommendation CONNECT is canceled"));
    let account = Account::default();
    let mut operation = Box::pin(saved_download_random_with_account(
        &execution,
        &context,
        &defaults,
        DownloadRandomInput::default(),
        &account,
        factory,
    ));
    let (mut socket, _) = tokio::select! {
        result=&mut operation=>panic!("handler finished before native CONNECT: {result:?}"),
        connection=listener.accept()=>connection.unwrap(),
    };
    let mut bytes = vec![];
    loop {
        let mut buffer = [0; 1024];
        let count = tokio::select! {
            result=&mut operation=>panic!("handler finished before native CONNECT headers: {result:?}"),
            read=socket.read(&mut buffer)=>read.unwrap(),
        };
        assert!(count > 0, "proxy peer closed before CONNECT headers");
        bytes.extend_from_slice(&buffer[..count]);
        assert!(bytes.len() < 8192, "unbounded CONNECT headers");
        if bytes.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let headers = String::from_utf8(bytes).unwrap();
    let connect_line = headers.lines().next().unwrap();
    assert_eq!(connect_line, "CONNECT app-api.pixiv.net:443 HTTP/1.1");
    if mode == "cancel" {
        context.cancel();
    }
    let result = operation.await;
    let request = {
        let requests = observed.recommendations.lock().unwrap();
        assert_eq!(requests.len(), 1, "recommendation request replayed");
        requests[0].clone()
    };
    let actual = json!({"request":request,"connect_line":connect_line,"parent_error":context.error().map(|error|error.to_string()).unwrap_or_default(),"result":serde_json::to_value(&result).unwrap()});
    let mut expected = case["expected"].clone();
    expected
        .as_object_mut()
        .unwrap()
        .remove("go_sdk_observation");
    expected
        .as_object_mut()
        .unwrap()
        .remove("go_peer_closed_within_1s");
    assert_eq!(actual, expected, "{mode}");
    assert_eq!(
        observed.opens.load(Ordering::SeqCst),
        1,
        "OAuth was replayed"
    );
    assert_eq!(
        database
            .lock()
            .unwrap()
            .get_pixiv(43)
            .unwrap()
            .credential_revision,
        2,
        "refresh was not persisted before recommendation"
    );
    assert!(
        !std::path::Path::new(&defaults.download_path).exists(),
        "media filesystem effects occurred"
    );
    let mut one = [0; 1];
    let rust_peer_closed =
        match tokio::time::timeout(Duration::from_secs(1), socket.read(&mut one)).await {
            Ok(Ok(0)) => true,
            Err(_) => false,
            other => panic!("unexpected native peer observation: {other:?}"),
        };
    println!(
        "native lifetime observation: mode={mode} rust_peer_closed_within_1s={rust_peer_closed} go_peer_closed_within_1s={}",
        case["expected"]["go_peer_closed_within_1s"]
    );
    drop(socket);
    drop(listener);
    drop(execution);
}
