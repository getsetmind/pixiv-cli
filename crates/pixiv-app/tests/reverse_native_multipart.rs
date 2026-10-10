use pixiv_app::reverse_search::{
    ASCII2DClient, CallerContext, Loader, Provider, SourceLoader, SourceLoaderOptions,
    ascii2d::{Client, Options},
    http::NativeHttpTransport,
};
use pixiv_sdk::{
    context::{Context, ContextError, ContextKey},
    fanbox::transport::{
        BodyFuture, ExternalError, Headers, RawBody, RawRead, RawRequest, RawResponse,
        RawTransport, TransportFuture,
    },
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::Read,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn decode_hex(value: &str) -> Vec<u8> {
    assert_eq!(value.len() % 2, 0);
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}
fn header<'a>(headers: &'a Headers, name: &str) -> &'a str {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .and_then(|(_, values)| values.first())
        .map(String::as_str)
        .unwrap_or("")
}
fn replace_boundary(bytes: &[u8], boundary: &[u8]) -> Vec<u8> {
    let mut canonical = Vec::with_capacity(bytes.len());
    let mut position = 0;
    while position < bytes.len() {
        if bytes[position..].starts_with(boundary) {
            canonical.extend_from_slice(b"<GENERATED_BOUNDARY>");
            position += boundary.len();
        } else {
            canonical.push(bytes[position]);
            position += 1;
        }
    }
    canonical
}
fn disposition_parameter<'a>(value: &'a str, name: &str) -> &'a str {
    value
        .split("; ")
        .skip(1)
        .find_map(|parameter| {
            let (key, value) = parameter.split_once('=')?;
            (key == name).then(|| value.strip_prefix('"').unwrap().strip_suffix('"').unwrap())
        })
        .unwrap_or("")
}
fn observe_multipart(request: &RawRequest, image: &[u8], token: &[u8]) -> Value {
    let content_type = header(&request.headers, "Content-Type");
    let boundary = content_type
        .strip_prefix("multipart/form-data; boundary=")
        .expect("complete multipart Content-Type");
    assert_eq!(boundary.len(), 60);
    assert!(
        boundary
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    );
    assert_eq!(decode_hex(boundary).len(), 30);
    let bytes = request.body.as_deref().expect("native raw body Vec");
    let delimiter = format!("--{boundary}\r\n");
    let next_delimiter = format!("\r\n--{boundary}");
    let mut position = 0;
    let mut parts = Vec::new();
    for expected in [token, image] {
        let start = if position == 0 {
            delimiter.as_bytes().to_vec()
        } else {
            format!("\r\n{delimiter}").into_bytes()
        };
        assert!(bytes[position..].starts_with(&start));
        position += start.len();
        let header_length = bytes[position..]
            .windows(4)
            .position(|bytes| bytes == b"\r\n\r\n")
            .expect("complete MIME header terminator");
        let raw_headers = std::str::from_utf8(&bytes[position..position + header_length]).unwrap();
        let mut headers = BTreeMap::new();
        for line in raw_headers.split("\r\n") {
            let (name, value) = line.split_once(": ").unwrap();
            assert!(
                headers
                    .insert(name.to_owned(), vec![value.to_owned()])
                    .is_none()
            );
        }
        position += header_length + 4;
        let payload_length = bytes[position..]
            .windows(next_delimiter.len())
            .position(|bytes| bytes == next_delimiter.as_bytes())
            .expect("complete multipart delimiter");
        let payload = &bytes[position..position + payload_length];
        assert_eq!(
            payload, expected,
            "every original owned part byte must survive"
        );
        let disposition = &headers["Content-Disposition"][0];
        let media_type = headers
            .get("Content-Type")
            .map(|values| values[0].as_str())
            .unwrap_or("");
        parts.push(json!({"name":disposition_parameter(disposition,"name"),"filename":disposition_parameter(disposition,"filename"),"content_type":media_type,"headers":headers,"size":payload.len(),"sha256":sha256(payload)}));
        position += payload_length;
    }
    assert_eq!(
        &bytes[position..],
        format!("\r\n--{boundary}--\r\n").as_bytes()
    );
    let expected_headers = [
        "Content-Disposition: form-data; name=\"authenticity_token\"",
        "Content-Disposition: form-data; name=\"file\"; filename=\"image.png\"\r\nContent-Type: image/png",
    ];
    let mut complete = Vec::new();
    for (index, payload) in [token, image].iter().enumerate() {
        if index > 0 {
            complete.extend_from_slice(b"\r\n");
        }
        complete.extend_from_slice(delimiter.as_bytes());
        complete.extend_from_slice(expected_headers[index].as_bytes());
        complete.extend_from_slice(b"\r\n\r\n");
        complete.extend_from_slice(payload);
    }
    complete.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    assert_eq!(bytes, complete, "exact framing and raw ordered headers");
    assert!(bytes.len() > 10 * 1024 * 1024 + 64 * 1024);
    json!({"media_type":"multipart/form-data","parameters":{"boundary":"<GENERATED_BOUNDARY>"},"boundary_length":boundary.len(),"boundary_decoded_length":decode_hex(boundary).len(),"boundary_lower_hex":boundary==boundary.to_lowercase(),"wire_size":bytes.len(),"canonical_sha256":sha256(&replace_boundary(bytes,boundary.as_bytes())),"complete_framing":true,"image_matches_snapshot":true,"token_matches_form":true,"exceeds_old_native_limit":true,"parts":parts})
}

struct ResponseBody {
    bytes: Vec<u8>,
    position: usize,
    closes: Arc<AtomicUsize>,
}
impl RawBody for ResponseBody {
    fn read<'a>(&'a mut self, output: &'a mut [u8]) -> BodyFuture<'a, RawRead> {
        Box::pin(async move {
            let count = output.len().min(self.bytes.len() - self.position);
            output[..count].copy_from_slice(&self.bytes[self.position..self.position + count]);
            self.position += count;
            RawRead {
                count,
                eof: self.position == self.bytes.len(),
                error: None,
            }
        })
    }
    fn close(&mut self) -> BodyFuture<'_, Result<(), ExternalError>> {
        self.closes.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }
}
struct SDKRawPort {
    name: String,
    input: Value,
    image: Vec<u8>,
    token: Vec<u8>,
    caller: CallerContext,
    cancel: Context,
    requests: Mutex<Vec<Value>>,
    closes: Mutex<Vec<Arc<AtomicUsize>>>,
    idle: AtomicUsize,
}
impl RawTransport for SDKRawPort {
    fn send(
        &self,
        request: RawRequest,
    ) -> TransportFuture<'_, Result<Option<RawResponse>, ExternalError>> {
        Box::pin(async move {
            assert!(Arc::ptr_eq(&request.context, &self.caller));
            assert!(request.logical_host.is_none());
            let multipart = if request.method == "POST" {
                assert_eq!(
                    request.content_length, -1,
                    "Go zero-length reader maps to raw unknown length"
                );
                observe_multipart(&request, &self.image, &self.token)
            } else {
                assert_eq!(request.content_length, 0);
                assert!(request.body.is_none());
                Value::Null
            };
            let endpoint = self.input["endpoint"].as_str().unwrap();
            let path = request.url.strip_prefix(endpoint).unwrap();
            self.requests.lock().unwrap().push(json!({"method":request.method,"url":request.url,"path":path,"body_nil":request.body.is_none(),"unknown_length":request.body.is_some()&&request.content_length==-1,"context_same":Arc::ptr_eq(&request.context,&self.caller),"context_value":request.context.value(&ContextKey::new("owned-large-multipart".to_owned())).and_then(|value|value.downcast::<String>().ok()).map(|value|value.as_ref().clone()),"context_canceled":request.context.error()==Some(ContextError::Canceled),"multipart":multipart}));
            if request.method == "POST" && self.name == "cancel-after-complete-upload" {
                self.cancel.cancel();
                return Err(Box::new(ContextError::Canceled) as ExternalError);
            }
            let mut status = 200;
            let mut headers = Headers::new();
            let text = match (request.method.as_str(), path) {
                ("POST", "/search/file") => {
                    status = 303;
                    headers.insert(
                        "Location".into(),
                        vec![format!(
                            "/search/color/{}",
                            self.input["hash"].as_str().unwrap()
                        )],
                    );
                    String::new()
                }
                ("GET", "") => self.input["home_template"]
                    .as_str()
                    .unwrap()
                    .replace("fixture-csrf", std::str::from_utf8(&self.token).unwrap()),
                ("GET", path)
                    if path
                        == format!("/search/color/{}", self.input["hash"].as_str().unwrap()) =>
                {
                    self.input["result_html"].as_str().unwrap().to_owned()
                }
                _ => panic!(
                    "unexpected ASCII2D request: {} {}",
                    request.method, request.url
                ),
            };
            let closes = Arc::new(AtomicUsize::new(0));
            self.closes.lock().unwrap().push(closes.clone());
            Ok(Some(RawResponse {
                status,
                headers,
                content_length: text.len() as i64,
                body: Some(Box::new(ResponseBody {
                    bytes: text.into_bytes(),
                    position: 0,
                    closes,
                })),
            }))
        })
    }
    fn close_idle_connections(&self) {
        self.idle.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn native_ascii2d_keeps_maximum_image_and_uncapped_csrf_token_through_public_upload() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../pixiv-cli/tests/fixtures/reverse-ascii2d-large-multipart.json"
    ))
    .unwrap();
    assert_eq!(
        fixture["source_commit"],
        "4b4426487ef18bed276706daec385e0d0a6979f9"
    );
    assert_eq!(fixture["go_version"], "go1.27.1");
    let input = &fixture["input"];
    let mut image = decode_hex(input["image_prefix_hex"].as_str().unwrap());
    image.resize(input["image_size"].as_u64().unwrap() as usize, 0);
    let token = input["token_byte"]
        .as_str()
        .unwrap()
        .repeat(input["token_size"].as_u64().unwrap() as usize)
        .into_bytes();
    assert_eq!(sha256(&image), input["image_sha256"]);
    assert_eq!(sha256(&token), input["token_sha256"]);
    for expected in fixture["cases"].as_array().unwrap() {
        let name = expected["name"].as_str().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("owned-image.png");
        std::fs::write(&source, &image).unwrap();
        let snapshot = Loader::new(SourceLoaderOptions {
            temp_dir: directory.path().into(),
            ..SourceLoaderOptions::default()
        })
        .load(Arc::new(Context::background()), source.to_str().unwrap())
        .await
        .unwrap();
        std::fs::write(&source, b"changed original after snapshot").unwrap();
        let cancel = Context::background().child();
        let caller: CallerContext = Arc::new(cancel.with_value(
            ContextKey::new("owned-large-multipart".to_owned()),
            Arc::new("owned-large-multipart".to_owned()),
        ));
        if name == "pre-canceled-upload" {
            cancel.cancel();
        }
        let port = Arc::new(SDKRawPort {
            name: name.to_owned(),
            input: input.clone(),
            image: image.clone(),
            token: token.clone(),
            caller: caller.clone(),
            cancel,
            requests: Mutex::new(Vec::new()),
            closes: Mutex::new(Vec::new()),
            idle: AtomicUsize::new(0),
        });
        let client = Client::new(Options {
            endpoint: input["endpoint"].as_str().unwrap().into(),
            http_transport: Some(Arc::new(NativeHttpTransport::new(port.clone()))),
            ..Options::default()
        })
        .unwrap();
        let upload = client.upload(caller.clone(), snapshot.clone()).await;
        let (session, error, search) = match upload {
            Ok(session) => {
                let result = session
                    .search(caller, Provider::Ascii2dColor)
                    .await
                    .unwrap();
                (
                    true,
                    Value::Null,
                    json!({"provider":result.provider,"matches":result.matches}),
                )
            }
            Err(error) => (
                false,
                json!({"message":error.to_string(),"canceled":error.context_error()==Some(ContextError::Canceled)}),
                Value::Null,
            ),
        };
        assert_eq!(
            session,
            expected["session"].as_bool().unwrap(),
            "{name}: {error}"
        );
        let mut retained = Vec::new();
        snapshot.open().unwrap().read_to_end(&mut retained).unwrap();
        assert_eq!(
            retained, image,
            "snapshot must survive source mutation and transport consumption"
        );
        client.close().await.unwrap();
        client.close().await.unwrap();
        snapshot.close().unwrap();
        snapshot.close().unwrap();
        let closed_error = snapshot.open().unwrap_err();
        let close_counts: Vec<_> = port
            .closes
            .lock()
            .unwrap()
            .iter()
            .map(|count| count.load(Ordering::SeqCst))
            .collect();
        let actual = json!({"name":name,"requests":*port.requests.lock().unwrap(),"session":session,"error":error,"search":search,"snapshot_size":snapshot.size(),"snapshot_sha256":snapshot.sha256(),"snapshot_bytes_retained":retained==image,"snapshot_closed_error":closed_error.to_string(),"response_close_counts":close_counts,"idle_closes":port.idle.load(Ordering::SeqCst)});
        assert_eq!(&actual, expected, "frozen Go public multipart case {name}");
    }
}
