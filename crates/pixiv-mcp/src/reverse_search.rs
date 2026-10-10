use crate::{CallToolResult, TextContent};
use pixiv_app::{
    lifecycle::Context,
    reverse_search::{
        ErrorCode, Input, PixivRefType, Provider, ProviderError, ProviderSummary, Request,
        Response, SearchResult, Searcher,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ReverseSearchInput {
    pub source: String,
    #[serde(default)]
    pub provider: String,
}

#[derive(Debug, Default, Serialize)]
pub struct ReverseSearchOutput {
    pub input: Input,
    #[serde(serialize_with = "serialize_wire_numbers")]
    pub providers: Vec<ProviderSummary>,
    #[serde(serialize_with = "serialize_wire_numbers")]
    pub results: Vec<SearchResult>,
    pub records: Vec<Value>,
    pub provider_errors: Vec<ProviderError>,
    pub partial: bool,
}

fn serialize_wire_numbers<T: Serialize, S: serde::Serializer>(
    value: &T,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let mut value = serde_json::to_value(value).map_err(serde::ser::Error::custom)?;
    crate::structured_wire_numbers(&mut value);
    value.serialize(serializer)
}

pub struct ReverseExecutor {
    searcher: Arc<dyn Searcher>,
    default_provider: Provider,
    pixiv_only: bool,
}
impl ReverseExecutor {
    pub fn new(searcher: Arc<dyn Searcher>, default_provider: Provider, pixiv_only: bool) -> Self {
        Self {
            searcher,
            default_provider,
            pixiv_only,
        }
    }

    pub async fn invoke(
        &self,
        context: &Context,
        input: ReverseSearchInput,
    ) -> CallToolResult<ReverseSearchOutput> {
        if input.source.trim().is_empty() {
            return failure(ReverseSearchOutput::default(), ErrorCode::InvalidRequest);
        }
        let provider = if input.provider.is_empty() {
            match &self.default_provider {
                Provider::Unspecified => Provider::SauceNao,
                provider => provider.clone(),
            }
        } else {
            Provider::from(input.provider.as_str())
        };
        let outcome = self
            .searcher
            .search(
                Arc::new(context.clone()),
                Request {
                    source: input.source,
                    provider,
                    pixiv_only: self.pixiv_only,
                },
            )
            .await;
        let mut output = output_from_response(outcome.response);
        let records_failed = match records(&output.results) {
            Ok(records) => {
                output.records = records;
                false
            }
            Err(()) => true,
        };
        if let Some(error) = outcome.error {
            let code = if context.error().is_some() || error.context_error().is_some() {
                ErrorCode::Unknown
            } else {
                error.code()
            };
            return failure(output, code);
        }
        if records_failed {
            return failure(output, ErrorCode::Unknown);
        }
        CallToolResult {
            content: vec![TextContent {
                kind: "text",
                text: format!("Retrieved {} reverse-search results.", output.results.len()),
            }],
            structured_content: output,
            is_error: false,
        }
    }
}

fn output_from_response(response: Response) -> ReverseSearchOutput {
    let results = response
        .results
        .unwrap_or_default()
        .into_iter()
        .map(|mut result| {
            result.evidence = Some(result.evidence.unwrap_or_default());
            result
        })
        .collect();
    ReverseSearchOutput {
        input: response.input,
        providers: response.providers.unwrap_or_default(),
        results,
        records: Vec::new(),
        provider_errors: response.provider_errors.unwrap_or_default(),
        partial: response.partial,
    }
}

fn records(results: &[SearchResult]) -> Result<Vec<Value>, ()> {
    results
        .iter()
        .filter_map(|result| result.pixiv.as_ref())
        .map(|reference| {
            let (kind, segment) = match &reference.kind {
                PixivRefType::Artwork => ("artwork", "artworks"),
                PixivRefType::User => ("user", "users"),
                PixivRefType::Other(_) => return Err(()),
            };
            pixiv_record::from_identity(
                reference.id,
                kind,
                &format!("https://www.pixiv.net/{segment}/{}", reference.id),
            )
            .map_err(|_| ())
        })
        .collect()
}

pub(crate) fn missing_executor(input: &ReverseSearchInput) -> CallToolResult<ReverseSearchOutput> {
    failure(
        ReverseSearchOutput::default(),
        if input.source.trim().is_empty() {
            ErrorCode::InvalidRequest
        } else {
            ErrorCode::ProviderNotConfigured
        },
    )
}

fn failure(output: ReverseSearchOutput, code: ErrorCode) -> CallToolResult<ReverseSearchOutput> {
    let mut text = String::from("Error: reverse search failed");
    if code != ErrorCode::Unknown {
        text.push_str(&format!(" ({})", code.as_str()));
    }
    CallToolResult {
        content: vec![TextContent { kind: "text", text }],
        structured_content: output,
        is_error: true,
    }
}

pub fn reverse_search_tool() -> Value {
    let string = json!({"type":"string"});
    let integer = json!({"type":"integer"});
    let input = json!({"type":"object","required":["kind","sha256"],"properties":{"kind":string,"sha256":string},"additionalProperties":false});
    let quota = json!({"type":"object","required":["short_remaining","long_remaining","short_limit","long_limit"],"properties":{"short_remaining":integer,"long_remaining":integer,"short_limit":integer,"long_limit":integer},"additionalProperties":false});
    let provider = json!({"type":"object","required":["name","status","result_count"],"properties":{"name":string,"status":string,"result_count":integer,"quota":quota},"additionalProperties":false});
    let pixiv = json!({"type":"object","required":["type","id"],"properties":{"type":string,"id":integer},"additionalProperties":false});
    let evidence = json!({"type":"object","required":["provider","rank","similarity","index_id","index_name"],"properties":{"provider":string,"rank":integer,"similarity":{"type":"number"},"index_id":integer,"index_name":string,"title":string,"author":string,"external_urls":{"type":"array","items":string}},"additionalProperties":false});
    let result = json!({"type":"object","required":["evidence"],"properties":{"pixiv":pixiv,"title":string,"author":string,"evidence":{"type":"array","items":evidence}},"additionalProperties":false});
    let record = json!({"type":"object","required":["type","id","url"],"properties":{"type":string,"id":string,"url":string},"additionalProperties":{}});
    let error = json!({"type":"object","required":["provider","code","message"],"properties":{"provider":string,"code":string,"message":string},"additionalProperties":false});
    json!({
        "name":"reverse_search",
        "description":"Search an image source with SauceNAO or ascii2d and return Pixiv matches.",
        "inputSchema":{"type":"object","additionalProperties":false,"required":["source"],"properties":{
            "source":{"type":"string","description":"Readable regular file path or HTTP(S) image URL. The source is fetched/uploaded by the trusted local MCP server and is never returned."},
            "provider":{"type":"string","enum":["saucenao","ascii2d-color","ascii2d-bovw","all"],"description":"Optional provider override; omit to use the server startup configuration."}
        }},
        "outputSchema":{"type":"object","additionalProperties":false,"required":["input","providers","results","records","provider_errors","partial"],"properties":{
            "input":input,"providers":{"type":"array","items":provider},"results":{"type":"array","items":result},"records":{"type":"array","items":record},"provider_errors":{"type":"array","items":error},"partial":{"type":"boolean"}
        }}
    })
}

pub(crate) fn decode(arguments: Option<&Value>) -> Result<ReverseSearchInput, String> {
    let arguments = arguments.unwrap_or(&Value::Null);
    let object = if arguments.is_null() {
        None
    } else {
        Some(arguments.as_object().ok_or_else(|| format!(
            "invalid params: validating \"arguments\": unmarshaling arguments: json: cannot unmarshal {} into Go value of type map[string]interface {{}}",
            crate::stdio::value_type(arguments)
        ))?)
    };
    let Some(object) = object else {
        return Err(missing_source());
    };
    let extra: Vec<_> = object
        .keys()
        .filter(|key| !matches!(key.as_str(), "source" | "provider"))
        .map(|key| format!("{key:?}"))
        .collect();
    if !extra.is_empty() {
        return Err(format!(
            "invalid params: validating \"arguments\": validating root: unexpected additional properties [{}]",
            extra.join(" ")
        ));
    }
    let source = object
        .get("source")
        .ok_or_else(missing_source)?
        .as_str()
        .ok_or_else(|| crate::stdio::type_error("source", &object["source"], "string"))?
        .to_owned();
    let provider = if let Some(value) = object.get("provider") {
        let provider = value
            .as_str()
            .ok_or_else(|| crate::stdio::type_error("provider", value, "string"))?;
        if !matches!(
            provider,
            "saucenao" | "ascii2d-color" | "ascii2d-bovw" | "all"
        ) {
            return Err(format!(
                "invalid params: validating \"arguments\": validating root: validating /properties/provider: enum: {provider} does not equal any of: [saucenao ascii2d-color ascii2d-bovw all]"
            ));
        }
        provider.to_owned()
    } else {
        String::new()
    };
    Ok(ReverseSearchInput { source, provider })
}
fn missing_source() -> String {
    String::from(
        "invalid params: validating \"arguments\": validating root: required: missing properties: [\"source\"]",
    )
}
