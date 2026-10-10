use crate::{
    CommandError,
    fanbox::{OwnedLease, command_route, fetch_posts, parse_bool, resolve_posts},
    fanbox_auth::AutomaticUpdateHooks,
    startup::StartupHooks,
};
use pixiv_app::{
    fanbox_facade::{Facade, OpenRequest},
    lifecycle::Context,
    scheduler::SchedulerError,
};
use pixiv_sdk::{
    cursor::Cursor,
    fanbox::{Asset, Client, Post},
    resource::SaveOptions,
};
use std::{
    collections::HashSet,
    fmt, fs,
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, Debug)]
pub struct DownloadCommand {
    pub sources: Vec<String>,
    pub proxy: Option<String>,
    pub no_proxy: Option<bool>,
    help: Option<String>,
    help_changed: bool,
}

impl DownloadCommand {
    pub fn parse(args: &[String]) -> Result<Self, CommandError> {
        let normalized = if crate::fanbox::is_fanbox_route(args) {
            args.to_vec()
        } else {
            std::iter::once("fanbox".into())
                .chain(args.iter().cloned())
                .collect()
        };
        let route = command_route(&normalized);
        let leaf = route.path.last().map(String::as_str).unwrap_or("fanbox");
        if leaf != "download" {
            return Err(CommandError::MessageText(format!(
                "unknown command \"{leaf}\" for \"pixiv fanbox\""
            )));
        }
        let mut command = Self {
            sources: vec![],
            proxy: None,
            no_proxy: None,
            help: None,
            help_changed: false,
        };
        let arguments = crate::fanbox::download_flag_args(&route.args);
        let mut index = 0;
        let mut positional = false;
        let mut help = false;
        while index < arguments.len() {
            let argument = &arguments[index];
            index += 1;
            if argument == "--" && !positional {
                positional = true;
                continue;
            }
            if positional || !argument.starts_with('-') || argument == "-" {
                command.sources.push(argument.clone());
                continue;
            }
            let (flag, attached) = argument
                .split_once('=')
                .filter(|(flag, _)| *flag != "-")
                .map_or((argument.as_str(), None), |(flag, value)| {
                    (flag, Some(value))
                });
            if !matches!(flag, "--help" | "-h" | "--proxy" | "--no-proxy") {
                return Err(CommandError::Usage(format!("unknown option '{flag}'")));
            }
            let value = if flag != "--proxy" {
                attached.unwrap_or("true")
            } else if let Some(value) = attached {
                value
            } else {
                let value = arguments.get(index).ok_or_else(|| {
                    CommandError::MessageText(format!("flag needs an argument: {flag}"))
                })?;
                index += 1;
                value
            };
            match flag {
                "--help" | "-h" => {
                    command.help_changed = true;
                    help = parse_bool(value, flag)?;
                }
                "--proxy" => command.proxy = Some(value.into()),
                "--no-proxy" => command.no_proxy = Some(parse_bool(value, flag)?),
                _ => unreachable!(),
            }
        }
        if help {
            command.help = crate::fanbox::help_route(&normalized)?;
        }
        Ok(command)
    }

    pub fn usage(&self) -> &'static str {
        "usage: pixiv fanbox download SOURCE..."
    }

    pub fn requires_startup(&self) -> bool {
        self.help.is_none()
    }

    pub fn resolve_source<R: Read>(
        &mut self,
        input: &mut R,
        terminal: bool,
    ) -> Result<(), CommandError> {
        if self.help.is_some() {
            return Ok(());
        }
        if self.sources.is_empty() && !terminal {
            let mut bytes = Vec::new();
            input
                .read_to_end(&mut bytes)
                .map_err(|error| CommandError::Usage(format!("read stdin value: {error}")))?;
            if bytes.ends_with(b"\r\n") {
                bytes.truncate(bytes.len() - 2);
            } else if bytes.ends_with(b"\n") {
                bytes.pop();
            }
            if !bytes.is_empty() {
                self.sources
                    .push(String::from_utf8_lossy(&bytes).into_owned());
            }
        }
        self.validate()
    }

    pub fn validate(&self) -> Result<(), CommandError> {
        if self.help.is_none() && self.sources.is_empty() {
            return Err(CommandError::Message(self.usage()));
        }
        Ok(())
    }

    pub fn proxy_override(&self) -> Result<Option<&str>, CommandError> {
        if self.proxy.is_some() && self.no_proxy.is_some() {
            return Err(CommandError::Message(
                "use either --proxy or --no-proxy, not both",
            ));
        }
        Ok(if self.no_proxy == Some(true) {
            Some("")
        } else {
            self.proxy.as_deref()
        })
    }

    pub async fn execute<S, W: Write>(
        &self,
        context: &Context,
        download_path: &Path,
        service: S,
        output: &mut W,
    ) -> Result<(), CommandError>
    where
        S: FnOnce() -> Result<Option<Arc<Facade>>, CommandError>,
    {
        self.validate()?;
        if self.write_help(output) {
            return Ok(());
        }
        let facade = service()?.ok_or(CommandError::Message(
            "fanbox account service is not configured",
        ))?;
        let request = OpenRequest {
            proxy_override: self.proxy_override()?.map(str::to_owned),
        };
        let lease = OwnedLease(
            facade
                .open(Some(context), request)
                .await
                .map_err(CommandError::App)?,
        );
        let result = self
            .execute_with_client(context, download_path, lease.0.value(), output)
            .await;
        let closed = lease
            .0
            .close()
            .map_err(|error| CommandError::App(SchedulerError::Shared(error)));
        match (result, closed) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Err(error), Err(closed)) => Err(CommandError::Joined(vec![error, closed])),
        }
    }

    pub async fn execute_with_client<W: Write>(
        &self,
        context: &Context,
        download_path: &Path,
        client: &Client,
        output: &mut W,
    ) -> Result<(), CommandError> {
        self.validate()?;
        if self.write_help(output) {
            return Ok(());
        }
        let base = clean_path(&download_path.join("fanbox"));
        let mut saved = HashSet::new();
        for source in &self.sources {
            let source = resolve_posts(client, context, source).await?;
            let mut cursor = Cursor::default();
            let mut seen = HashSet::new();
            loop {
                if let Some(error) = context.error() {
                    return Err(CommandError::State(Box::new(error)));
                }
                if !seen.insert(cursor.as_str().to_owned()) {
                    return Err(CommandError::Message("pagination cursor repeated"));
                }
                let page = fetch_posts(client, context, &source, cursor).await?;
                for post in &page.items {
                    save_post_assets(context, client, &base, post, &mut saved, output).await?;
                }
                if page.next.is_zero() {
                    break;
                }
                cursor = page.next;
            }
        }
        Ok(())
    }

    pub fn post_success<W: Write>(
        &self,
        context: &Context,
        release: bool,
        hooks: &dyn AutomaticUpdateHooks,
        diagnostics: &mut W,
    ) {
        if !release || self.help_changed || self.help.is_some() {
            return;
        }
        crate::fanbox_auth::run_automatic_update(
            context,
            self.proxy.as_deref(),
            self.no_proxy,
            hooks,
            diagnostics,
        );
    }

    fn write_help<W: Write>(&self, output: &mut W) -> bool {
        let Some(help) = &self.help else {
            return false;
        };
        if let Some((description, rest)) = help.split_once("\n\n") {
            let _ = output.write(format!("{description}\n").as_bytes());
            let _ = output.write(b"\n");
            let _ = output.write(rest.as_bytes());
        } else {
            let _ = output.write(help.as_bytes());
        }
        true
    }
}

pub fn prepare_root<R: Read, W: Write, C>(
    context: &Context,
    args: &[String],
    input: &mut R,
    terminal: bool,
    hooks: &dyn StartupHooks,
    diagnostics: &mut W,
    runtime: C,
) -> Result<DownloadCommand, CommandError>
where
    C: FnOnce() -> Result<(), CommandError>,
{
    let mut command = DownloadCommand::parse(args)?;
    command.resolve_source(input, terminal)?;
    if command.requires_startup() {
        crate::startup::run_startup(context, hooks, diagnostics)?;
        runtime()?;
    }
    Ok(command)
}

async fn save_post_assets<W: Write>(
    context: &Context,
    client: &Client,
    base: &Path,
    post: &Post,
    seen: &mut HashSet<PathBuf>,
    output: &mut W,
) -> Result<(), CommandError> {
    let Some(body) = &post.body else {
        return Ok(());
    };
    let creator = safe_path_segment(&post.creator_id);
    let id = safe_path_segment(&post.id);
    if creator.is_empty() || id.is_empty() {
        return Err(CommandError::MessageText(format!(
            "post {} from creator {} has no usable path identity",
            post.id, post.creator_id
        )));
    }
    let directory = clean_path(&base.join(creator).join(id));
    if !path_inside_base(&directory, base) {
        return Err(CommandError::MessageText(format!(
            "post {} from creator {} resolves outside the download directory",
            post.id, post.creator_id
        )));
    }
    mkdir_all(&directory)?;
    for asset in body.assets.as_deref().unwrap_or_default() {
        if asset.resource.reference.is_zero() {
            continue;
        }
        let name = safe_path_segment(&asset_filename(asset));
        if name.is_empty() {
            return Err(CommandError::MessageText(format!(
                "asset {} in post {} has no usable filename",
                asset.id, post.id
            )));
        }
        let path = clean_path(&directory.join(name));
        if !path_inside_base(&path, base) {
            return Err(CommandError::MessageText(format!(
                "asset {} in post {} resolves outside the download directory",
                asset.id, post.id
            )));
        }
        if !seen.insert(path.clone()) {
            continue;
        }
        client
            .save_resource(
                Arc::new(context.clone()),
                asset.resource.reference.clone(),
                SaveOptions {
                    path: path.to_string_lossy().into_owned(),
                    ..Default::default()
                },
            )
            .await?;
        let _ = output.write(format!("saved: {}\n", path.display()).as_bytes());
    }
    Ok(())
}

fn safe_path_segment(value: &str) -> String {
    let value = value.trim();
    if matches!(value, "" | "." | "..") {
        return String::new();
    }
    let value: String = value
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => character,
        })
        .collect();
    let value = value.trim();
    if matches!(value, "" | "." | "..") {
        String::new()
    } else {
        value.into()
    }
}

fn clean_path(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match result.components().next_back() {
                Some(Component::Normal(_)) => {
                    result.pop();
                }
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => result.push(".."),
            },
            _ => result.push(component.as_os_str()),
        }
    }
    if result.as_os_str().is_empty() {
        result.push(".");
    }
    result
}

fn path_inside_base(target: &Path, base: &Path) -> bool {
    let target = clean_path(target);
    let base = clean_path(base);
    if target == base {
        return true;
    }
    target.strip_prefix(&base).is_ok_and(|relative| {
        !relative.is_absolute()
            && !matches!(relative.components().next(), Some(Component::ParentDir))
    })
}

fn asset_filename(asset: &Asset) -> String {
    if !asset.name.is_empty() {
        return asset.name.clone();
    }
    let extension = asset_extension(&asset.resource.url);
    if !asset.id.is_empty() {
        format!("{}{extension}", asset.id)
    } else {
        format!("asset{extension}")
    }
}

fn asset_extension(raw: &str) -> String {
    let Some((_, remainder)) = raw.split_once("://") else {
        return String::new();
    };
    let start = remainder.find(['/', '?', '#']).unwrap_or(remainder.len());
    let path = &remainder[start..];
    let end = path.find(['?', '#']).unwrap_or(path.len());
    let Some(path) = decode_path(&path[..end]) else {
        return String::new();
    };
    let name = path
        .rsplit(|character| character == '/' || cfg!(windows) && character == '\\')
        .next()
        .unwrap_or_default();
    let Some(dot) = name.rfind('.') else {
        return String::new();
    };
    let extension: String = name[dot..]
        .chars()
        .map(|character| character.to_lowercase().next().unwrap_or(character))
        .collect();
    if (2..=12).contains(&extension.len()) {
        extension
    } else {
        String::new()
    }
}

fn decode_path(value: &str) -> Option<String> {
    let mut decoded = Vec::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = char::from(*bytes.get(index + 1)?).to_digit(16)?;
            let low = char::from(*bytes.get(index + 2)?).to_digit(16)?;
            decoded.push((high * 16 + low) as u8);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    Some(String::from_utf8_lossy(&decoded).into_owned())
}

fn mkdir_all(path: &Path) -> Result<(), CommandError> {
    if let Ok(metadata) = fs::metadata(path) {
        return if metadata.is_dir() {
            Ok(())
        } else {
            Err(path_error(
                path,
                io::Error::from(io::ErrorKind::NotADirectory),
            ))
        };
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        mkdir_all(parent)?;
    }
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o755);
    }
    if let Err(error) = builder.create(path) {
        if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir()) {
            return Ok(());
        }
        return Err(path_error(path, error));
    }
    Ok(())
}

fn path_error(path: &Path, error: io::Error) -> CommandError {
    CommandError::State(Box::new(MkdirError {
        path: path.into(),
        error,
    }))
}

#[derive(Debug)]
struct MkdirError {
    path: PathBuf,
    error: io::Error,
}

impl fmt::Display for MkdirError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self.error.kind() {
            io::ErrorKind::NotFound => "no such file or directory".into(),
            io::ErrorKind::NotADirectory => "not a directory".into(),
            io::ErrorKind::AlreadyExists => "file exists".into(),
            io::ErrorKind::PermissionDenied => "permission denied".into(),
            io::ErrorKind::InvalidInput => "invalid argument".into(),
            _ => {
                let message = self.error.to_string();
                message
                    .rsplit_once(" (os error ")
                    .map_or(message.clone(), |(message, _)| message.into())
                    .to_lowercase()
            }
        };
        write!(formatter, "mkdir {}: {message}", self.path.display())
    }
}

impl std::error::Error for MkdirError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}
