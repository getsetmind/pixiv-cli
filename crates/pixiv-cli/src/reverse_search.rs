use crate::{CommandError, DetailOutput};
use pixiv_app::reverse_search::{
    CallerContext, Evidence, Input as SourceInput, PixivRef, PixivRefType, Provider, ProviderError,
    ProviderSummary, Request, Response, SearchResult, Searcher, SourceKind,
};
use serde::Serialize;
use serde_json::{Value, value::RawValue};
use std::io::Write;

const INVALID_IDENTITY: &str = "reverse search returned an invalid Pixiv identity";
const PROVIDER_USAGE: &str = "provider must be one of saucenao, ascii2d-color, ascii2d-bovw, all";

#[derive(Clone, Debug, Default)]
pub struct Input {
    pub source: String,
    pub provider: String,
    pub changed_flags: Vec<String>,
    pub ndjson: bool,
    pub json_changed: bool,
}

pub fn is_image_source(source: &str) -> bool {
    let lower = source.to_ascii_lowercase();
    lower.starts_with("http:")
        || lower.starts_with("https:")
        || std::fs::metadata(source).is_ok_and(|info| info.is_file())
}

pub fn validate_input(input: &Input) -> Result<Provider, CommandError> {
    let changed = |name: &str| input.changed_flags.iter().any(|flag| flag == name);
    if !is_image_source(&input.source) {
        return if changed("provider") {
            Err(CommandError::Usage(
                "--provider is only supported for image sources".into(),
            ))
        } else {
            Ok(Provider::Unspecified)
        };
    }
    if input.json_changed && (changed("ndjson") || input.ndjson) {
        return Err(CommandError::Usage(
            "--ndjson cannot be used with --json".into(),
        ));
    }
    for name in [
        "search-by",
        "sort",
        "period",
        "start-date",
        "end-date",
        "rating",
        "type",
        "content-type",
        "resolution",
        "aspect-ratio",
        "draw-tool",
        "ai-mode",
        "bookmark-min",
        "bookmark-max",
        "bookmark-strategy",
        "limit",
        "page",
    ] {
        if changed(name) {
            return Err(CommandError::Usage(format!(
                "--{name} is not supported for image sources"
            )));
        }
    }
    explicit_provider(&input.provider)
}

pub fn resolve_provider(explicit: &str, default: &str) -> Result<Provider, CommandError> {
    if explicit.is_empty() {
        Ok(Provider::from(default))
    } else {
        explicit_provider(explicit)
    }
}

fn explicit_provider(value: &str) -> Result<Provider, CommandError> {
    match Provider::from(value) {
        provider @ (Provider::Unspecified
        | Provider::SauceNao
        | Provider::Ascii2dColor
        | Provider::Ascii2dBovw
        | Provider::All) => Ok(provider),
        Provider::Other(_) => Err(CommandError::Usage(PROVIDER_USAGE.into())),
    }
}

pub fn has_response_data(response: &Response) -> bool {
    response.input.kind != SourceKind::Unspecified
        || !response.input.sha256.is_empty()
        || response
            .providers
            .as_ref()
            .is_some_and(|values| !values.is_empty())
        || response
            .results
            .as_ref()
            .is_some_and(|values| !values.is_empty())
        || response
            .provider_errors
            .as_ref()
            .is_some_and(|values| !values.is_empty())
        || response.partial
}

pub async fn run<W: Write, E: Write>(
    searcher: &dyn Searcher,
    context: CallerContext,
    request: Request,
    mode: DetailOutput,
    output: &mut W,
    warnings: Option<&mut E>,
) -> Result<(), CommandError> {
    let outcome = searcher.search(context, request).await;
    if !has_response_data(&outcome.response)
        && let Some(error) = outcome.error.as_ref()
    {
        return Err(CommandError::ReverseSearch(error.clone()));
    }
    write_response(&outcome.response, mode, output)?;
    if outcome.response.partial {
        write_warning(&outcome.response, warnings)?;
    }
    match outcome.error {
        Some(error) => Err(CommandError::ReverseSearch(error)),
        None => Ok(()),
    }
}

pub fn write_response<W: Write>(
    response: &Response,
    mode: DetailOutput,
    output: &mut W,
) -> Result<(), CommandError> {
    let results = response.results.as_deref().unwrap_or_default();
    let records = records(results)?;
    match mode {
        DetailOutput::Ndjson => {
            for record in records {
                let encoded = serde_json::to_string(&record).map_err(serialization_error)?;
                write_once(output, &format!("{}\n", crate::go_json_escape(encoded)))?;
            }
        }
        DetailOutput::Json => {
            let results = results
                .iter()
                .map(ResultOutput::new)
                .collect::<Result<Vec<_>, _>>()?;
            let value = Output {
                input: &response.input,
                providers: response.providers.as_deref().unwrap_or_default(),
                results,
                records,
                provider_errors: response.provider_errors.as_deref().unwrap_or_default(),
                partial: response.partial,
            };
            let encoded = serde_json::to_string_pretty(&value).map_err(serialization_error)?;
            write_once(output, &format!("{}\n", crate::go_json_escape(encoded)))?;
        }
        DetailOutput::Human => {
            for result in results {
                let mut line = match result.pixiv.as_ref() {
                    Some(reference) => identity(reference)?["url"]
                        .as_str()
                        .expect("identity URL is a string")
                        .to_owned(),
                    None => "external result".to_owned(),
                };
                let title = crate::safe_line(&result.title);
                if !title.is_empty() {
                    line.push_str(" — ");
                    line.push_str(&title);
                }
                let author = crate::safe_line(&result.author);
                if !author.is_empty() {
                    line.push_str(" by ");
                    line.push_str(&author);
                }
                line.push('\n');
                write_once(output, &line)?;
            }
        }
    }
    Ok(())
}

pub fn write_warning<W: Write>(
    response: &Response,
    output: Option<&mut W>,
) -> Result<(), CommandError> {
    let output = output.ok_or(CommandError::Message(
        "reverse search partial warning output is not configured",
    ))?;
    let mut failed = response
        .provider_errors
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|error| error.provider.as_str())
        .collect::<Vec<_>>();
    if failed.is_empty() {
        failed.push("unknown provider");
    }
    write_once(
        output,
        &format!(
            "warning: reverse search completed partially; failed providers: {}\n",
            failed.join(", ")
        ),
    )
}

fn write_once<W: Write>(output: &mut W, value: &str) -> Result<(), CommandError> {
    // Go's writer contract permits a short count with no error.
    let _ = output.write(value.as_bytes())?;
    Ok(())
}

fn serialization_error(error: serde_json::Error) -> CommandError {
    CommandError::MessageText(error.to_string())
}

fn records(results: &[SearchResult]) -> Result<Vec<Value>, CommandError> {
    results
        .iter()
        .filter_map(|result| result.pixiv.as_ref())
        .map(identity)
        .collect()
}

fn identity(reference: &PixivRef) -> Result<Value, CommandError> {
    let (kind, path) = match &reference.kind {
        PixivRefType::Artwork => ("artwork", "artworks"),
        PixivRefType::User => ("user", "users"),
        PixivRefType::Other(_) => return Err(CommandError::Message(INVALID_IDENTITY)),
    };
    pixiv_record::from_identity(
        reference.id,
        kind,
        &format!("https://www.pixiv.net/{path}/{}", reference.id),
    )
    .map_err(|_| CommandError::Message(INVALID_IDENTITY))
}

#[derive(Serialize)]
struct Output<'a> {
    input: &'a SourceInput,
    providers: &'a [ProviderSummary],
    results: Vec<ResultOutput<'a>>,
    records: Vec<Value>,
    provider_errors: &'a [ProviderError],
    partial: bool,
}

#[derive(Serialize)]
struct ResultOutput<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pixiv: Option<&'a PixivRef>,
    #[serde(skip_serializing_if = "str::is_empty")]
    title: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    author: &'a str,
    evidence: Option<Vec<EvidenceOutput<'a>>>,
}
impl<'a> ResultOutput<'a> {
    fn new(result: &'a SearchResult) -> Result<Self, CommandError> {
        Ok(Self {
            pixiv: result.pixiv.as_ref(),
            title: &result.title,
            author: &result.author,
            evidence: result
                .evidence
                .as_ref()
                .map(|evidence| evidence.iter().map(EvidenceOutput::new).collect())
                .transpose()?,
        })
    }
}

#[derive(Serialize)]
struct EvidenceOutput<'a> {
    provider: &'a Provider,
    rank: i64,
    similarity: Box<RawValue>,
    index_id: i64,
    index_name: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    title: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    author: &'a str,
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    external_urls: &'a [String],
}
impl<'a> EvidenceOutput<'a> {
    fn new(evidence: &'a Evidence) -> Result<Self, CommandError> {
        Ok(Self {
            provider: &evidence.provider,
            rank: evidence.rank,
            similarity: go_float(evidence.similarity)?,
            index_id: evidence.index_id,
            index_name: &evidence.index_name,
            title: &evidence.title,
            author: &evidence.author,
            external_urls: &evidence.external_urls,
        })
    }
}

fn go_float(value: f64) -> Result<Box<RawValue>, CommandError> {
    if !value.is_finite() {
        let value = if value.is_nan() {
            "NaN"
        } else if value.is_sign_positive() {
            "+Inf"
        } else {
            "-Inf"
        };
        return Err(CommandError::MessageText(format!(
            "json: unsupported value: {value}"
        )));
    }
    let shortest = serde_json::to_string(&value).map_err(serialization_error)?;
    let (negative, unsigned) = shortest
        .strip_prefix('-')
        .map_or((false, shortest.as_str()), |value| (true, value));
    let (mantissa, exponent) = unsigned
        .split_once('e')
        .map_or((unsigned, 0), |(mantissa, exponent)| {
            (mantissa, exponent.parse::<i32>().expect("JSON exponent"))
        });
    let decimal = mantissa.find('.').unwrap_or(mantissa.len()) as i32;
    let mut digits = mantissa.replace('.', "");
    let mut decimal = decimal + exponent;
    while digits.starts_with('0') && digits.len() > 1 {
        digits.remove(0);
        decimal -= 1;
    }
    while digits.ends_with('0') && digits.len() > 1 {
        digits.pop();
    }
    let mut encoded = if digits == "0" {
        "0".to_owned()
    } else if value.abs() < 1e-6 || value.abs() >= 1e21 {
        let exponent = decimal - 1;
        let mut number = digits[..1].to_owned();
        if digits.len() > 1 {
            number.push('.');
            number.push_str(&digits[1..]);
        }
        number.push('e');
        if exponent >= 0 {
            number.push('+');
        }
        number.push_str(&exponent.to_string());
        number
    } else if decimal <= 0 {
        format!("0.{}{digits}", "0".repeat((-decimal) as usize))
    } else if decimal as usize >= digits.len() {
        format!("{digits}{}", "0".repeat(decimal as usize - digits.len()))
    } else {
        let position = decimal as usize;
        format!("{}.{}", &digits[..position], &digits[position..])
    };
    if negative {
        encoded.insert(0, '-');
    }
    RawValue::from_string(encoded).map_err(serialization_error)
}
