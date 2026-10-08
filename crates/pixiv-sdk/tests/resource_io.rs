use base64::{Engine, engine::general_purpose::STANDARD};
use pixiv_sdk::resource::{OpenResourceRequest, ResourceHeaders, ResourceResponse};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    io::{self, Cursor},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncReadExt, ReadBuf};
#[derive(Deserialize)]
struct RequestCase {
    method: String,
    range: String,
    if_none_match: String,
    if_modified_since: String,
    if_range: String,
    reason: String,
    message: String,
}
#[derive(Deserialize)]
struct ResponseCase {
    status: i64,
    input: BTreeMap<String, Option<Vec<String>>>,
    headers: ResourceHeaders,
    content_type: String,
    content_length: i64,
    content_range: String,
    accept_ranges: String,
    etag: String,
    last_modified: String,
    cache_control: String,
    body: String,
}
#[derive(Deserialize)]
struct Contract {
    requests: Vec<RequestCase>,
    responses: Vec<ResponseCase>,
}
fn contract() -> Contract {
    serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/resource-io.json"
    ))
    .unwrap()
}
#[test]
fn resource_requests_match_go_method_validation_and_every_control_byte() {
    let cases = contract().requests;
    assert_eq!(cases.len(), 144);
    for case in cases {
        let request = OpenResourceRequest {
            method: case.method,
            range: case.range,
            if_none_match: case.if_none_match,
            if_modified_since: case.if_modified_since,
            if_range: case.if_range,
            ..OpenResourceRequest::default()
        };
        let result = request.validate();
        if case.reason.is_empty() {
            result.unwrap();
        } else {
            let error = result.unwrap_err();
            assert_eq!(error.code.as_str(), case.reason);
            assert_eq!(error.to_string(), case.message);
        }
    }
}
struct OwnedStream {
    reader: Cursor<Vec<u8>>,
    polls: Arc<AtomicUsize>,
    drops: Arc<AtomicUsize>,
}
impl AsyncRead for OwnedStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.polls.fetch_add(1, Ordering::SeqCst);
        Pin::new(&mut self.reader).poll_read(context, buffer)
    }
}
impl Drop for OwnedStream {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn dropping_an_unread_resource_response_releases_its_stream_without_polling() {
    let polls = Arc::new(AtomicUsize::new(0));
    let drops = Arc::new(AtomicUsize::new(0));
    let stream = OwnedStream {
        reader: Cursor::new(vec![1, 2, 3]),
        polls: polls.clone(),
        drops: drops.clone(),
    };
    let response = ResourceResponse::new(200, &ResourceHeaders::new(), stream);
    let _ = response.header();
    let _ = response.content_length();
    assert_eq!(polls.load(Ordering::SeqCst), 0);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    drop(response);
    assert_eq!(polls.load(Ordering::SeqCst), 0);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn resource_responses_match_go_allowlisting_accessors_and_stream_ownership() {
    let cases = contract().responses;
    assert_eq!(cases.len(), 18);
    for case in cases {
        let polls = Arc::new(AtomicUsize::new(0));
        let drops = Arc::new(AtomicUsize::new(0));
        let expected = STANDARD.decode(&case.body).unwrap();
        let stream = OwnedStream {
            reader: Cursor::new(expected.clone()),
            polls: polls.clone(),
            drops: drops.clone(),
        };
        let mut input: ResourceHeaders = case
            .input
            .into_iter()
            .map(|(key, values)| (key, values.unwrap_or_default()))
            .collect();
        let mut response = ResourceResponse::new(case.status, &input, stream);
        assert_eq!(polls.load(Ordering::SeqCst), 0);
        input.insert("Content-Type".into(), vec!["mutated-source".into()]);
        assert_eq!(response.status_code, case.status);
        assert_eq!(response.header(), case.headers);
        assert_eq!(response.content_type(), case.content_type);
        assert_eq!(response.content_length(), case.content_length);
        assert_eq!(response.content_range(), case.content_range);
        assert_eq!(response.accept_ranges(), case.accept_ranges);
        assert_eq!(response.etag(), case.etag);
        assert_eq!(response.last_modified(), case.last_modified);
        assert_eq!(response.cache_control(), case.cache_control);
        let mut copy = response.header();
        copy.insert("Content-Type".into(), vec!["mutated-copy".into()]);
        assert_eq!(response.content_type(), case.content_type);
        let debug = format!("{response:?}");
        assert!(!debug.contains("secret") && !debug.contains("fixture-body"));
        let mut body = Vec::new();
        response.body.read_to_end(&mut body).await.unwrap();
        assert_eq!(body, expected);
        assert!(polls.load(Ordering::SeqCst) > 0);
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        drop(response);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }
}
