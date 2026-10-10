use pixiv_app::{
    reverse_search::http::{HttpRequest, HttpTransport},
    update::{CallerContext, ExternalError, UpdateFuture, http::ClientTransport},
};
use pixiv_sdk::{
    context::Context,
    fanbox::transport::{Headers, RawResponse},
};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Transport {
    requests: Mutex<Vec<(String, String, Headers)>>,
    responses: Mutex<std::collections::VecDeque<RawResponse>>,
}
impl HttpTransport for Transport {
    fn send(
        &self,
        request: HttpRequest,
    ) -> UpdateFuture<'_, Result<Option<RawResponse>, ExternalError>> {
        self.requests
            .lock()
            .unwrap()
            .push((request.method, request.url, request.headers));
        let response = self.responses.lock().unwrap().pop_front();
        Box::pin(async move { Ok(response) })
    }
}
fn request(url: &str) -> HttpRequest {
    HttpRequest {
        method: "GET".into(),
        url: url.into(),
        logical_host: None,
        headers: Headers::new(),
        body: None,
        content_length: 0,
        context: Arc::new(Context::background()) as CallerContext,
    }
}
fn response(status: u16, location: &str) -> RawResponse {
    let mut headers = Headers::new();
    if !location.is_empty() {
        headers.insert("Location".into(), vec![location.into()]);
    }
    RawResponse {
        status,
        headers,
        content_length: 0,
        body: None,
    }
}
#[tokio::test]
async fn ordinary_redirects_follow_go_limit_and_referer_policy() {
    let raw = Arc::new(Transport::default());
    raw.responses
        .lock()
        .unwrap()
        .extend([response(302, "/next"), response(200, "")]);
    let client = ClientTransport::new(raw.clone());
    let mut result = client
        .send(request("https://example.invalid/start#frag"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.status, 200);
    result.body.as_mut().unwrap().close().await.unwrap();
    let requests = raw.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].1, "https://example.invalid/next");
    assert_eq!(
        requests[1].2.get("Referer").unwrap(),
        &vec!["https://example.invalid/start".to_string()]
    );
}
#[tokio::test]
async fn downgrade_redirect_preserves_explicit_referer_but_drops_cross_domain_secrets() {
    let raw = Arc::new(Transport::default());
    raw.responses.lock().unwrap().extend([
        response(302, "http://elsewhere.invalid/end"),
        response(200, ""),
    ]);
    let client = ClientTransport::new(raw.clone());
    let mut initial = request("https://example.invalid/start");
    initial
        .headers
        .insert("Authorization".into(), vec!["synthetic".into()]);
    initial
        .headers
        .insert("Referer".into(), vec!["https://owned.invalid/".into()]);
    assert_eq!(client.send(initial).await.unwrap().unwrap().status, 200);
    let requests = raw.requests.lock().unwrap();
    assert_eq!(
        requests[1].2.get("Referer").unwrap(),
        &vec!["https://owned.invalid/".to_string()]
    );
    assert!(!requests[1].2.contains_key("Authorization"));
}
#[tokio::test]
async fn ten_redirects_stop_before_an_eleventh_request() {
    let raw = Arc::new(Transport::default());
    for _ in 0..10 {
        raw.responses
            .lock()
            .unwrap()
            .push_back(response(302, "/loop"));
    }
    let error = ClientTransport::new(raw.clone())
        .send(request("https://example.invalid/start"))
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Get \"/loop\": stopped after 10 redirects"
    );
    assert_eq!(raw.requests.lock().unwrap().len(), 10);
}
#[tokio::test]
async fn missing_location_remains_a_response_for_the_caller() {
    let raw = Arc::new(Transport::default());
    raw.responses.lock().unwrap().push_back(response(302, ""));
    assert_eq!(
        ClientTransport::new(raw)
            .send(request("https://example.invalid/start"))
            .await
            .unwrap()
            .unwrap()
            .status,
        302
    );
}

#[tokio::test]
async fn frozen_standard_http_client_nil_and_redirect_diagnostics_keep_whole_error_chains() {
    let observations: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("support/updater_http_go_oracle.json")).unwrap();
    for row in observations {
        let raw = Arc::new(Transport::default());
        match row["mode"].as_str().unwrap() {
            "nil" => {}
            "positive" => {
                let mut response = response(200, "");
                response.content_length = 1;
                raw.responses.lock().unwrap().push_back(response);
            }
            _ => raw.responses.lock().unwrap().extend([
                response(302, row["location"].as_str().unwrap()),
                response(200, ""),
            ]),
        }
        let mut input = request("https://example.invalid/start");
        input.method = row["method"].as_str().unwrap().into();
        let outcome = ClientTransport::new(raw.clone()).send(input).await;
        let (status, errors) = match outcome {
            Ok(Some(response)) => (response.status, Vec::new()),
            Ok(None) => panic!("HTTPClient fills or rejects missing response"),
            Err(error) => {
                let mut errors = vec![error.to_string()];
                let mut cause = error.source();
                while let Some(value) = cause {
                    errors.push(value.to_string());
                    cause = value.source();
                }
                (0, errors)
            }
        };
        let expected: Vec<String> = row["errors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .unwrap()
                    .replace("(*main.transport)", "(HttpTransport)")
            })
            .collect();
        assert_eq!(errors, expected, "{}", row["name"]);
        assert_eq!(
            status as u64,
            row["status"].as_u64().unwrap(),
            "{}",
            row["name"]
        );
        assert_eq!(
            raw.requests.lock().unwrap().len() as u64,
            row["requests"].as_u64().unwrap(),
            "{}",
            row["name"]
        );
    }
}
