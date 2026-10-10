use crate::CommandError;

pub struct Route {
    pub path: Vec<String>,
    pub args: Vec<String>,
}
fn children(path: &[String]) -> &'static [&'static str] {
    match path
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [] => &["fanbox"],
        ["fanbox"] => &[
            "auth",
            "creators",
            "download",
            "home",
            "mcp",
            "post",
            "posts",
            "supporting",
            "tags",
        ],
        ["fanbox", "auth"] => &["import", "list", "remove", "status", "use"],
        _ => &[],
    }
}
fn no_value(flag: &str, path: &[String]) -> bool {
    if flag == "--no-proxy" {
        return !path.is_empty();
    }
    let leaf = path.last().map(String::as_str).unwrap_or_default();
    matches!(
        leaf,
        "creators"
            | "home"
            | "post"
            | "posts"
            | "supporting"
            | "tags"
            | "import"
            | "list"
            | "status"
            | "use"
            | "remove"
            | "download"
    ) && matches!(
        flag,
        "--json" | "--ndjson" | "--default" | "--auto" | "--yes"
    )
}
pub fn command_route(args: &[String]) -> Route {
    let mut path = vec![];
    let mut remaining = args.to_vec();
    loop {
        let mut index = 0;
        let mut found = None;
        while index < remaining.len() {
            let token = &remaining[index];
            index += 1;
            if token == "--" {
                break;
            }
            if token.starts_with('-') && !token.contains('=') && !no_value(token, &path) {
                if remaining.len() - index <= 1 {
                    break;
                }
                index += 1;
                continue;
            }
            if !token.is_empty() && !token.starts_with('-') {
                found = Some(index - 1);
                break;
            }
        }
        let Some(index) = found else {
            break;
        };
        if !children(&path).contains(&remaining[index].as_str()) {
            break;
        }
        path.push(remaining.remove(index));
    }
    Route {
        path,
        args: remaining,
    }
}
pub fn is_fanbox_route(args: &[String]) -> bool {
    command_route(args)
        .path
        .first()
        .is_some_and(|path| path == "fanbox")
}

pub(crate) fn download_flag_args(args: &[String]) -> Vec<String> {
    let mut expanded = Vec::new();
    let mut positional = false;
    let mut proxy_value = false;
    for argument in args {
        if positional || proxy_value {
            expanded.push(argument.clone());
            proxy_value = false;
            continue;
        }
        if argument == "--" {
            positional = true;
        }
        if argument == "--proxy" {
            proxy_value = true;
        }
        if let Some(short) = argument
            .strip_prefix('-')
            .filter(|short| short.starts_with('h'))
        {
            let mut rest = short;
            while let Some(suffix) = rest.strip_prefix('h') {
                if let Some(value) = suffix.strip_prefix('=').filter(|value| !value.is_empty()) {
                    expanded.push(format!("-h={value}"));
                    rest = "";
                    break;
                }
                expanded.push("-h".into());
                rest = suffix;
            }
            if let Some(unknown) = rest.chars().next() {
                expanded.push(format!("-{unknown}"));
            }
        } else if let Some(short) = argument
            .strip_prefix('-')
            .filter(|short| !short.starts_with('-') && !short.is_empty())
        {
            expanded.push(format!("-{}", short.chars().next().unwrap()));
        } else {
            expanded.push(argument.clone());
        }
    }
    expanded
}

pub fn help_route(args: &[String]) -> Result<Option<String>, CommandError> {
    let route = command_route(args);
    if route.path.first().is_none_or(|part| part != "fanbox") {
        return Ok(None);
    }
    let leaf = route.path.last().map(String::as_str).unwrap_or("fanbox");
    let mut help = false;
    let arguments = if leaf == "download" {
        download_flag_args(&route.args)
    } else {
        route.args.clone()
    };
    let mut positions = 0;
    let mut index = 0;
    while index < arguments.len() {
        let token = &arguments[index];
        index += 1;
        if token == "--" {
            positions += arguments.len() - index;
            break;
        }
        if !token.starts_with('-') || token == "-" {
            positions += 1;
            continue;
        }
        let (flag, value) = token
            .split_once('=')
            .filter(|(flag, _)| *flag != "-")
            .map_or((token.as_str(), None), |(flag, value)| (flag, Some(value)));
        let boolean = matches!(
            flag,
            "--help"
                | "-h"
                | "--no-proxy"
                | "--json"
                | "--ndjson"
                | "--default"
                | "--auto"
                | "--yes"
        );
        let valid = matches!(flag, "--help" | "-h" | "--proxy" | "--no-proxy")
            || flags(leaf)
                .iter()
                .any(|item| item.name == flag.trim_start_matches('-'));
        if !valid {
            return Err(CommandError::Usage(format!("unknown option '{flag}'")));
        }
        let value = if boolean {
            value.unwrap_or("true")
        } else if let Some(value) = value {
            value
        } else {
            let value = arguments.get(index).ok_or_else(|| {
                CommandError::MessageText(format!("flag needs an argument: {flag}"))
            })?;
            index += 1;
            value
        };
        if boolean {
            let enabled = super::parse_bool(value, flag)?;
            if matches!(flag, "--help" | "-h") {
                help = enabled;
            }
        } else if matches!(flag, "--limit" | "--page") {
            super::parse_integer(value, flag)?;
        }
    }
    let group = matches!(leaf, "fanbox" | "auth");
    if help || group && positions == 0 {
        return Ok(Some(render(&route.path)));
    }
    Ok(None)
}
struct Flag {
    name: &'static str,
    short: bool,
    kind: &'static str,
    description: &'static str,
}
const PROXY: Flag = Flag {
    name: "proxy",
    short: false,
    kind: "string",
    description: "native FANBOX proxy URL (HTTP or HTTPS CONNECT)",
};
const NO_PROXY: Flag = Flag {
    name: "no-proxy",
    short: false,
    kind: "",
    description: "use a direct native FANBOX connection for this command",
};
fn flags(leaf: &str) -> Vec<Flag> {
    let mut flags = vec![];
    if matches!(
        leaf,
        "creators"
            | "home"
            | "post"
            | "posts"
            | "supporting"
            | "tags"
            | "import"
            | "list"
            | "status"
            | "use"
            | "remove"
    ) {
        flags.push(Flag {
            name: "json",
            short: false,
            kind: "",
            description: "print JSON",
        });
    }
    if matches!(
        leaf,
        "creators" | "home" | "post" | "posts" | "supporting" | "tags"
    ) {
        flags.push(Flag {
            name: "ndjson",
            short: false,
            kind: "",
            description: "print one item as JSON per line",
        });
    }
    if matches!(leaf, "creators" | "home" | "posts" | "supporting") {
        flags.extend([Flag{name:"limit",short:false,kind:"int",description:"maximum results; omitted returns one upstream batch; 0 returns all results"},Flag{name:"page",short:false,kind:"int",description:"1-based logical page (requires --limit > 0)"}]);
    }
    if leaf == "creators" {
        flags.push(Flag {
            name: "kind",
            short: false,
            kind: "string",
            description: "creator list kind: supporting or following (default \"supporting\")",
        });
    }
    if leaf == "use" {
        flags.push(Flag {
            name: "auto",
            short: false,
            kind: "",
            description: "clear the explicit default and use the first stored account",
        });
    }
    if leaf == "remove" {
        flags.push(Flag {
            name: "yes",
            short: false,
            kind: "",
            description: "skip confirmation in interactive terminals",
        });
    }
    if leaf == "import" {
        flags.extend([
            Flag {
                name: "default",
                short: false,
                kind: "",
                description: "set the imported account as the default FANBOX account",
            },
            Flag {
                name: "from-browser",
                short: false,
                kind: "string",
                description: "read the FANBOXSESSID value from a browser profile",
            },
            Flag {
                name: "profile",
                short: false,
                kind: "string",
                description: "browser profile identifier when the browser has multiple profiles",
            },
        ]);
    }
    flags
}
fn short(leaf: &str) -> &'static str {
    match leaf {
        "fanbox" => "Browse and download Pixiv FANBOX content",
        "auth" => "Manage local FANBOX authentication",
        "creators" => "List supporting or following FANBOX creators",
        "download" => "Download posts and their assets from FANBOX",
        "home" => "Browse the FANBOX home feed",
        "mcp" => "Run the FANBOX MCP stdio server",
        "post" => "Show one FANBOX post",
        "posts" => "List posts from a creator, tag, post, or FANBOX URL",
        "supporting" => "Browse posts from supporting creators",
        "tags" => "List tags used by a FANBOX creator",
        "import" => "Import a FANBOX session",
        "list" => "List FANBOX accounts",
        "remove" => "Remove a FANBOX account",
        "status" => "Show the default FANBOX account",
        "use" => "Set the default FANBOX account",
        _ => "",
    }
}
fn render_flags(mut flags: Vec<Flag>) -> String {
    flags.sort_by_key(|flag| flag.name);
    let rows = flags
        .into_iter()
        .map(|flag| {
            let label = format!(
                "{}--{}{}{}",
                if flag.short { "  -h, " } else { "      " },
                flag.name,
                if flag.kind.is_empty() { "" } else { " " },
                flag.kind
            );
            (label, flag.description)
        })
        .collect::<Vec<_>>();
    let width = rows.iter().map(|(label, _)| label.len()).max().unwrap_or(0);
    rows.into_iter()
        .map(|(label, description)| {
            format!(
                "{label}{}{description}\n",
                " ".repeat(width - label.len() + 3)
            )
        })
        .collect()
}
fn render(path: &[String]) -> String {
    let leaf = path.last().map(String::as_str).unwrap_or("fanbox");
    let command = format!("pixiv {}", path.join(" "));
    let group = matches!(leaf, "fanbox" | "auth");
    let parameter = match leaf {
        "post" => " POST_ID",
        "posts" => " SOURCE",
        "download" => " SOURCE...",
        "tags" => " CREATOR",
        "status" => " [UID]",
        "remove" => " UID",
        "use" => " [UID]",
        _ => "",
    };
    let mut result = format!(
        "{}\n\nUsage:\n  {command}{parameter} [flags]\n",
        short(leaf)
    );
    if group {
        result.push_str(&format!("  {command} [command]\n\nAvailable Commands:\n"));
        let commands = children(path);
        let width = commands
            .iter()
            .map(|name| name.len() + 2)
            .max()
            .unwrap_or(0)
            .max(12);
        for name in commands {
            result.push_str(&format!("  {name:<width$}{}\n", short(name)));
        }
    }
    if matches!(leaf, "mcp" | "import") {
        result.push_str(&format!("\nExamples:\n{command}\n"));
    }
    let mut local = flags(leaf);
    local.push(Flag {
        name: "help",
        short: true,
        kind: "",
        description: match leaf {
            "fanbox" => "help for fanbox",
            "auth" => "help for auth",
            "mcp" => "help for mcp",
            "import" => "help for import",
            "status" => "help for status",
            "creators" => "help for creators",
            "post" => "help for post",
            "posts" => "help for posts",
            "home" => "help for home",
            "supporting" => "help for supporting",
            "tags" => "help for tags",
            "download" => "help for download",
            "list" => "help for list",
            "remove" => "help for remove",
            "use" => "help for use",
            _ => "help",
        },
    });
    if leaf == "fanbox" {
        local.extend([NO_PROXY, PROXY]);
    }
    result.push_str("\nFlags:\n");
    result.push_str(&render_flags(local));
    if leaf != "fanbox" {
        result.push_str("\nGlobal Flags:\n");
        result.push_str(&render_flags(vec![NO_PROXY, PROXY]));
    }
    if group {
        result.push_str(&format!(
            "\nUse \"{command} [command] --help\" for more information about a command.\n"
        ));
    }
    result
}
