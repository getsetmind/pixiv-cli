use crate::{CallToolResult, TextContent};
use pixiv_app::{
    download::{
        DownloadAttempt, DownloadReport, DownloadRequest, DownloadSaveClient, download_sources,
        parse_pages, validate_quality, validate_ugoira_format,
    },
    execution::Execution,
    lifecycle::{Context, Lease},
};
use pixiv_sdk::{Client, models::UgoiraFrame, transport::Transport};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{future::Future, io::Read, path::Path, pin::Pin, sync::Arc};

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct DownloadInput {
    pub src: String,
    pub srcs: Vec<String>,
    pub pages: String,
    pub quality: String,
    pub ugoira_mode: String,
    pub delivery: String,
}
#[derive(Clone, Debug, Default, Serialize)]
pub struct DownloadOutput {
    pub delivery: String,
    pub items: Vec<DownloadItem>,
    pub failures: Vec<DownloadFailure>,
    pub warnings: Vec<DownloadWarning>,
    pub files: Vec<DownloadFile>,
    pub text: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct DownloadItem {
    pub url: String,
    pub illust_id: i64,
    pub title: String,
    pub author: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub files: Vec<DownloadFile>,
}
#[derive(Clone, Debug, Serialize)]
pub struct DownloadFailure {
    pub url: String,
    pub illust_id: i64,
    #[serde(rename = "type")]
    pub kind: String,
    pub message: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct DownloadWarning {
    pub illust_id: i64,
    #[serde(rename = "type")]
    pub kind: String,
    pub message: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct DownloadFile {
    pub illust_id: i64,
    pub title: String,
    pub author: String,
    pub path: String,
    pub file_uri: String,
    pub mime_type: String,
    pub size_bytes: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub page: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub quality: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub frames: Vec<UgoiraFrame>,
}
fn is_zero(value: &i64) -> bool {
    *value == 0
}
pub type DownloadFuture = Pin<Box<dyn Future<Output = CallToolResult<DownloadOutput>> + Send>>;
pub type DownloadExecutor = dyn Fn(Context, DownloadInput) -> DownloadFuture + Send + Sync;
pub type SaveClientFactory<T> = dyn Fn(Arc<Client<T>>) -> Arc<dyn DownloadSaveClient> + Send + Sync;

pub fn download_tool() -> Value {
    let frame = json!({"type":"object","additionalProperties":false,"required":["filename","delay_milliseconds"],"properties":{"filename":{"type":"string"},"delay_milliseconds":{"type":"integer"}}});
    let file = json!({"type":"object","additionalProperties":false,"required":["illust_id","title","author","path","file_uri","mime_type","size_bytes"],"properties":{"illust_id":{"type":"integer"},"title":{"type":"string"},"author":{"type":"string"},"path":{"type":"string"},"file_uri":{"type":"string"},"mime_type":{"type":"string"},"size_bytes":{"type":"integer"},"page":{"type":"integer"},"quality":{"type":"string"},"frames":{"type":"array","items":frame}}});
    let item = json!({"type":"object","additionalProperties":false,"required":["url","illust_id","title","author","type","files"],"properties":{"url":{"type":"string"},"illust_id":{"type":"integer"},"title":{"type":"string"},"author":{"type":"string"},"type":{"type":"string"},"files":{"type":"array","items":file.clone()}}});
    let failure = json!({"type":"object","additionalProperties":false,"required":["url","illust_id","type","message"],"properties":{"url":{"type":"string"},"illust_id":{"type":"integer"},"type":{"type":"string"},"message":{"type":"string"}}});
    let warning = json!({"type":"object","additionalProperties":false,"required":["illust_id","type","message"],"properties":{"illust_id":{"type":"integer"},"type":{"type":"string"},"message":{"type":"string"}}});
    json!({"name":"download","description":"Download artwork IDs or supported Pixiv artwork/user URLs with intelligent storage rules.","inputSchema":{"type":"object","additionalProperties":false,"properties":{
        "src":{"type":"string","description":"PID, Pixiv artwork/user URL, or allowed CDN resource URL"},
        "srcs":{"type":"array","description":"multiple PID, Pixiv artwork/user URL, or allowed CDN resource URLs","items":{"type":"string"}},
        "pages":{"type":"string","description":"1-based page selection, e.g. 1,3-5; default all pages"},
        "quality":{"type":"string","description":"static image quality: original, regular, small, thumb, mini"},
        "ugoira_mode":{"type":"string","description":"ugoira output mode: gif, apng, zip, or raw; default gif"},
        "delivery":{"type":"string","description":"delivery mode: local_path only"}
    }},"outputSchema":{"type":"object","additionalProperties":false,"required":["delivery","items","failures","warnings","files","text"],"properties":{"delivery":{"type":"string"},"items":{"type":"array","items":item},"failures":{"type":"array","items":failure},"warnings":{"type":"array","items":warning},"files":{"type":"array","items":file},"text":{"type":"string"}}}})
}

pub fn decode_download(arguments: Option<&Value>) -> Result<DownloadInput, String> {
    let mut arguments = arguments
        .filter(|value| !value.is_null())
        .cloned()
        .unwrap_or_else(|| json!({}));
    if !arguments.is_object() {
        return Err(format!(
            "invalid params: validating \"arguments\": unmarshaling arguments: json: cannot unmarshal {} into Go value of type map[string]interface {{}}",
            crate::stdio::value_type(&arguments)
        ));
    }
    crate::search::validate_schema_with_integer_binding(
        &mut arguments,
        &download_tool()["inputSchema"],
        "",
        "invalid params: validating \"arguments\": validating root",
        ("downloadIn", "int"),
    )?;
    serde_json::from_value(arguments).map_err(|error| format!("invalid params: {error}"))
}
#[derive(Clone, Debug, Default)]
pub struct DownloadDefaults {
    pub download_path: String,
    pub filename_template: String,
    pub directory_template: String,
}

impl From<&pixiv_app::config::RuntimeConfig> for DownloadDefaults {
    fn from(runtime: &pixiv_app::config::RuntimeConfig) -> Self {
        Self {
            download_path: runtime.download_path.clone(),
            filename_template: runtime.filename_template.clone(),
            directory_template: runtime.directory_template.clone(),
        }
    }
}

struct Plan {
    sources: Vec<String>,
    request: DownloadRequest,
}
fn plan(input: DownloadInput, defaults: &DownloadDefaults) -> Result<Plan, String> {
    let sources = if !input.src.trim().is_empty() {
        if !input.srcs.is_empty() {
            return Err("Error: provide src or srcs, not both".into());
        }
        vec![input.src]
    } else {
        if input.srcs.is_empty() {
            return Err("Error: provide src (one source) or srcs (a source list)".into());
        }
        input.srcs
    };
    if !matches!(input.delivery.trim(), "" | "local_path") {
        return Err("Error: delivery supports only \"local_path\".".into());
    }
    let pages = parse_pages(&input.pages).map_err(|error| format!("Error: {error}"))?;
    let quality = if input.quality.trim().is_empty() {
        "original"
    } else {
        input.quality.trim()
    }
    .to_owned();
    validate_quality(&quality).map_err(|error| format!("Error: {error}"))?;
    let ugoira_format = if input.ugoira_mode.trim().is_empty() {
        "gif"
    } else {
        input.ugoira_mode.trim()
    }
    .to_owned();
    validate_ugoira_format(&ugoira_format).map_err(|error| format!("Error: {error}"))?;
    Ok(Plan {
        sources,
        request: DownloadRequest {
            download_path: defaults.download_path.clone(),
            filename_template: defaults.filename_template.clone(),
            directory_template: defaults.directory_template.clone(),
            pages,
            quality,
            ugoira_format,
        },
    })
}
fn result(output: DownloadOutput, is_error: bool) -> CallToolResult<DownloadOutput> {
    CallToolResult {
        content: vec![TextContent {
            kind: "text",
            text: output.text.clone(),
        }],
        structured_content: output,
        is_error,
    }
}
fn empty_error(text: String) -> CallToolResult<DownloadOutput> {
    result(
        DownloadOutput {
            delivery: "local_path".into(),
            text,
            ..Default::default()
        },
        true,
    )
}
pub async fn download(
    context: &Context,
    client: &dyn DownloadSaveClient,
    path: &str,
    input: DownloadInput,
) -> CallToolResult<DownloadOutput> {
    download_with_defaults(
        context,
        client,
        &DownloadDefaults {
            download_path: path.into(),
            ..Default::default()
        },
        input,
    )
    .await
}
pub async fn download_with_defaults(
    context: &Context,
    client: &dyn DownloadSaveClient,
    defaults: &DownloadDefaults,
    input: DownloadInput,
) -> CallToolResult<DownloadOutput> {
    let plan = match plan(input, defaults) {
        Ok(plan) => plan,
        Err(error) => return empty_error(error),
    };
    finish(download_sources(context, client, &plan.sources, &plan.request).await)
}
struct DownloadLease<C>(Lease<Arc<C>, pixiv_app::scheduler::SchedulerError>);
impl<C> Drop for DownloadLease<C> {
    fn drop(&mut self) {
        // The Go handler defers Close without replacing its download result.
        let _ = self.0.close();
    }
}
pub async fn saved_download<T: Transport + 'static>(
    execution: &Execution<T>,
    context: &Context,
    path: &str,
    input: DownloadInput,
    proxy: Option<&str>,
    factory: Arc<SaveClientFactory<T>>,
) -> CallToolResult<DownloadOutput> {
    saved_download_with_defaults(
        execution,
        context,
        &DownloadDefaults {
            download_path: path.into(),
            ..Default::default()
        },
        input,
        proxy,
        factory,
    )
    .await
}
pub async fn saved_download_with_defaults<T: Transport + 'static>(
    execution: &Execution<T>,
    context: &Context,
    defaults: &DownloadDefaults,
    input: DownloadInput,
    proxy: Option<&str>,
    factory: Arc<SaveClientFactory<T>>,
) -> CallToolResult<DownloadOutput> {
    saved_download_with_account(
        execution,
        context,
        defaults,
        input,
        &crate::runtime::Account {
            user_id: 0,
            https_proxy_override: proxy.map(str::to_owned),
        },
        factory,
    )
    .await
}
pub async fn saved_download_with_account<T: Transport + 'static>(
    execution: &Execution<T>,
    context: &Context,
    defaults: &DownloadDefaults,
    input: DownloadInput,
    account: &crate::runtime::Account,
    factory: Arc<SaveClientFactory<T>>,
) -> CallToolResult<DownloadOutput> {
    let plan = match plan(input, defaults) {
        Ok(plan) => plan,
        Err(error) => return empty_error(error),
    };
    let lease = match execution
        .open_client(
            context,
            account.user_id,
            account.https_proxy_override.as_deref(),
        )
        .await
    {
        Ok(lease) => DownloadLease(lease),
        Err(error) => {
            return finish(DownloadAttempt {
                error: Some(error),
                ..Default::default()
            });
        }
    };
    let client = factory(lease.0.value().clone());
    let attempt = download_sources(context, client.as_ref(), &plan.sources, &plan.request).await;
    drop(client);
    drop(lease);
    finish(attempt)
}
fn finish(attempt: DownloadAttempt) -> CallToolResult<DownloadOutput> {
    let mut output = match build_report(&attempt.report) {
        Ok(output) => output,
        Err(error) => return empty_error(format!("Could not build the download result: {error}")),
    };
    let failed = attempt.error.is_some() || !output.failures.is_empty();
    if let Some(error) = attempt.error {
        output.text.push_str(&format!("\nDownload failed: {error}"))
    }
    result(output, failed)
}
fn build_report(report: &DownloadReport) -> std::io::Result<DownloadOutput> {
    let mut output = DownloadOutput {
        delivery: "local_path".into(),
        text: "Download completed; delivery: local_path.".into(),
        ..Default::default()
    };
    for item in &report.items {
        let url = if item.illust_id > 0 {
            format!("https://www.pixiv.net/artworks/{}", item.illust_id)
        } else {
            String::new()
        };
        if item.illust_id > 0 {
            output.text.push_str(&format!(
                "\nArtwork {} - {:?} / Author: {} / Type: {}",
                item.illust_id, item.title, item.author, item.kind
            ))
        } else {
            output
                .text
                .push_str(&format!("\nResource download / Type: {}", item.kind))
        }
        let mut files = Vec::new();
        for file in &item.files {
            let published = DownloadFile {
                illust_id: item.illust_id,
                title: item.title.clone(),
                author: item.author.clone(),
                path: file.path.to_string_lossy().into_owned(),
                file_uri: file_uri(&file.path),
                mime_type: mime_type(&file.path),
                size_bytes: std::fs::metadata(&file.path)?.len() as i64,
                page: file.page,
                quality: if item.kind == "ugoira" {
                    item.quality.clone()
                } else {
                    String::new()
                },
                frames: if item.kind == "ugoira" {
                    item.frames.clone()
                } else {
                    vec![]
                },
            };
            output.text.push_str(&format!(
                "\n- {}\n  URI: {}\n  MIME: {}\n  Size: {} bytes",
                published.path, published.file_uri, published.mime_type, published.size_bytes
            ));
            files.push(published.clone());
            output.files.push(published);
        }
        output.items.push(DownloadItem {
            url,
            illust_id: item.illust_id,
            title: item.title.clone(),
            author: item.author.clone(),
            kind: item.kind.clone(),
            files,
        });
    }
    for warning in &report.warnings {
        output.warnings.push(DownloadWarning {
            illust_id: warning.illust_id,
            kind: warning.kind.clone(),
            message: warning.message.clone(),
        });
        let prefix = if warning.illust_id > 0 {
            format!("Warning artwork {}", warning.illust_id)
        } else if !warning.kind.is_empty() {
            format!("Warning {}", warning.kind)
        } else {
            "Warning".into()
        };
        output
            .text
            .push_str(&format!("\n{prefix}: {}", warning.message));
    }
    for failure in &report.failures {
        output.failures.push(DownloadFailure {
            url: failure.url.clone(),
            illust_id: failure.illust_id,
            kind: failure.kind.clone(),
            message: failure.message.clone(),
        });
        output
            .text
            .push_str(&format!("\nFailed {}: {}", failure.url, failure.message));
    }
    Ok(output)
}
fn mime_type(path: &Path) -> String {
    let mut buffer = [0u8; 512];
    let count = std::fs::File::open(path)
        .and_then(|mut file| file.read(&mut buffer))
        .unwrap_or_default();
    let bytes = &buffer[..count];
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return "image/png".into();
    }
    if bytes.starts_with(b"\xff\xd8\xff") {
        return "image/jpeg".into();
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return "image/gif".into();
    }
    if bytes.len() >= 14 && bytes.starts_with(b"RIFF") && &bytes[8..14] == b"WEBPVP" {
        return "image/webp".into();
    }
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
    .into()
}
fn file_uri(path: &Path) -> String {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|root| root.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let mut encoded = String::new();
    let path = absolute.to_string_lossy();
    let path = if cfg!(windows) {
        path.replace('\\', "/")
    } else {
        path.into_owned()
    };
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~/:@&=+$;,".contains(&byte) {
            encoded.push(byte as char)
        } else {
            encoded.push_str(&format!("%{byte:02X}"))
        }
    }
    if encoded.starts_with('/') {
        format!("file://{encoded}")
    } else {
        format!("file:{encoded}")
    }
}
