#[allow(dead_code)]
mod fanbox_resource_support;

#[cfg(unix)]
mod unix {
    use super::fanbox_resource_support::{
        self as support, FixtureTransport, error, ownership, project,
    };
    use base64::{Engine, engine::general_purpose::STANDARD};
    use futures_util::FutureExt;
    use pixiv_sdk::{
        context::{Context, ContextError, ContextFuture, ContextKey, ContextValue, RequestContext},
        fanbox::{
            Client, CreatorRequest, Options, PostRequest, SessionCredentials,
            transport::{
                BodyFuture, ExternalError, RawBody, RawRead, RawRequest, RawResponse, RawTransport,
                TransportFuture,
            },
        },
        resource::{Resource, ResourceRef, SaveOptions, SaveProgress, SavedResource},
    };
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use std::{
        error::Error as StdError,
        fs,
        os::unix::fs::{DirBuilderExt, PermissionsExt, symlink},
        panic::AssertUnwindSafe,
        path::{Component, Path, PathBuf},
        sync::{Arc, Mutex, atomic::Ordering},
        time::Instant,
    };
    use tokio::sync::Notify;

    #[derive(Debug)]
    struct SaveContext {
        base: Context,
        completed: Mutex<Option<ContextError>>,
        wake: Notify,
    }
    impl SaveContext {
        fn stop(&self, kind: &str) {
            let mut completed = self.completed.lock().unwrap();
            if completed.is_none() {
                *completed = Some(if kind == "deadline" {
                    ContextError::DeadlineExceeded
                } else {
                    ContextError::Canceled
                });
                self.wake.notify_waiters();
            }
        }
    }
    impl RequestContext for SaveContext {
        fn error(&self) -> Option<ContextError> {
            (*self.completed.lock().unwrap()).or_else(|| self.base.error())
        }
        fn deadline(&self) -> Option<Instant> {
            None
        }
        fn cancelled(&self) -> ContextFuture<'_> {
            Box::pin(async move {
                loop {
                    let notified = self.wake.notified();
                    if let Some(error) = self.error() {
                        return error;
                    }
                    tokio::select! { _ = notified => {}, error = self.base.cancelled() => return error }
                }
            })
        }
        fn value(&self, key: &ContextKey) -> Option<ContextValue> {
            RequestContext::value(&self.base, key)
        }
    }
    struct SaveBody {
        source: Box<dyn RawBody>,
        context: Arc<SaveContext>,
        zero_reads: u64,
        stop_on_read: String,
    }
    impl RawBody for SaveBody {
        fn read<'a>(&'a mut self, output: &'a mut [u8]) -> BodyFuture<'a, RawRead> {
            Box::pin(async move {
                let result = if self.zero_reads > 0 {
                    self.zero_reads -= 1;
                    RawRead {
                        count: 0,
                        eof: false,
                        error: None,
                    }
                } else {
                    self.source.read(output).await
                };
                if !self.stop_on_read.is_empty() {
                    self.context.stop(&self.stop_on_read);
                    self.stop_on_read.clear();
                }
                result
            })
        }
        fn close(&mut self) -> BodyFuture<'_, std::result::Result<(), ExternalError>> {
            self.source.close()
        }
    }
    struct SaveTransport {
        source: Arc<FixtureTransport>,
        input: Value,
        context: Arc<SaveContext>,
    }
    impl RawTransport for SaveTransport {
        fn send(
            &self,
            request: RawRequest,
        ) -> TransportFuture<'_, std::result::Result<Option<RawResponse>, ExternalError>> {
            Box::pin(async move {
                let target = url::Url::parse(&request.url).unwrap();
                let metadata = target.host_str() == Some("api.fanbox.cc")
                    && matches!(target.path(), "/creator.get" | "/post.info");
                let mut response = self.source.send(request).await?;
                if !metadata && let Some(response) = &mut response {
                    response.body = response.body.take().map(|source| {
                        Box::new(SaveBody {
                            source,
                            context: self.context.clone(),
                            zero_reads: self.input["zero_reads"].as_u64().unwrap(),
                            stop_on_read: self.input["stop_on_read"].as_str().unwrap().into(),
                        }) as Box<dyn RawBody>
                    });
                    let stop = self.input["stop_on_open"].as_str().unwrap();
                    if !stop.is_empty() {
                        self.context.stop(stop);
                    }
                }
                Ok(response)
            })
        }
        fn close_idle_connections(&self) {
            self.source.close_idle_connections();
        }
    }
    fn client(source: Arc<FixtureTransport>, input: &Value, context: Arc<SaveContext>) -> Client {
        Client::open_with(
            SessionCredentials {
                fanbox_sessid: support::SESSION.into(),
            },
            Options {
                http_client: Some(Arc::new(SaveTransport {
                    source,
                    input: input.clone(),
                    context,
                })),
                user_agent: "resource-injected-agent".into(),
                ..Default::default()
            },
        )
        .unwrap()
    }
    fn snapshot(resource: &Resource) -> Value {
        let payload = String::from_utf8(resource.reference.payload().unwrap()).unwrap();
        assert!(
            !payload.contains("https://")
                && !payload.contains("signature")
                && !payload.contains(support::SESSION)
        );
        json!({"ref":resource.reference.as_str(),"payload":payload,"url":resource.url,"request_headers":resource.request_headers,
            "expires_at":resource.expires_at,"requires_credentials":resource.requires_credentials,
            "dto":pixiv_sdk::dto::ResourceDto::from_resource(resource)})
    }
    async fn generate(
        client: &Client,
        context: Arc<dyn RequestContext>,
        kind: &str,
    ) -> pixiv_sdk::Result<(Resource, Vec<Value>)> {
        let mut all = vec![];
        if kind.starts_with("creator_") {
            let creator = client
                .creator(
                    context,
                    CreatorRequest {
                        creator_id: "resource-creator".into(),
                    },
                )
                .await?;
            for resource in [&creator.icon.resource, &creator.cover.resource] {
                if !resource.reference.is_zero() {
                    all.push(snapshot(resource));
                }
            }
            Ok((
                if kind == "creator_icon" {
                    creator.icon.resource
                } else {
                    creator.cover.resource
                },
                all,
            ))
        } else {
            let post = client
                .post(
                    context,
                    PostRequest {
                        post_id: "resource-post".into(),
                    },
                )
                .await?;
            let mut selected = Resource::default();
            if let Some(body) = post.body {
                for asset in body.assets.unwrap_or_default() {
                    all.push(snapshot(&asset.resource));
                    if format!("post_{}", asset.kind.as_str()) == kind {
                        selected = asset.resource;
                    }
                }
            }
            assert!(
                !selected.reference.is_zero(),
                "synthetic producer did not emit selected resource"
            );
            Ok((selected, all))
        }
    }
    struct OwnedRoot(PathBuf);
    impl OwnedRoot {
        fn new() -> Self {
            let mut random = [0u8; 16];
            getrandom::fill(&mut random).unwrap();
            let path =
                std::env::temp_dir().join(format!("fanbox-save-{:x}", u128::from_ne_bytes(random)));
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            builder.create(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for OwnedRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn relative(root: &Path, path: &Path) -> String {
        path.strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/")
    }
    fn files(root: &Path) -> Value {
        fn visit(root: &Path, dir: &Path, result: &mut Vec<Value>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                let info = fs::symlink_metadata(&path).unwrap();
                let kind = if info.is_symlink() {
                    "symlink"
                } else if info.is_dir() {
                    "directory"
                } else {
                    "file"
                };
                let mut name = relative(root, &path);
                if path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(".atomic-write-")
                {
                    name = relative(root, &path.parent().unwrap().join(".atomic-write-$TEMP"));
                }
                let mut item = json!({"path":name,"kind":kind,"mode":format!("{:04o}",info.permissions().mode() & 0o777)});
                if kind == "file" {
                    let bytes = fs::read(&path).unwrap();
                    item["bytes"] = json!(STANDARD.encode(&bytes));
                    item["size"] = json!(bytes.len());
                    item["sha256"] = json!(format!("{:x}", Sha256::digest(&bytes)));
                }
                if kind == "symlink" {
                    item["target"] = json!(relative(root, &fs::read_link(&path).unwrap()));
                }
                result.push(item);
                if kind == "directory" {
                    visit(root, &path, result);
                }
            }
        }
        let mut result = vec![];
        visit(root, root, &mut result);
        result.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
        json!(result)
    }
    fn mkdir(path: &Path, mode: u32) {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(mode);
        builder.create(path).unwrap();
    }
    fn write(path: &Path, bytes: &[u8], mode: u32) {
        fs::write(path, bytes).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }
    fn destination(root: &Path, input: &Value) -> String {
        let raw = input["path"].as_str().unwrap();
        let path = if raw.trim().is_empty() {
            PathBuf::from(raw)
        } else {
            let mut path = root.to_path_buf();
            for component in Path::new(raw).components() {
                match component {
                    Component::ParentDir => {
                        path.pop();
                    }
                    Component::CurDir => {}
                    other => path.push(other.as_os_str()),
                }
            }
            path
        };
        match input["setup"].as_str().unwrap() {
            "" => {}
            "parent_file" => write(&root.join("nested"), b"parent-old", 0o600),
            "final_directory" => {
                mkdir(&path, 0o755);
                write(&path.join("child"), b"final-old", 0o644);
            }
            "old_final" => {
                mkdir(path.parent().unwrap(), 0o755);
                write(&path, b"final-old", 0o644);
            }
            "existing_parent" => mkdir(path.parent().unwrap(), 0o755),
            "final_symlink" => {
                mkdir(path.parent().unwrap(), 0o755);
                let target = root.join("link-target");
                write(&target, b"target-old", 0o644);
                symlink(target, &path).unwrap();
            }
            "parent_symlink" => {
                let target = root.join("owned-target");
                mkdir(&target, 0o755);
                symlink(target, root.join("nested")).unwrap();
            }
            other => panic!("unknown destination setup {other}"),
        }
        path.to_string_lossy().into_owned()
    }
    fn progress_action(path: &str, action: &str, context: &SaveContext) {
        match action {
            "" => {}
            "cancel" | "deadline" => context.stop(action),
            "remove_temp" | "temp_directory" => {
                let matches: Vec<_> = fs::read_dir(Path::new(path).parent().unwrap())
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .filter(|path| {
                        path.file_name()
                            .unwrap()
                            .to_string_lossy()
                            .starts_with(".atomic-write-")
                    })
                    .collect();
                assert_eq!(
                    matches.len(),
                    1,
                    "progress must observe one real atomic destination"
                );
                fs::remove_file(&matches[0]).unwrap();
                if action == "temp_directory" {
                    mkdir(&matches[0], 0o700);
                }
            }
            "final_directory" => {
                mkdir(Path::new(path), 0o700);
                write(&Path::new(path).join("child"), b"callback-owned", 0o600);
            }
            other => panic!("unknown progress action {other}"),
        }
    }
    async fn observe(input: &Value) -> Value {
        let root = OwnedRoot::new();
        let cancel = Context::new().with_value(
            ContextKey::new("resource-context-key"),
            Arc::new("resource-context".to_owned()),
        );
        let controlled = Arc::new(SaveContext {
            base: cancel.clone(),
            completed: Mutex::new(None),
            wake: Notify::new(),
        });
        let context: Arc<dyn RequestContext> = controlled.clone();
        let resource = &input["resource"];
        let producer_transport = Arc::new(FixtureTransport::new(
            "producer",
            resource["producer_document"].as_str().unwrap(),
            resource,
            &cancel,
            context.clone(),
        ));
        let consumer_transport = Arc::new(FixtureTransport::new(
            "consumer",
            resource["reopen_document"].as_str().unwrap(),
            resource,
            &cancel,
            context.clone(),
        ));
        let producer = client(producer_transport.clone(), input, controlled.clone());
        let consumer = (resource["mode"] == "fresh")
            .then(|| client(consumer_transport.clone(), input, controlled.clone()));
        let client = consumer.as_ref().unwrap_or(&producer);
        let mut out = json!({"generated_resources":[],"generation_error":error(None,false),"reference_error":error(None,false),"outcomes":[]});
        let mut reference = ResourceRef::default();
        let kind = resource["kind"].as_str().unwrap();
        let ready = if !kind.is_empty() {
            match generate(&producer, context.clone(), kind).await {
                Ok((value, all)) => {
                    reference = value.reference;
                    out["generated_resources"] = json!(all);
                    true
                }
                Err(failure) => {
                    out["generation_error"] = error(Some(&failure), false);
                    false
                }
            }
        } else if !resource["ref_payload"].as_str().unwrap().is_empty() {
            match ResourceRef::new(
                resource["ref_product"].as_str().unwrap(),
                resource["ref_payload"].as_str().unwrap().as_bytes(),
            ) {
                Ok(value) => {
                    reference = value;
                    true
                }
                Err(failure) => {
                    out["reference_error"] = error(Some(&failure), false);
                    false
                }
            }
        } else if resource["parse_only"] == true
            || !resource["parse_text"].as_str().unwrap().is_empty()
        {
            match ResourceRef::parse(resource["parse_text"].as_str().unwrap()) {
                Ok(value) => {
                    reference = value;
                    resource["parse_only"] != true
                }
                Err(failure) => {
                    out["reference_error"] = error(Some(&failure), false);
                    false
                }
            }
        } else {
            true
        };
        let path = destination(&root.0, input);
        out["initial_files"] = files(&root.0);
        if ready {
            out["selected_ref"] = json!(reference.as_str());
            let initial_context = resource["context"].as_str().unwrap();
            if !initial_context.is_empty() {
                controlled.stop(initial_context);
            }
            let mut outcomes = vec![];
            for _ in 0..resource["repeat"].as_u64().unwrap().max(1) {
                let progress = Arc::new(Mutex::new(vec![]));
                let action_applied = Arc::new(Mutex::new(false));
                let callback = (input["progress"] == true).then(|| {
                    let progress = progress.clone();
                    let action_applied = action_applied.clone();
                    let root = root.0.clone();
                    let path = path.clone();
                    let input = input.clone();
                    let controlled = controlled.clone();
                    let producer_transport = producer_transport.clone();
                    let consumer_transport = consumer_transport.clone();
                    Arc::new(move |value: SaveProgress| {
                        progress.lock().unwrap().push(json!({
                            "done": value.done,
                            "total": value.total,
                            "files_before_write": files(&root),
                            "ownership_before_write": ownership(&producer_transport, &consumer_transport)
                        }));
                        if input["panic_on_progress"] == true {
                            panic!("owned-progress-panic");
                        }
                        let mut applied = action_applied.lock().unwrap();
                        if !*applied {
                            progress_action(&path, input["progress_action"].as_str().unwrap(), &controlled);
                            *applied = true;
                        }
                    }) as Arc<dyn Fn(SaveProgress) + Send + Sync>
                });
                let invocation = AssertUnwindSafe(client.save_resource(
                    context.clone(),
                    reference.clone(),
                    SaveOptions {
                        path: path.clone(),
                        progress: callback,
                    },
                ))
                .catch_unwind()
                .await;
                let (returned, panic_text, saved, failure) = match invocation {
                    Ok(result) => match result {
                        Ok(saved) => (true, "", saved, None),
                        Err(failure) => (
                            true,
                            "",
                            SavedResource {
                                path: String::new(),
                                size: 0,
                                content_type: String::new(),
                            },
                            Some(failure),
                        ),
                    },
                    Err(payload) => {
                        let expected = input["panic_on_progress"] == true
                            && (payload
                                .downcast_ref::<&str>()
                                .is_some_and(|text| *text == "owned-progress-panic")
                                || payload
                                    .downcast_ref::<String>()
                                    .is_some_and(|text| text == "owned-progress-panic"));
                        assert!(expected, "unexpected SaveResource panic");
                        (
                            false,
                            "owned-progress-panic",
                            SavedResource {
                                path: String::new(),
                                size: 0,
                                content_type: String::new(),
                            },
                            None,
                        )
                    }
                };
                let saved_path = if !saved.path.is_empty() {
                    format!("$OWNED/{}", relative(&root.0, Path::new(&saved.path)))
                } else {
                    String::new()
                };
                let context_error = controlled.error();
                outcomes.push(json!({"returned":returned,"panic":panic_text,"saved":{"path":saved_path,"size":saved.size,"content_type":saved.content_type},
                    "error":error(failure.as_ref().map(|failure|failure as &dyn StdError),false),"progress":*progress.lock().unwrap(),"files":files(&root.0),
                    "ownership_at_return":ownership(&producer_transport,&consumer_transport),"context_at_return":error(context_error.as_ref().map(|error|error as &dyn StdError),false)}));
            }
            out["outcomes"] = json!(outcomes);
        }
        producer.close_idle_connections();
        producer.close_idle_connections();
        if let Some(consumer) = &consumer {
            consumer.close_idle_connections();
            consumer.close_idle_connections();
        }
        out["requests"] = json!(
            producer_transport
                .requests
                .lock()
                .unwrap()
                .iter()
                .cloned()
                .chain(consumer_transport.requests.lock().unwrap().iter().cloned())
                .collect::<Vec<_>>()
        );
        out["final_ownership"] = ownership(&producer_transport, &consumer_transport);
        out["producer_close_idle_calls"] =
            json!(producer_transport.idle_calls.load(Ordering::SeqCst));
        out["consumer_close_idle_calls"] =
            json!(consumer_transport.idle_calls.load(Ordering::SeqCst));
        for file in files(&root.0).as_array().unwrap() {
            assert!(
                !file["path"].as_str().unwrap().contains(".atomic-write-"),
                "public save left an owned temporary destination behind"
            );
        }
        out
    }
    fn differences(actual: &Value, expected: &Value, path: &str, out: &mut Vec<String>) {
        match (actual, expected) {
            (Value::Object(actual), Value::Object(expected)) => {
                let keys: std::collections::BTreeSet<_> =
                    actual.keys().chain(expected.keys()).collect();
                for key in keys {
                    differences(
                        actual.get(key).unwrap_or(&Value::Null),
                        expected.get(key).unwrap_or(&Value::Null),
                        &format!("{path}/{key}"),
                        out,
                    );
                }
            }
            (Value::Array(actual), Value::Array(expected)) => {
                if actual.len() != expected.len() {
                    out.push(format!(
                        "{path}/length actual={} expected={}",
                        actual.len(),
                        expected.len()
                    ));
                }
                for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
                    differences(actual, expected, &format!("{path}/{index}"), out);
                }
            }
            _ if actual != expected => {
                out.push(format!("{path}: actual={actual} expected={expected}"))
            }
            _ => {}
        }
    }
    #[tokio::test]
    async fn all_frozen_public_save_observations_match() {
        let raw = include_bytes!("fixtures/fanbox-save-resource.json");
        assert_eq!(
            format!("{:x}", Sha256::digest(raw)),
            "e9a289e77908c55563b2a2a76b01cc10cd1395bf3e5a2a5f682ceb2e90e3cd41"
        );
        let fixture: Value = serde_json::from_slice(raw).unwrap();
        let cases = fixture["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 277);
        let mut failures = vec![];
        for row in cases {
            let mut actual = observe(&row["input"]).await;
            let mut expected = row["observation"].clone();
            project(&mut actual);
            project(&mut expected);
            if actual != expected {
                let mut details = vec![];
                differences(&actual, &expected, "", &mut details);
                failures.push(format!("{}\n{}", row["name"], details.join("\n")));
            }
        }
        assert!(
            failures.is_empty(),
            "{} public SaveResource mismatches\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}
