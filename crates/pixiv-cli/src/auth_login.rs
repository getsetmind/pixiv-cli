use crate::{
    CommandError,
    auth_accounts::{AccountOut, AccountPrompts, auth_proxy_override, boolean, print_json},
    terminal_prompt::TerminalPrompts,
};
use pixiv_app::{
    account_login::{LoginCompleteRequest, LoginService},
    account_service::AccountService,
    callback_handler::BrowserOpener,
    config::{Store, parse_duration},
    connection::CommandConnection,
    database::Database,
    lifecycle::Context,
    login_bridge::{
        BridgeError, CallbackAccepter, LoginBridgeHooks, LoginBridgeServer, RelayCleanup,
        wait_for_login_code,
    },
    login_input::validate_login_addr,
    native_browser::NativeBrowserOpener,
    relay_server::{
        RelayLoginServer, RelayServerFlagOverrides, configured_relay_server_options,
        wait_for_handoff_relay_login_code,
    },
    url_handler::{SystemUrlHandler, UrlHandler},
};
use pixiv_sdk::transport::{HttpTransport, Transport};
use std::{
    io::{self, Write},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Default)]
pub struct LoginOptions {
    json: bool,
    json_changed: bool,
    no_open: Option<bool>,
    use_after_login: Option<bool>,
    address: Option<String>,
    timeout: i64,
    proxy: Option<String>,
    no_proxy: Option<bool>,
    relay: RelayServerFlagOverrides,
}

#[derive(Clone, Debug)]
pub enum LoginCommand {
    Run(LoginOptions),
    Help(String),
}
impl LoginCommand {
    pub fn discover(args: &[String]) -> Option<&str> {
        crate::auth_accounts::discover_auth_operation(args)
            .filter(|operation| *operation == "login")
    }

    pub fn parse(args: &[String]) -> Result<Self, CommandError> {
        Self::parse_tracking_json(args, &mut false)
    }

    pub fn machine_output_requested(args: &[String]) -> bool {
        let mut changed = false;
        let _ = Self::parse_tracking_json(args, &mut changed);
        changed
    }

    fn parse_tracking_json(args: &[String], json_changed: &mut bool) -> Result<Self, CommandError> {
        let mut options = LoginOptions::default();
        let mut help = false;
        let mut values = Vec::new();
        let (mut flags, mut command_seen, mut index) = (true, false, 0);
        while index < args.len() {
            let argument = &args[index];
            index += 1;
            if flags && argument == "--" {
                flags = false;
                continue;
            }
            if flags && argument.starts_with("--") {
                let (name, raw) = argument
                    .split_once('=')
                    .map_or((argument.as_str(), None), |(name, raw)| (name, Some(raw)));
                if !command_seen && name != "--help" {
                    return Err(CommandError::Usage(format!("unknown option '{name}'")));
                }
                if matches!(
                    name,
                    "--addr"
                        | "--proxy"
                        | "--timeout"
                        | "--relay-public-url"
                        | "--relay-listen-addr"
                        | "--relay-tls-cert-file"
                        | "--relay-tls-key-file"
                ) {
                    let value = if let Some(raw) = raw {
                        raw.to_owned()
                    } else {
                        let value = args.get(index).ok_or_else(|| {
                            CommandError::MessageText(format!("flag needs an argument: {name}"))
                        })?;
                        index += 1;
                        value.clone()
                    };
                    match name {
                        "--addr" => options.address = Some(value),
                        "--proxy" => options.proxy = Some(value),
                        "--relay-public-url" => options.relay.public_url = Some(value),
                        "--relay-listen-addr" => options.relay.listen_addr = Some(value),
                        "--relay-tls-cert-file" => options.relay.tls_cert_file = Some(value),
                        "--relay-tls-key-file" => options.relay.tls_key_file = Some(value),
                        "--timeout" => {
                            options.timeout = parse_duration(&value).map_err(|error| {
                                CommandError::MessageText(format!(
                                    "invalid argument {} for \"--timeout\" flag: {error}",
                                    crate::search::quote(&value)
                                ))
                            })?;
                        }
                        _ => unreachable!(),
                    }
                    continue;
                }
                let value = match name {
                    "--help" | "--json" | "--no-open" | "--use" | "--no-proxy" => boolean(
                        raw.unwrap_or("true"),
                        if name == "--help" { "-h, --help" } else { name },
                    )?,
                    _ => return Err(CommandError::Usage(format!("unknown option '{name}'"))),
                };
                match name {
                    "--help" => help = value,
                    "--json" => {
                        options.json = value;
                        options.json_changed = true;
                        *json_changed = true;
                    }
                    "--no-open" => options.no_open = Some(value),
                    "--use" => options.use_after_login = Some(value),
                    "--no-proxy" => options.no_proxy = Some(value),
                    _ => unreachable!(),
                }
                continue;
            }
            if flags && argument.starts_with('-') && argument != "-" {
                let mut characters = argument[1..].chars().peekable();
                while let Some(character) = characters.next() {
                    if character != 'h' {
                        return Err(CommandError::Usage(format!(
                            "unknown option '-{character}'"
                        )));
                    }
                    if characters.peek() == Some(&'=') {
                        characters.next();
                        help = boolean(&characters.collect::<String>(), "-h, --help")?;
                        break;
                    }
                    help = true;
                }
                continue;
            }
            command_seen = true;
            values.push(argument);
        }
        if help {
            return Ok(Self::Help(help_text().into()));
        }
        if values.len() != 1 {
            return Err(CommandError::Message(
                "usage: pixiv auth login [--json] [--no-open] [--addr 127.0.0.1:0] [--use] [--timeout DURATION] [--relay-public-url URL] [--relay-listen-addr ADDR]",
            ));
        }
        Ok(Self::Run(options))
    }

    pub fn machine_output(&self) -> bool {
        matches!(
            self,
            Self::Run(LoginOptions {
                json_changed: true,
                ..
            })
        )
    }

    pub fn requires_config(&self) -> bool {
        matches!(self, Self::Run(_))
    }

    pub async fn execute_http<W: Write>(
        &self,
        store: &Store,
        context: &Context,
        out: &mut W,
    ) -> Result<(), CommandError> {
        self.execute_with_factory(
            store,
            context,
            out,
            Arc::new(SystemLoginHooks::new()),
            |proxy| {
                match proxy {
                    Some(proxy) => HttpTransport::new(Some(proxy)),
                    None => HttpTransport::new_with_environment_proxy(),
                }
                .map_err(Into::into)
            },
        )
        .await
    }

    pub async fn execute_with_transport<T: Transport, W: Write>(
        &self,
        store: &Store,
        context: &Context,
        out: &mut W,
        hooks: Arc<dyn LoginBridgeHooks>,
        transport: T,
    ) -> Result<(), CommandError> {
        let mut transport = Some(transport);
        self.execute_with_factory(store, context, out, hooks, |_| {
            Ok(transport.take().expect("one login transport"))
        })
        .await
    }

    pub async fn execute_with_factory<
        T: Transport,
        W: Write,
        F: FnMut(Option<&str>) -> Result<T, CommandError>,
    >(
        &self,
        store: &Store,
        _context: &Context,
        out: &mut W,
        hooks: Arc<dyn LoginBridgeHooks>,
        mut factory: F,
    ) -> Result<(), CommandError> {
        let Self::Run(options) = self else {
            let Self::Help(help) = self else {
                unreachable!()
            };
            let _ = out.write_all(help.as_bytes());
            return Ok(());
        };
        let proxy = auth_proxy_override(&options.proxy, options.no_proxy)?;
        store
            .ensure_defaults()
            .map_err(|error| CommandError::State(Box::new(error)))?;
        let database = Database::open(store.path().parent().unwrap())
            .map_err(|error| CommandError::State(Box::new(error)))?;
        let runtime = store
            .current()
            .and_then(|snapshot| snapshot.runtime())
            .map_err(pixiv_app::scheduler::SchedulerError::from)?;
        let relay = configured_relay_server_options(&options.relay, &runtime)
            .map_err(|error| CommandError::State(Box::new(error)))?;
        let address = options.address.as_deref().unwrap_or("127.0.0.1:0");
        if relay.is_none() {
            validate_login_addr(address).map_err(|error| CommandError::State(Box::new(error)))?;
        }
        if let Some(proxy) = proxy {
            CommandConnection::resolve(&runtime, Some(proxy))
                .map_err(|error| CommandError::State(Box::new(error)))?;
        }
        let transport = factory(proxy)?;
        let defaults = store.clone();
        let accounts = AccountService {
            repository: Arc::new(Mutex::new(database)),
            defaults: Some(Arc::new(move || {
                defaults.read_pixiv_default_user_id().map_err(Into::into)
            })),
        };
        let service = LoginService {
            pixiv: Some(&accounts),
            defaults: Some(store),
        };
        let start = service.start()?;
        let background = if options.timeout > 0 {
            Context::with_deadline(Instant::now() + Duration::from_nanos(options.timeout as u64))
        } else {
            Context::new()
        };
        let accepts_start = start.clone();
        let accepts: CallbackAccepter =
            Arc::new(move |callback| accepts_start.accepts_callback_url(callback));
        let (code, server) = if let Some(relay) = relay {
            let result = wait_for_handoff_relay_login_code(
                &background,
                relay,
                Some(accepts),
                &start.authorization_url,
                hooks,
            )
            .await
            .map_err(CommandError::State)?;
            (result.code, LoginServer::Relay(result.server))
        } else {
            let result = wait_for_login_code(
                &background,
                address,
                Some(accepts),
                &start.authorization_url,
                options.no_open.unwrap_or(!runtime.login_open_browser),
                hooks,
            )
            .await
            .map_err(CommandError::State)?;
            (result.code, LoginServer::Local(result.server))
        };
        let result = service
            .complete(
                &background,
                &start,
                &LoginCompleteRequest {
                    callback_or_code: code,
                    use_after_login: options
                        .use_after_login
                        .unwrap_or(runtime.login_use_after_login),
                },
                &transport,
            )
            .await;
        let result = match result {
            Err(error) => {
                server.notify_final(false).await;
                Err(error.into())
            }
            Ok(account) => {
                server.notify_final(true).await;
                let account = AccountOut::from(account);
                if options.json {
                    print_json(out, &account)
                } else {
                    let _ = write!(out, "✓ uid:{}", account.user_id);
                    if !account.username.is_empty() {
                        let _ = write!(out, " username:{}", account.username);
                    }
                    let _ = writeln!(out);
                    Ok(())
                }
            }
        };
        server.cleanup().await;
        result
    }
}

enum LoginServer {
    Local(LoginBridgeServer),
    Relay(RelayLoginServer),
}
impl LoginServer {
    async fn notify_final(&self, success: bool) {
        match self {
            Self::Local(server) => server.notify_final(success).await,
            Self::Relay(server) => server.notify_final(success).await,
        }
    }
    async fn cleanup(&self) {
        match self {
            Self::Local(server) => server.cleanup().await,
            Self::Relay(server) => server.cleanup().await,
        }
    }
}

struct SystemLoginHooks {
    prompts: Mutex<TerminalPrompts>,
    handler: SystemUrlHandler,
}
impl SystemLoginHooks {
    fn new() -> Self {
        Self {
            prompts: Mutex::new(TerminalPrompts::new(
                io::stdin(),
                io::stdout(),
                io::stderr(),
            )),
            handler: SystemUrlHandler::system(),
        }
    }
}
impl LoginBridgeHooks for SystemLoginHooks {
    fn diagnostic(&self, message: &str) {
        let _ = io::stderr().lock().write_all(message.as_bytes());
    }
    fn open_browser(&self, url: &str) -> Result<(), BridgeError> {
        NativeBrowserOpener::default().open(url)
    }
    fn ensure_scheme_relay(&self, context: &Context) -> Result<(), BridgeError> {
        self.handler.ensure_if_needed(context)
    }
    fn install_scheme_relay(
        &self,
        context: &Context,
        callback: &str,
    ) -> Result<Option<RelayCleanup>, BridgeError> {
        let mut installation = self.handler.install(context, callback)?;
        Ok(Some(Box::new(move || installation.cleanup())))
    }
    fn can_prompt(&self) -> bool {
        self.prompts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .can_prompt()
    }
    fn prompt_input(&self, message: &str, default: &str) -> Result<String, BridgeError> {
        self.prompts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .input(message, default)
            .map_err(|error| Box::new(error) as BridgeError)
    }
    fn output(&self, message: &str) {
        let _ = io::stdout().lock().write_all(message.as_bytes());
    }
}

fn help_text() -> &'static str {
    "Login with the Pixiv browser OAuth flow\n\nUsage:\n  pixiv auth login [flags]\n\nExamples:\npixiv auth login --use\n\nFlags:\n      --addr string                  local loopback callback address; use 127.0.0.1:0 for an available port (default \"127.0.0.1:0\")\n  -h, --help                         help for login\n      --json                         print JSON\n      --no-open                      do not open the browser\n      --no-proxy                     clear the configured proxy for this command\n      --proxy string                 proxy URL (http, https, socks5, or socks5h) for this command\n      --relay-listen-addr string     listen address for this remote login relay\n      --relay-public-url string      public URL for this remote login relay\n      --relay-tls-cert-file string   PEM certificate file for this remote login relay\n      --relay-tls-key-file string    PEM private key file for this remote login relay\n      --timeout duration             maximum time to wait for login flow; 0 adds no deadline\n      --use                          set as default account after login\n"
}
