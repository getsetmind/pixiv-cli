// This reuses the private production owner; it does not execute native HTTP dispatch.
pub use pixiv_sdk::fanbox::transport;
#[path = "../src/fanbox/decoded_body.rs"]
mod decoded_body;
#[path = "fanbox_media_support/decoded.rs"]
mod support;

use serde_json::Value;
use sha2::{Digest, Sha256};
use support::{Source, action, ownership_projection, raw_action_error};

fn rows() -> Vec<Value> {
    serde_json::from_str::<Value>(include_str!("fixtures/fanbox-media-body.json")).unwrap()["cases"]
        .as_array()
        .unwrap()
        .clone()
}

#[test]
fn sealed_helper_mapping_keeps_nil_metadata_and_go_panics_explicit() {
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(include_bytes!("fixtures/fanbox-media-body.json"))
        ),
        "a3b48317ef6f4dba2c5ca6d6f5d935467eee273147b88ebeff776b286f60bce2"
    );
    let rows = rows();
    let helpers: Vec<_> = rows
        .iter()
        .filter(|row| row["input"]["boundary"] == "fhttp_dependency_helper")
        .collect();
    assert_eq!(helpers.len(), 43);
    assert_eq!(
        helpers
            .iter()
            .filter(|row| row["input"]["bodies"][0]["nil_body"] == true)
            .count(),
        1
    );
    let panics: Vec<_> = helpers
        .iter()
        .flat_map(|row| row["result"]["steps"].as_array().unwrap())
        .filter_map(|step| step.get("go_only_panic"))
        .collect();
    assert_eq!(panics.len(), 2);
    assert!(panics.iter().all(|panic| *panic == "runtime.errorString: runtime error: invalid memory address or nil pointer dereference"));
    assert_eq!(
        helpers
            .iter()
            .filter(|row| row["input"]["name"] == "bridge_metadata_is_copied")
            .count(),
        1
    );
}

#[tokio::test]
async fn actual_private_decoder_replays_42_frozen_body_rows() {
    let mut checked = 0;
    for row in rows() {
        let input = &row["input"];
        if input["boundary"] != "fhttp_dependency_helper" || input["bodies"][0]["nil_body"] == true
        {
            continue;
        }
        checked += 1;
        let name = input["name"].as_str().unwrap();
        let source = Source::new(&input["bodies"][0]);
        let ownership = source.ownership.clone();
        let mut body = decoded_body::decode_body(
            Box::new(source),
            input["bodies"][0]["encoding"].as_str().unwrap(),
        )
        .await;
        assert_eq!(
            ownership_projection(&ownership.lock().unwrap()),
            ownership_projection(&row["result"]["ownership_at_return"][0]),
            "{name} ownership at return"
        );
        let operations = row["result"]["go_only_dependency_operations"]
            .as_array()
            .unwrap();
        let mut operation = 0;
        for (index, step) in row["result"]["steps"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
        {
            let kind = step["action"]["kind"].as_str().unwrap();
            if kind == "mutate_dependency_metadata" {
                // This body replay has no Go dependency-response metadata object to mutate.
                assert_eq!(name, "bridge_metadata_is_copied");
                continue;
            }
            let expected_raw_error = raw_action_error(operations, &mut operation, kind);
            let (bytes, error) = action(body.as_mut(), &step["action"]).await;
            if step.get("go_only_panic").is_some() {
                // Go dereferences its uninitialized decoder. Rust preserves an explicit error.
                assert_eq!(
                    error, "deflate decoder is not initialized",
                    "{name} Go-only panic mapping"
                );
            } else {
                assert_eq!(
                    bytes.len() as u64,
                    step["bytes"].as_u64().unwrap(),
                    "{name} step {index} bytes"
                );
                if matches!(kind, "read" | "read_all") {
                    assert_eq!(
                        format!("{:x}", Sha256::digest(&bytes)),
                        step["sha256"],
                        "{name} step {index} hash"
                    );
                }
                assert_eq!(
                    error, expected_raw_error,
                    "{name} step {index} raw decoder error"
                );
            }
            assert_eq!(
                ownership_projection(&ownership.lock().unwrap()),
                ownership_projection(&step["sources"][0]),
                "{name} step {index} ownership"
            );
        }
        let actual = ownership.lock().unwrap();
        let requested: Vec<_> = actual["reads"]
            .as_array()
            .unwrap()
            .iter()
            .map(|read| read["requested"].as_u64().unwrap())
            .collect();
        let expected_requested: Vec<_> = row["result"]["final_ownership"][0]["reads"]
            .as_array()
            .unwrap()
            .iter()
            .map(|read| read["requested"].as_u64().unwrap())
            .collect();
        if requested != expected_requested {
            eprintln!(
                "source-buffer-size exclusion {name}: Go {expected_requested:?}; Rust {requested:?}"
            );
        }
    }
    assert_eq!(checked, 42);
}

struct DecoderReplayTransport {
    spec: Value,
    source: std::sync::Mutex<Option<Box<dyn transport::RawBody>>>,
    requests: std::sync::Mutex<Vec<String>>,
}
impl transport::RawTransport for DecoderReplayTransport {
    fn send(
        &self,
        request: transport::RawRequest,
    ) -> transport::TransportFuture<
        '_,
        Result<Option<transport::RawResponse>, transport::ExternalError>,
    > {
        Box::pin(async move {
            self.requests.lock().unwrap().push(request.url.clone());
            if request
                .url
                .starts_with("https://api.fanbox.cc/creator.get?")
            {
                use base64::Engine;
                let metadata = serde_json::json!({"body":{"creatorId":"decoder-replay","user":{"name":"Decoder replay","iconUrl":"https://downloads.fanbox.cc/media/owned.bin"}}});
                let metadata_spec = serde_json::json!({"wire":base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&metadata).unwrap()),"chunk":0,"read_error":"","error_with_last_bytes":false,"close_error":false});
                return Ok(Some(transport::RawResponse {
                    status: 200,
                    headers: transport::Headers::new(),
                    content_length: -1,
                    body: Some(Box::new(Source::new(&metadata_spec))),
                }));
            }
            assert_eq!(request.url, "https://downloads.fanbox.cc/media/owned.bin");
            let body = if self.spec["nil_body"] == true {
                None
            } else {
                let source = self.source.lock().unwrap().take().unwrap();
                Some(
                    decoded_body::decode_body(source, self.spec["encoding"].as_str().unwrap())
                        .await,
                )
            };
            let mut headers = transport::Headers::from([(
                "Content-Length".into(),
                vec![self.spec["content_length"].to_string()],
            )]);
            let encoding = self.spec["encoding"].as_str().unwrap();
            if !encoding.is_empty() {
                headers.insert("Content-Encoding".into(), vec![encoding.into()]);
            }
            Ok(Some(transport::RawResponse {
                status: self.spec["status"].as_u64().unwrap() as u16,
                headers,
                content_length: if body.is_some() { -1 } else { 0 },
                body,
            }))
        })
    }
}

#[tokio::test]
async fn private_decoder_and_compiled_public_resource_safety_replay_all_43_helper_rows() {
    use pixiv_sdk::{
        context::Context,
        fanbox::{Client, Options, SessionCredentials},
        resource::{OpenResourceRequest, ResourceRef},
    };
    use std::sync::{Arc, Mutex};
    let mut checked = 0;
    for row in rows() {
        let input = &row["input"];
        if input["boundary"] != "fhttp_dependency_helper" {
            continue;
        }
        checked += 1;
        let name = input["name"].as_str().unwrap();
        let source = Source::new(&input["bodies"][0]);
        let ownership = source.ownership.clone();
        let transport = Arc::new(DecoderReplayTransport {
            spec: input["bodies"][0].clone(),
            source: Mutex::new(Some(Box::new(source))),
            requests: Mutex::new(Vec::new()),
        });
        let client = Client::open_with(
            SessionCredentials {
                fanbox_sessid: "synthetic-owned-media".into(),
            },
            Options {
                http_client: Some(transport.clone()),
                ..Default::default()
            },
        )
        .unwrap();
        let reference =
            ResourceRef::new("fanbox", br#"{"k":"creator_icon","c":"decoder-replay"}"#).unwrap();
        let mut response = client
            .open_resource(
                Arc::new(Context::background()),
                OpenResourceRequest {
                    reference,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(
            transport.requests.lock().unwrap().as_slice(),
            [
                "https://api.fanbox.cc/creator.get?creatorId=decoder-replay",
                "https://downloads.fanbox.cc/media/owned.bin"
            ],
            "{name} public ref metadata/media connectivity"
        );
        assert_eq!(
            ownership_projection(&ownership.lock().unwrap()),
            ownership_projection(&row["result"]["ownership_at_return"][0]),
            "{name} public safety ownership at return"
        );
        for (index, step) in row["result"]["steps"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
        {
            let kind = step["action"]["kind"].as_str().unwrap();
            if kind == "mutate_dependency_metadata" {
                // This body replay has no Go dependency-response metadata object to mutate.
                assert_eq!(name, "bridge_metadata_is_copied");
                continue;
            }
            let (bytes, error) = action(&mut response.body, &step["action"]).await;
            if step.get("go_only_panic").is_some() {
                assert_eq!(
                    error, "close FANBOX media failed",
                    "{name} fallible Rust mapping of Go-only panic"
                );
            } else {
                assert_eq!(
                    bytes.len() as u64,
                    step["bytes"].as_u64().unwrap(),
                    "{name} public safety step {index} bytes"
                );
                if matches!(kind, "read" | "read_all") {
                    assert_eq!(
                        format!("{:x}", Sha256::digest(&bytes)),
                        step["sha256"],
                        "{name} public safety step {index} hash"
                    );
                }
                assert_eq!(
                    error, step["error"]["message"],
                    "{name} public safety step {index} safe error"
                );
            }
            assert!(
                !error.contains("canary"),
                "{name} external cause escaped media safety"
            );
            assert_eq!(
                ownership_projection(&ownership.lock().unwrap()),
                ownership_projection(&step["sources"][0]),
                "{name} public safety step {index} ownership"
            );
        }
    }
    assert_eq!(checked, 43);
}

#[tokio::test]
async fn frozen_codec_faults_keep_bytes_and_error_in_the_same_raw_read() {
    for name in [
        "gzip_checksum_error",
        "deflate_truncated",
        "br_excessive_input",
        "zstd_truncated",
    ] {
        let row = rows()
            .into_iter()
            .find(|row| row["input"]["name"] == name)
            .unwrap();
        let input = &row["input"];
        let source = Source::new(&input["bodies"][0]);
        let mut body = decoded_body::decode_body(
            Box::new(source),
            input["bodies"][0]["encoding"].as_str().unwrap(),
        )
        .await;
        let expected = &row["result"]["go_only_dependency_operations"][0];
        let mut output = [0; 512];
        let read = body.read(&mut output).await;
        assert_eq!(
            read.count as u64,
            expected["returned"].as_u64().unwrap(),
            "{name} simultaneous bytes/error count"
        );
        assert_eq!(
            read.error.unwrap().to_string(),
            expected["go_only_dependency_error"]["message"],
            "{name} simultaneous bytes/error value"
        );
        assert!(!read.eof, "{name} codec failure cannot become exact EOF");
    }
}

struct PausingEnd {
    source: Source,
    wire_length: u64,
    release: std::sync::Arc<std::sync::atomic::AtomicBool>,
    blocked: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl transport::RawBody for PausingEnd {
    fn read<'a>(
        &'a mut self,
        output: &'a mut [u8],
    ) -> transport::BodyFuture<'a, transport::RawRead> {
        Box::pin(async move {
            use std::sync::atomic::Ordering;
            let consumed = self.source.ownership.lock().unwrap()["bytes_read"]
                .as_u64()
                .unwrap();
            if consumed == self.wire_length && !self.release.load(Ordering::SeqCst) {
                self.blocked.store(true, Ordering::SeqCst);
                std::future::pending::<()>().await;
            }
            self.source.read(output).await
        })
    }
    fn close(&mut self) -> transport::BodyFuture<'_, Result<(), transport::ExternalError>> {
        self.source.close()
    }
}

#[tokio::test]
async fn cancelled_read_futures_keep_decoded_output_under_body_ownership() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    for name in ["gzip_complete", "zstd_complete"] {
        let row = rows()
            .into_iter()
            .find(|row| row["input"]["name"] == name)
            .unwrap();
        let spec = &row["input"]["bodies"][0];
        let release = Arc::new(AtomicBool::new(false));
        let blocked = Arc::new(AtomicBool::new(false));
        let source = PausingEnd {
            source: Source::new(spec),
            wire_length: spec["content_length"].as_u64().unwrap(),
            release: release.clone(),
            blocked: blocked.clone(),
        };
        let mut body =
            decoded_body::decode_body(Box::new(source), spec["encoding"].as_str().unwrap()).await;
        let mut abandoned = [0; 512];
        let mut read = body.read(&mut abandoned);
        assert!(
            futures_util::poll!(read.as_mut()).is_pending(),
            "{name} read must wait at the controlled source boundary"
        );
        assert!(
            blocked.load(Ordering::SeqCst),
            "{name} source boundary was not reached"
        );
        drop(read);
        release.store(true, Ordering::SeqCst);
        let mut output = [0; 512];
        let read = body.read(&mut output).await;
        assert_eq!(
            &output[..read.count],
            b"owned media payload",
            "{name} cancellation discarded consumed decoded bytes"
        );
        assert!(read.eof, "{name} resumed stream must retain exact EOF");
        assert!(read.error.is_none());
    }
}

#[tokio::test]
async fn cancelled_zstd_checksum_read_resumes_validation_before_exposing_block() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let row = rows()
        .into_iter()
        .find(|row| row["input"]["name"] == "zstd_truncated")
        .unwrap();
    let spec = &row["input"]["bodies"][0];
    let release = Arc::new(AtomicBool::new(false));
    let blocked = Arc::new(AtomicBool::new(false));
    let source = PausingEnd {
        source: Source::new(spec),
        wire_length: 28,
        release: release.clone(),
        blocked: blocked.clone(),
    };
    let mut body = decoded_body::decode_body(Box::new(source), "zstd").await;
    let mut abandoned = [0; 512];
    let mut read = body.read(&mut abandoned);
    assert!(futures_util::poll!(read.as_mut()).is_pending());
    assert!(blocked.load(Ordering::SeqCst));
    drop(read);
    release.store(true, Ordering::SeqCst);
    let mut output = [0; 512];
    let read = body.read(&mut output).await;
    assert_eq!(
        read.count, 0,
        "unverified synchronous zstd block escaped before checksum failure"
    );
    assert_eq!(read.error.unwrap().to_string(), "unexpected EOF");
    let read = body.read(&mut output[..1]).await;
    assert_eq!(read.count, 1);
    assert_eq!(output[0], b'o');
    assert!(read.error.is_none());
}
