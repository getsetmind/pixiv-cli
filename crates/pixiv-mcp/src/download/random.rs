use super::{
    DownloadDefaults, DownloadFuture, DownloadLease, DownloadOutput, SaveClientFactory,
    download_tool, empty_error, finish,
};
use crate::{CallToolResult, runtime::Account};
use pixiv_app::{
    download::{
        DownloadRequest, download_artworks, parse_pages, validate_quality, validate_ugoira_format,
    },
    execution::Execution,
    lifecycle::{Context, ContextError},
};
use pixiv_sdk::{
    Error, Reason,
    error::{Cause, TransportKind},
    pixiv::RecommendedArtworksRequest,
    transport::Transport,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct DownloadRandomInput {
    pub count: Option<i64>,
    pub pages: String,
    pub quality: String,
    pub ugoira_mode: String,
    pub delivery: String,
}

pub type DownloadRandomExecutor =
    dyn Fn(Context, DownloadRandomInput) -> DownloadFuture + Send + Sync;

pub fn download_random_tool() -> Value {
    let download = download_tool();
    json!({
        "name":"download_random_from_recommendation",
        "description":"Download random artworks from recommendations.",
        "inputSchema":{
            "type":"object",
            "additionalProperties":false,
            "properties":{
                "count":{
                    "type":["null","integer"],
                    "description":"optional artwork count; defaults to 5; explicit value must be from 1 to 20"
                },
                "pages":download["inputSchema"]["properties"]["pages"],
                "quality":download["inputSchema"]["properties"]["quality"],
                "ugoira_mode":download["inputSchema"]["properties"]["ugoira_mode"],
                "delivery":download["inputSchema"]["properties"]["delivery"]
            }
        },
        "outputSchema":download["outputSchema"]
    })
}

pub fn decode_download_random(arguments: Option<&Value>) -> Result<DownloadRandomInput, String> {
    let mut arguments = arguments
        .filter(|value| !value.is_null())
        .cloned()
        .unwrap_or_else(|| json!({}));
    if !arguments.is_object() {
        return Err(format!(
            "invalid params: validating \"arguments\": unmarshaling arguments: json: cannot unmarshal {} into Go value of type map[string]interface {{}}",
            match &arguments {
                Value::Bool(_) => "bool",
                Value::Number(_) => "number",
                _ => crate::stdio::value_type(&arguments),
            }
        ));
    }
    crate::search::validate_schema_with_integer_binding(
        &mut arguments,
        &download_random_tool()["inputSchema"],
        "",
        "invalid params: validating \"arguments\": validating root",
        ("downloadRandomIn", "int"),
    )?;
    serde_json::from_value(arguments).map_err(|error| format!("invalid params: {error}"))
}

struct Plan {
    count: usize,
    request: DownloadRequest,
}

fn plan(input: DownloadRandomInput, defaults: &DownloadDefaults) -> Result<Plan, String> {
    if !matches!(input.delivery.trim(), "" | "local_path") {
        return Err("Error: delivery supports only \"local_path\".".into());
    }
    let count = input.count.unwrap_or(5);
    if !(1..=20).contains(&count) {
        return Err("Error: count must be an integer from 1 to 20".into());
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
        count: count as usize,
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

fn shuffle<T>(items: &mut [T]) -> Result<(), getrandom::Error> {
    for index in (1..items.len()).rev() {
        let bound = (index + 1) as u64;
        let ceiling = u64::MAX - u64::MAX % bound;
        let selected = loop {
            let mut bytes = [0; 8];
            getrandom::fill(&mut bytes)?;
            let value = u64::from_ne_bytes(bytes);
            if value < ceiling {
                break (value % bound) as usize;
            }
        };
        items.swap(index, selected);
    }
    Ok(())
}

fn recommendation_context_error(error: ContextError) -> Error {
    Error::new(Reason::UpstreamUnavailable, "RecommendedArtworks")
        .with_transport(TransportKind::Http)
        .with_cause(Cause::TransportFailure(Box::new(match error {
            ContextError::Canceled => Cause::Canceled,
            ContextError::DeadlineExceeded => Cause::DeadlineExceeded,
        })))
}

pub async fn saved_download_random_with_account<T: Transport + 'static>(
    execution: &Execution<T>,
    context: &Context,
    defaults: &DownloadDefaults,
    input: DownloadRandomInput,
    account: &Account,
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
            return empty_error(format!("Could not retrieve recommendations: {error}"));
        }
    };
    let recommendation = tokio::select! {
        biased;
        result = lease.0.value().recommended_artworks(RecommendedArtworksRequest::default()) => result,
        error = context.cancelled() => Err(recommendation_context_error(error)),
    };
    let mut candidates = match recommendation {
        Ok(page) => page.items,
        Err(error) => {
            return empty_error(format!("Could not retrieve recommendations: {error}"));
        }
    };
    if candidates.is_empty() {
        return empty_error("Could not retrieve recommendations: the list is empty.".into());
    }
    if let Err(error) = shuffle(&mut candidates) {
        return empty_error(format!("Could not select random artworks: {error}"));
    }
    candidates.truncate(plan.count);
    let ids = candidates
        .iter()
        .map(|artwork| artwork.id)
        .collect::<Vec<_>>();
    let client = factory(lease.0.value().clone());
    let attempt = download_artworks(context, client.as_ref(), &ids, &plan.request).await;
    drop(client);
    drop(lease);
    finish(attempt)
}
