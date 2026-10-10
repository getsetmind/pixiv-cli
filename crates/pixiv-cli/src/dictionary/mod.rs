pub mod service;

use crate::CommandError;
use pixiv_app::lifecycle::Context;
use serde::Serialize;
use std::io::Write;

#[derive(Clone, Debug)]
pub struct ArticleOptions {
    pub reference: String,
    pub json: Option<bool>,
    pub language: String,
    pub skip_counters: bool,
}

#[derive(Clone, Debug)]
pub struct SearchOptions {
    pub query: String,
    pub json: Option<bool>,
    pub ndjson: bool,
    pub page: i64,
    pub limit: i64,
}

#[derive(Clone, Debug)]
pub struct DictionaryCommand {
    operation: Operation,
}

#[derive(Clone, Debug)]
enum Operation {
    Group,
    Help(Topic),
    Article(ArticleOptions),
    Search(SearchOptions),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Topic {
    Group,
    Article,
    Search,
}

struct Parsed {
    topic: Topic,
    values: Vec<String>,
    help: bool,
    json: bool,
    json_changed: bool,
    ndjson: bool,
    language: String,
    skip_counters: bool,
    page: i64,
    limit: i64,
}

impl DictionaryCommand {
    pub fn parse(args: &[String]) -> Result<Self, CommandError> {
        let (parsed, result) = parse_flags(args);
        result?;
        if parsed.help {
            return Ok(Self {
                operation: Operation::Help(parsed.topic),
            });
        }
        let expected = usize::from(parsed.topic != Topic::Group);
        if parsed.values.len() != expected {
            return Err(CommandError::Message(match parsed.topic {
                Topic::Group => "usage: pixiv dic <search|article> [options]",
                Topic::Article => {
                    "usage: pixiv dic article <title|url> [--lang ja|en] [--no-counters] [--json]"
                }
                Topic::Search => {
                    "usage: pixiv dic search <query> [--page N] [--limit N] [--json] [--ndjson]"
                }
            }));
        }
        let json = parsed.json_changed.then_some(parsed.json);
        let operation = match parsed.topic {
            Topic::Group => Operation::Group,
            Topic::Article => Operation::Article(ArticleOptions {
                reference: parsed.values.into_iter().next().unwrap(),
                json,
                language: parsed.language,
                skip_counters: parsed.skip_counters,
            }),
            Topic::Search => Operation::Search(SearchOptions {
                query: parsed.values.into_iter().next().unwrap(),
                json,
                ndjson: parsed.ndjson,
                page: parsed.page,
                limit: parsed.limit,
            }),
        };
        Ok(Self { operation })
    }

    pub fn parse_root(args: &[String]) -> Result<Self, CommandError> {
        Self::parse(args).map_err(|error| match error {
            CommandError::MessageText(message) => {
                if let Some(name) = message.strip_prefix("unknown flag: ") {
                    CommandError::Usage(format!("unknown option '{name}'"))
                } else if let Some(short) = message
                    .strip_prefix("unknown shorthand flag: ")
                    .and_then(|value| value.split('\'').nth(1))
                {
                    CommandError::Usage(format!("unknown option '-{short}'"))
                } else {
                    CommandError::MessageText(message)
                }
            }
            other => other,
        })
    }

    pub fn output_policy_requested(args: &[String]) -> (bool, bool) {
        let (parsed, _) = parse_flags(args);
        (parsed.ndjson, parsed.json_changed || parsed.ndjson)
    }

    pub fn requires_runtime(&self) -> bool {
        !matches!(self.operation, Operation::Help(_))
    }

    pub fn ndjson_output(&self) -> bool {
        matches!(&self.operation, Operation::Search(options) if options.ndjson)
    }

    pub fn machine_output(&self) -> bool {
        match &self.operation {
            Operation::Article(options) => options.json.is_some(),
            Operation::Search(options) => options.json.is_some() || options.ndjson,
            Operation::Group | Operation::Help(_) => false,
        }
    }

    pub fn render_help(&self, command_path: &str) -> Option<String> {
        let topic = match self.operation {
            Operation::Group => Topic::Group,
            Operation::Help(topic) => topic,
            Operation::Article(_) | Operation::Search(_) => return None,
        };
        Some(help_text(topic, command_path))
    }

    pub fn write_help<W: Write>(&self, command_path: &str, mut output: W) {
        let Some(text) = self.render_help(command_path) else {
            return;
        };
        let (description, usage) = text.split_once("\n\n").unwrap();
        let _ = output.write(format!("{description}\n").as_bytes());
        let _ = output.write(b"\n");
        let _ = output.write(usage.as_bytes());
    }

    pub fn validate_options(&self) -> Result<(), CommandError> {
        match &self.operation {
            Operation::Article(options) if !matches!(options.language.as_str(), "ja" | "en") => {
                Err(CommandError::Usage("--lang must be one of: ja, en".into()))
            }
            Operation::Search(options) => {
                if options.page < 1 {
                    return Err(CommandError::Usage("--page must be greater than 0".into()));
                }
                if options.limit < 0 {
                    return Err(CommandError::Usage("--limit must not be negative".into()));
                }
                if options.ndjson && options.json.is_some() {
                    return Err(CommandError::Usage(
                        "--ndjson cannot be used with --json".into(),
                    ));
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    pub async fn execute<T, W, F>(
        &self,
        client: &service::Client<T>,
        context: &Context,
        mut output: W,
        json_out: F,
    ) -> Result<(), CommandError>
    where
        T: service::Transport,
        W: Write,
        F: FnOnce(Option<bool>) -> Result<bool, CommandError>,
    {
        self.validate_options()?;
        match &self.operation {
            Operation::Group | Operation::Help(_) => {
                self.write_help("dic", &mut output);
                Ok(())
            }
            Operation::Article(options) => {
                let article = client
                    .article(
                        Some(context),
                        service::ArticleRequest {
                            reference: options.reference.clone(),
                            language: options.language.clone(),
                            skip_counters: options.skip_counters,
                        },
                    )
                    .await
                    .map_err(|error| CommandError::State(Box::new(error)))?;
                let record = ArticleOutput::new(&article, options.skip_counters);
                if json_out(options.json)? {
                    write_json(&mut output, &record, true)
                } else {
                    print_article(&mut output, &record)
                }
            }
            Operation::Search(options) => {
                let mut results = client
                    .search(
                        Some(context),
                        service::SearchRequest {
                            query: options.query.clone(),
                            page: options.page,
                        },
                    )
                    .await
                    .map_err(|error| CommandError::State(Box::new(error)))?;
                if options.limit > 0 && (results.len() as u64) > options.limit as u64 {
                    results.truncate(options.limit as usize);
                }
                let records = results.iter().map(SearchOutput::from).collect::<Vec<_>>();
                if options.ndjson {
                    for record in &records {
                        write_json(&mut output, record, false)?;
                    }
                    return Ok(());
                }
                if json_out(options.json)? {
                    write_json(&mut output, &records, true)
                } else {
                    print_search(&mut output, &records)
                }
            }
        }
    }
}

fn discover(args: &[String]) -> (Topic, Option<usize>) {
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            break;
        }
        if !arg.contains('=') && (arg.starts_with("--") || (arg.starts_with('-') && arg.len() == 2))
        {
            if args.len() - index - 1 <= 1 {
                break;
            }
            index += 2;
            continue;
        }
        if !arg.is_empty() && !arg.starts_with('-') {
            return match arg.as_str() {
                "article" => (Topic::Article, Some(index)),
                "search" => (Topic::Search, Some(index)),
                _ => (Topic::Group, None),
            };
        }
        index += 1;
    }
    (Topic::Group, None)
}

fn parse_flags(args: &[String]) -> (Parsed, Result<(), CommandError>) {
    let (topic, operation_index) = discover(args);
    let args = args
        .iter()
        .enumerate()
        .filter(|(index, _)| Some(*index) != operation_index)
        .map(|(_, arg)| arg)
        .collect::<Vec<_>>();
    let mut parsed = Parsed {
        topic,
        values: vec![],
        help: false,
        json: false,
        json_changed: false,
        ndjson: false,
        language: "ja".into(),
        skip_counters: false,
        page: 1,
        limit: 0,
    };
    let result = (|| {
        let mut index = 0;
        let mut flags = true;
        while index < args.len() {
            let arg = args[index];
            index += 1;
            if flags && arg == "--" {
                flags = false;
                continue;
            }
            if flags && arg.starts_with("--") {
                let (name, raw) = arg
                    .split_once('=')
                    .map_or((arg.as_str(), None), |(name, raw)| (name, Some(raw)));
                if name.len() == 2 || name.starts_with("---") {
                    return Err(CommandError::MessageText(format!("bad flag syntax: {arg}")));
                }
                let boolean = match name {
                    "--help" => Some("-h, --help"),
                    "--json" if topic != Topic::Group => Some("-j, --json"),
                    "--no-counters" if topic == Topic::Article => Some("--no-counters"),
                    "--ndjson" if topic == Topic::Search => Some("--ndjson"),
                    _ => None,
                };
                if let Some(label) = boolean {
                    set_boolean(&mut parsed, name, raw.unwrap_or("true"), label)?;
                    continue;
                }
                let scalar = matches!(
                    (topic, name),
                    (Topic::Article, "--lang") | (Topic::Search, "--page" | "--limit")
                );
                if !scalar {
                    return Err(CommandError::MessageText(format!("unknown flag: {name}")));
                }
                let value = if let Some(raw) = raw {
                    raw
                } else {
                    let value = args.get(index).ok_or_else(|| {
                        CommandError::MessageText(format!("flag needs an argument: {name}"))
                    })?;
                    index += 1;
                    value.as_str()
                };
                set_scalar(&mut parsed, name, value)?;
                continue;
            }
            if flags && arg.starts_with('-') && arg != "-" {
                let mut shorts = &arg[1..];
                while !shorts.is_empty() {
                    let flag = shorts.chars().next().unwrap();
                    let tail = &shorts[flag.len_utf8()..];
                    let (name, label) = match flag {
                        'h' => ("--help", "-h, --help"),
                        'j' if topic != Topic::Group => ("--json", "-j, --json"),
                        'n' if topic == Topic::Search => ("--limit", "-n, --limit"),
                        _ => {
                            return Err(CommandError::MessageText(format!(
                                "unknown shorthand flag: {flag:?} in -{shorts}"
                            )));
                        }
                    };
                    if shorts.len() > 2 && tail.starts_with('=') {
                        if flag == 'n' {
                            set_scalar(&mut parsed, name, &tail[1..])?;
                        } else {
                            set_boolean(&mut parsed, name, &tail[1..], label)?;
                        }
                        break;
                    }
                    if flag == 'n' {
                        let value = if !tail.is_empty() {
                            tail
                        } else {
                            let value = args.get(index).ok_or_else(|| {
                                CommandError::MessageText(format!(
                                    "flag needs an argument: 'n' in -{shorts}"
                                ))
                            })?;
                            index += 1;
                            value.as_str()
                        };
                        set_scalar(&mut parsed, name, value)?;
                        break;
                    }
                    set_boolean(&mut parsed, name, "true", label)?;
                    shorts = tail;
                }
                continue;
            }
            parsed.values.push(arg.clone());
        }
        Ok(())
    })();
    (parsed, result)
}

fn set_boolean(
    parsed: &mut Parsed,
    name: &str,
    raw: &str,
    label: &str,
) -> Result<(), CommandError> {
    let value = crate::auth_accounts::boolean(raw, label);
    let flag = value.as_ref().copied().unwrap_or(false);
    match name {
        "--help" => parsed.help = flag,
        "--json" => parsed.json = flag,
        "--ndjson" => parsed.ndjson = flag,
        "--no-counters" => parsed.skip_counters = flag,
        _ => unreachable!(),
    }
    value?;
    if name == "--json" {
        parsed.json_changed = true;
    }
    Ok(())
}

fn set_scalar(parsed: &mut Parsed, name: &str, raw: &str) -> Result<(), CommandError> {
    if name == "--lang" {
        parsed.language = raw.into();
        return Ok(());
    }
    let value = crate::timeline::timeline_integer(raw).map_err(|cause| {
        let label = if name == "--limit" {
            "-n, --limit"
        } else {
            "--page"
        };
        CommandError::MessageText(format!(
            "invalid argument {} for {} flag: strconv.ParseInt: parsing {}: {cause}",
            crate::search::quote(raw),
            crate::search::quote(label),
            crate::search::quote(raw),
        ))
    })?;
    if name == "--page" {
        parsed.page = value;
    } else {
        parsed.limit = value;
    }
    Ok(())
}

fn help_text(topic: Topic, command_path: &str) -> String {
    match topic {
        Topic::Group => {
            let extra = if command_path.split_whitespace().count() == 1 {
                "  completion  Generate the autocompletion script for the specified shell\n  help        Help about any command\n"
            } else {
                ""
            };
            format!(
                "Read the Pixiv encyclopedia (dic.pixiv.net)\n\nUsage:\n  {command_path} [flags]\n  {command_path} [command]\n\nAvailable Commands:\n  article     Fetch one Pixiv encyclopedia article\n{extra}  search      Search Pixiv encyclopedia articles\n\nFlags:\n  -h, --help   help for dic\n\nUse \"{command_path} [command] --help\" for more information about a command.\n"
            )
        }
        Topic::Article => format!(
            "Fetch one Pixiv encyclopedia article\n\nUsage:\n  {command_path} article <title|url> [flags]\n\nFlags:\n  -h, --help          help for article\n  -j, --json          print JSON\n      --lang string   article language: ja or en (default \"ja\")\n      --no-counters   skip the view and work counters\n"
        ),
        Topic::Search => format!(
            "Search Pixiv encyclopedia articles\n\nUsage:\n  {command_path} search <query> [flags]\n\nFlags:\n  -h, --help        help for search\n  -j, --json        print JSON\n  -n, --limit int   maximum results; 0 returns every article on the page\n      --ndjson      print one encyclopedia article as JSON per line\n      --page int    1-based result page (default 1)\n"
        ),
    }
}

#[derive(Serialize)]
struct ArticleOutput<'a> {
    id: i64,
    title: &'a str,
    yomigana: &'a str,
    translation: &'a str,
    categories: &'a Option<Vec<String>>,
    #[serde(rename = "abstract")]
    abstract_text: &'a str,
    related: &'a [String],
    body: &'a str,
    url: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    views: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    works: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    comments: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    checklists: Option<i64>,
}

impl<'a> ArticleOutput<'a> {
    fn new(article: &'a service::Article, skip_counters: bool) -> Self {
        Self {
            id: article.id,
            title: &article.title,
            yomigana: &article.yomigana,
            translation: &article.translation,
            categories: &article.categories,
            abstract_text: &article.abstract_,
            related: &article.related,
            body: &article.body,
            url: &article.url,
            views: (!skip_counters).then_some(article.views),
            works: (!skip_counters).then_some(article.works),
            comments: (!skip_counters).then_some(article.comments),
            checklists: (!skip_counters).then_some(article.checklists),
        }
    }
}

#[derive(Serialize)]
struct SearchOutput<'a> {
    title: &'a str,
    summary: &'a str,
    updated: &'a str,
    views: i64,
    works: i64,
    checklists: i64,
    related: &'a Option<Vec<String>>,
    url: &'a str,
    thumbnail: &'a str,
}

impl<'a> From<&'a service::SearchResult> for SearchOutput<'a> {
    fn from(result: &'a service::SearchResult) -> Self {
        Self {
            title: &result.title,
            summary: &result.summary,
            updated: &result.updated,
            views: result.views,
            works: result.works,
            checklists: result.checklists,
            related: &result.related,
            url: &result.url,
            thumbnail: &result.thumbnail,
        }
    }
}

fn write_once<W: Write>(output: &mut W, text: String) -> Result<(), CommandError> {
    let _ = output.write(text.as_bytes())?;
    Ok(())
}

fn write_json<W: Write, T: Serialize>(
    output: &mut W,
    value: &T,
    pretty: bool,
) -> Result<(), CommandError> {
    let body = if pretty {
        serde_json::to_string_pretty(value)
    } else {
        serde_json::to_string(value)
    }
    .map_err(|error| CommandError::State(Box::new(error)))?;
    write_once(output, format!("{}\n", crate::go_json_escape(body)))
}

fn print_article<W: Write>(
    output: &mut W,
    article: &ArticleOutput<'_>,
) -> Result<(), CommandError> {
    let heading = if article.yomigana.is_empty() {
        article.title.to_owned()
    } else {
        format!("{} ({})", article.title, article.yomigana)
    };
    write_once(output, format!("{heading}\n  {}\n", article.url))?;
    if !article.translation.is_empty() {
        write_once(output, format!("  translation: {}\n", article.translation))?;
    }
    if let Some(categories) = article
        .categories
        .as_ref()
        .filter(|items| !items.is_empty())
    {
        write_once(output, format!("  categories: {}\n", categories.join(", ")))?;
    }
    if let Some(views) = article.views {
        write_once(
            output,
            format!(
                "  views:{views} works:{} comments:{} checklists:{}\n",
                article.works.unwrap(),
                article.comments.unwrap(),
                article.checklists.unwrap()
            ),
        )?;
    }
    if !article.related.is_empty() {
        write_once(
            output,
            format!("  related: {}\n", article.related.join(", ")),
        )?;
    }
    for section in [article.abstract_text, article.body] {
        if !section.is_empty() {
            write_once(output, format!("\n{section}\n"))?;
        }
    }
    Ok(())
}

fn print_search<W: Write>(
    output: &mut W,
    records: &[SearchOutput<'_>],
) -> Result<(), CommandError> {
    if records.is_empty() {
        return write_once(output, "no encyclopedia articles matched\n".into());
    }
    for record in records {
        write_once(output, format!("{}\n{}\n", record.url, record.title))?;
        if !record.summary.is_empty() {
            write_once(output, format!("  {}\n", record.summary))?;
        }
        write_once(
            output,
            format!(
                "  updated:{} views:{} works:{} checklists:{}\n",
                record.updated, record.views, record.works, record.checklists
            ),
        )?;
        if let Some(related) = record.related.as_ref().filter(|items| !items.is_empty()) {
            write_once(output, format!("  related: {}\n", related.join(", ")))?;
        }
    }
    Ok(())
}
