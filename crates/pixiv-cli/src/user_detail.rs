use crate::{CommandError, DetailOutput};
use pixiv_app::{execution::Execution, lifecycle::Context};
use pixiv_sdk::{Client, models::UserDetail, pixiv::UserRequest, transport::Transport};
use std::io::Write;
pub async fn user_detail<T: Transport, W: Write>(
    client: &Client<T>,
    id: i64,
    mode: DetailOutput,
    out: &mut W,
) -> Result<(), CommandError> {
    let user = client.user(UserRequest { user_id: id }).await?;
    write(&user, mode, false, out)
}
pub async fn saved_user_detail<T: Transport + 'static, W: Write>(
    execution: &Execution<T>,
    context: &Context,
    id: i64,
    user_id: i64,
    proxy: Option<&str>,
    mode: DetailOutput,
    out: &mut W,
) -> Result<(), CommandError> {
    saved_detail(execution, context, id, user_id, proxy, (mode, false), out).await
}
async fn saved_detail<T: Transport + 'static, W: Write>(
    execution: &Execution<T>,
    context: &Context,
    id: i64,
    user_id: i64,
    proxy: Option<&str>,
    presentation: (DetailOutput, bool),
    out: &mut W,
) -> Result<(), CommandError> {
    let user = execution
        .read(context, user_id, proxy, move |_, client| async move {
            client
                .user(UserRequest { user_id: id })
                .await
                .map_err(Into::into)
        })
        .await?;
    write(&user, presentation.0, presentation.1, out)
}
fn write<W: Write>(
    user: &UserDetail,
    mode: DetailOutput,
    safe: bool,
    out: &mut W,
) -> Result<(), CommandError> {
    match mode {
        DetailOutput::Json => writeln!(
            out,
            "{}",
            crate::go_json_escape(
                serde_json::to_string_pretty(&pixiv_sdk::dto::UserDetailDto::from(user))
                    .map_err(std::io::Error::other)?
            )
        )?,
        DetailOutput::Ndjson => writeln!(
            out,
            "{}",
            crate::go_json_escape(
                serde_json::to_string(
                    &pixiv_record::from_user_detail(user)
                        .map_err(|error| CommandError::Message(error.message()))?
                )
                .map_err(std::io::Error::other)?
            )
        )?,
        DetailOutput::Human => {
            writeln!(out, "user id: {}", user.user.id)?;
            for (name, value) in [
                ("name", user.user.name.as_str()),
                ("account", &user.user.account),
                ("comment", &user.user.comment),
                ("webpage", &public_webpage(&user.profile.webpage)),
                ("region", &user.profile.region),
                ("country", &user.profile.country_code),
                ("job", &user.profile.job),
            ] {
                if !value.is_empty() {
                    writeln!(
                        out,
                        "{name}: {}",
                        if safe {
                            crate::safe_line(value)
                        } else {
                            value.into()
                        }
                    )?;
                }
            }
            for (name, value) in [
                ("artworks", user.profile.total_illusts),
                ("manga", user.profile.total_manga),
                ("novels", user.profile.total_novels),
                ("following", user.profile.total_follow_users),
            ] {
                writeln!(out, "{name}: {value}")?;
            }
            for (name, value) in [
                ("pc", &user.workspace.pc),
                ("monitor", &user.workspace.monitor),
                ("tool", &user.workspace.tool),
                ("scanner", &user.workspace.scanner),
                ("tablet", &user.workspace.tablet),
                ("mouse", &user.workspace.mouse),
                ("printer", &user.workspace.printer),
                ("desktop", &user.workspace.desktop),
                ("music", &user.workspace.music),
                ("desk", &user.workspace.desk),
                ("chair", &user.workspace.chair),
                ("comment", &user.workspace.comment),
            ] {
                if !value.is_empty() {
                    writeln!(
                        out,
                        "workspace {name}: {}",
                        if safe {
                            crate::safe_line(value)
                        } else {
                            value.into()
                        }
                    )?;
                }
            }
        }
    }
    Ok(())
}
fn valid_escapes(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.iter().enumerate().all(|(index, byte)| {
        *byte != b'%'
            || bytes.get(index + 1).is_some_and(u8::is_ascii_hexdigit)
                && bytes.get(index + 2).is_some_and(u8::is_ascii_hexdigit)
    })
}
fn public_webpage(raw: &str) -> String {
    let Some((scheme, rest)) = raw.split_once("://") else {
        return String::new();
    };
    let scheme = scheme.to_ascii_lowercase();
    if !matches!(scheme.as_str(), "http" | "https") || raw.bytes().any(|b| b < 32 || b == 127) {
        return String::new();
    }
    if raw
        .split_once('#')
        .is_some_and(|(_, fragment)| !valid_escapes(fragment))
    {
        return String::new();
    }
    let boundary = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let original_authority = &rest[..boundary];
    if original_authority.bytes().any(|b| b == b' ') || !valid_escapes(original_authority) {
        return String::new();
    }
    let authority = original_authority.rsplit('@').next().unwrap_or_default();
    if authority.is_empty()
        || authority.bytes().any(|b| {
            matches!(
                b,
                b'<' | b'>' | b'"' | b'\\' | b'^' | b'`' | b'{' | b'|' | b'}'
            )
        })
    {
        return String::new();
    }
    if let Some((_, port)) = authority.rsplit_once(':')
        && (!authority.ends_with(']') && !port.bytes().all(|b| b.is_ascii_digit()))
    {
        return String::new();
    }
    let path = rest[boundary..]
        .split(['?', '#'])
        .next()
        .unwrap_or_default();
    if !valid_escapes(path) {
        return String::new();
    }
    let mut escaped = String::new();
    for byte in authority.bytes() {
        if byte >= 128 {
            escaped.push_str(&format!("%{byte:02X}"));
        } else {
            escaped.push(byte as char);
        }
    }
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'_'
                    | b'.'
                    | b'~'
                    | b'!'
                    | b'$'
                    | b'&'
                    | b'\''
                    | b'('
                    | b')'
                    | b'*'
                    | b'+'
                    | b','
                    | b';'
                    | b'='
                    | b':'
                    | b'@'
                    | b'/'
                    | b'%'
            )
        {
            escaped.push(byte as char);
        } else {
            escaped.push_str(&format!("%{byte:02X}"));
        }
    }
    format!("{scheme}://{escaped}")
}

#[derive(clap::Args)]
pub struct UserDetailOptions {
    #[arg(num_args = 0..)]
    pub sources: Vec<String>,
    #[arg(long, short = 'j', num_args = 0..=1, require_equals = true, default_missing_value = "true")]
    pub json: Option<bool>,
}
impl UserDetailOptions {
    pub fn resolve_source<R: std::io::Read>(
        &self,
        input: &mut R,
        terminal: bool,
    ) -> Result<String, CommandError> {
        if self.sources.len() > 1 {
            return Err(CommandError::Message(
                "usage: pixiv user detail [options] USER_ID",
            ));
        }
        let source = match self.sources.first() {
            Some(source) => source.clone(),
            None => crate::search::read_text_value(
                input,
                terminal,
                "usage: pixiv user detail [options] USER_ID",
                "stdin user ID is not valid UTF-8",
            )?,
        };
        Ok(source)
    }
}
pub async fn saved_user_profile<T: Transport + 'static, W: Write>(
    execution: &Execution<T>,
    context: &Context,
    id: i64,
    proxy: Option<&str>,
    mode: DetailOutput,
    out: &mut W,
) -> Result<(), CommandError> {
    saved_detail(execution, context, id, 0, proxy, (mode, true), out).await
}

pub fn profile_user_id(source: &str) -> Result<i64, CommandError> {
    if source.trim().is_empty() {
        return Err(CommandError::LabeledSdk(
            "user_id",
            pixiv_sdk::Error::new(pixiv_sdk::Reason::InvalidArgument, "user detail")
                .with_detail("input value is required"),
        ));
    }
    crate::user_works::resolved_user_id(Some(&source.to_owned()), "user detail")
}
