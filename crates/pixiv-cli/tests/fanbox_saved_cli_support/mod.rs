use pixiv_app::{
    config::Store,
    database::Database,
    fanbox_account_service::AccountService,
    fanbox_facade::{AccountOpener, Facade},
    lifecycle::Context,
    scheduler::SchedulerError,
    sessions::ClientOpen,
};
use pixiv_cli_rs::{CommandError, fanbox::ReadCommand, finish_command};
use pixiv_sdk::fanbox::{
    Client,
    transport::{
        BodyFuture, ExternalError, RawBody, RawRead, RawRequest, RawResponse, RawTransport,
        TransportFuture,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{self, IsTerminal, Read, Write},
    path::Path,
    sync::{Arc, Mutex},
    thread::JoinHandle,
};

#[derive(Clone, Deserialize)]
pub struct Input {
    pub boundary: String,
    pub args: Vec<String>,
    pub config: String,
    pub saved: String,
    #[serde(default)]
    pub db_failure: String,
    #[serde(default)]
    pub stdin: String,
    #[serde(default)]
    pub stdin_error: bool,
    pub replies: Vec<Reply>,
    #[serde(default)]
    pub writer: String,
    #[serde(default)]
    pub write_limit: usize,
    #[serde(default)]
    pub close_error: bool,
    #[serde(default)]
    pub options_error: bool,
    #[serde(default)]
    pub cancel: String,
    #[serde(default)]
    pub bad_temp: bool,
    #[serde(default)]
    pub output_pipe: bool,
    #[serde(default)]
    pub repeat: usize,
}
#[derive(Clone, Default, Deserialize)]
pub struct Reply {
    #[serde(default)]
    body: String,
    #[serde(default)]
    status: u16,
    #[serde(default)]
    transport_error: String,
    #[serde(default)]
    read_error: String,
    #[serde(default)]
    close_error: String,
}
#[derive(Default, Serialize)]
pub struct Observation {
    pub exits: Vec<i32>,
    pub stdout: String,
    pub stderr: String,
    pub errors: Vec<String>,
    pub reasons: Vec<String>,
    pub trace: Vec<String>,
    pub requests: Vec<Value>,
    pub options: Vec<Value>,
    pub proxy_overrides: Vec<Option<String>>,
    pub stdin_reads: usize,
    pub writes: Vec<usize>,
    pub body_closes: usize,
    pub lease_closes: usize,
    pub unique_clients: usize,
    pub output_is_tty: bool,
    pub auto_ndjson: bool,
    pub idle_closes: usize,
    pub db_before: String,
    pub db_after: String,
    pub db_rows_before: Vec<String>,
    pub db_rows_after: Vec<String>,
    pub config_after: String,
    pub remaining_temps: Vec<String>,
    pub socket_denied: bool,
    pub exec_denied: bool,
}
type Shared = Arc<Mutex<Observation>>;

pub async fn observe(home: &Path, input: Input) -> Observation {
    let directory = home.join(".pixiv-cli");
    if directory.exists() {
        std::fs::remove_dir_all(&directory).unwrap();
    }
    seed(&directory, &input);
    let path = directory.join("pixiv-cli.db");
    let observation = Arc::new(Mutex::new(Observation {
        db_before: hash(&path),
        db_rows_before: rows(&path),
        ..Default::default()
    }));
    let context = Context::new();
    if input.cancel == "before" {
        context.cancel();
    }
    let transport = Arc::new(Transport {
        input: input.clone(),
        observation: observation.clone(),
        index: Mutex::new(0),
        context: context.clone(),
    });
    let clients = Arc::new(Mutex::new(Vec::<Arc<Client>>::new()));
    let mut reader = Reader {
        input: io::Cursor::new(input.stdin.as_bytes().to_vec()),
        fail: input.stdin_error,
        observation: observation.clone(),
    };
    let mut writer = Writer::new(&input, observation.clone());
    observation.lock().unwrap().output_is_tty = writer.is_terminal();
    let temp = std::env::temp_dir();
    if input.bad_temp {
        std::fs::remove_dir(&temp).unwrap();
        std::fs::write(&temp, b"owned temp blocker").unwrap();
    }
    let mut diagnostics = Vec::new();
    for _ in 0..input.repeat.max(1) {
        let parsed = ReadCommand::parse(&input.args);
        let (ndjson, machine) = parsed
            .as_ref()
            .map(|command| (command.ndjson, command.ndjson || command.json == Some(true)))
            .unwrap_or_default();
        let result = match parsed {
            Err(error) => Err(error),
            Ok(mut command) => match command.resolve_source(&mut reader, false) {
                Err(error) => Err(error),
                Ok(()) => {
                    let observing = observation.clone();
                    let input = input.clone();
                    let directory = directory.clone();
                    let transport = transport.clone();
                    let clients = clients.clone();
                    command.execute(&context, move || {
                        observing.lock().unwrap().trace.push("service.open".into());
                        let database = Database::open(&directory).map_err(|error| CommandError::MessageText(error.to_string()))?;
                        let store = Arc::new(Store::new(directory.join("config.toml")));
                        let mut accounts = AccountService::from_store(Arc::new(Mutex::new(database)), store);
                        let original = accounts.load_options.take().unwrap();
                        let options_observation = observing.clone();
                        let options_transport = transport.clone();
                        accounts.load_options = Some(Arc::new(move || {
                            options_observation.lock().unwrap().trace.push("options.load".into());
                            if input.options_error { return Err(SchedulerError::Message("owned options failure".into())); }
                            options_observation.lock().unwrap().trace.push("runtime.read".into());
                            let mut options = original()?;
                            let solver = options.flare_solverr.as_ref().map(|solver| json!({"url":solver.url,"proxy_url":solver.proxy_url}));
                            options_observation.lock().unwrap().options.push(json!({"proxy_url": options.proxy_url,"user_agent":options.user_agent,"solver":solver}));
                            options.http_client = Some(options_transport.clone());
                            Ok(options)
                        }));
                        let opener = Arc::new(Opener { accounts, observation: observing.clone(), clients });
                        Ok(Some(Arc::new(Facade::with_close_client(Some(opener), Some(Arc::new(move |client| {
                            { let mut observing = observing.lock().unwrap(); observing.lease_closes += 1; observing.trace.push("lease.close".into()); }
                            client.close_idle_connections();
                            if input.close_error { Err(SchedulerError::Message("owned lease close failure".into())) } else { Ok(()) }
                        }))))))
                    }, &mut writer).await
                }
            },
        };
        if input.bad_temp {
            let Err(CommandError::Output(error)) = &result else {
                panic!("physical bad TMPDIR must retain the OS output error");
            };
            assert_eq!(error.kind(), io::ErrorKind::NotADirectory);
            assert_eq!(error.raw_os_error(), Some(20));
        }
        {
            let mut observed = observation.lock().unwrap();
            observed.errors.push(
                result
                    .as_ref()
                    .err()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
            );
            observed.reasons.push(
                result
                    .as_ref()
                    .err()
                    .and_then(CommandError::sdk_error)
                    .map(|error| error.code.as_str().to_owned())
                    .unwrap_or_default(),
            );
        }
        let exit = finish_command(result, ndjson, machine, &mut diagnostics);
        observation.lock().unwrap().exits.push(exit);
    }
    let stdout = writer.finish();
    drop(reader);
    drop(transport);
    drop(clients);
    let mut observed = Arc::try_unwrap(observation)
        .ok()
        .unwrap()
        .into_inner()
        .unwrap();
    observed.stdout = String::from_utf8(stdout).unwrap();
    observed.stderr = String::from_utf8(diagnostics).unwrap();
    observed.db_after = hash(&path);
    observed.db_rows_after = rows(&path);
    observed.config_after = std::fs::read_to_string(directory.join("config.toml")).unwrap();
    if input.bad_temp {
        std::fs::remove_file(&temp).unwrap();
        std::fs::create_dir(&temp).unwrap();
    }
    observed.remaining_temps = std::fs::read_dir(&temp)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    observed.remaining_temps.sort();
    observed
}

struct Opener {
    accounts: AccountService,
    observation: Shared,
    clients: Arc<Mutex<Vec<Arc<Client>>>>,
}
impl AccountOpener for Opener {
    fn open_client_with_proxy(
        &self,
        context: &Context,
        proxy_override: Option<&str>,
    ) -> ClientOpen<Client> {
        {
            let mut observed = self.observation.lock().unwrap();
            observed
                .proxy_overrides
                .push(proxy_override.map(str::to_owned));
            observed.trace.push("account.open".into());
        }
        let opened = self
            .accounts
            .open_client_with_proxy(context, proxy_override);
        if let Some(client) = &opened.client {
            let mut clients = self.clients.lock().unwrap();
            if !clients.iter().any(|other| Arc::ptr_eq(other, client)) {
                clients.push(client.clone());
            }
            self.observation.lock().unwrap().unique_clients = clients.len();
        }
        opened
    }
}
struct Transport {
    input: Input,
    observation: Shared,
    index: Mutex<usize>,
    context: Context,
}
impl RawTransport for Transport {
    fn send(
        &self,
        request: RawRequest,
    ) -> TransportFuture<'_, Result<Option<RawResponse>, ExternalError>> {
        Box::pin(async move {
            {
                let mut observed = self.observation.lock().unwrap();
                observed.requests.push(json!({"method":request.method,"url":request.url,"headers":request.headers,"context_error":request.context.error().map(|error|error.to_string()).unwrap_or_default()}));
                observed.trace.push(format!("request:{}", request.url));
            }
            let cookie = request
                .headers
                .get("Cookie")
                .and_then(|values| values.first())
                .map(String::as_str)
                .unwrap_or_default();
            assert!(
                matches!(
                    cookie,
                    "FANBOXSESSID=owned-session-42" | "FANBOXSESSID=owned-session-7"
                ),
                "unexpected saved session"
            );
            let reply = if request.url == "https://www.fanbox.cc/" {
                let id = if cookie.ends_with("-7") { 7 } else { 42 };
                Reply {
                    body: format!(
                        "<html><head><meta name=\"metadata\" content='{{\"context\":{{\"user\":{{\"userId\":{id},\"name\":\"owned user\"}}}}}}'></head></html>"
                    ),
                    ..Default::default()
                }
            } else {
                assert_eq!(
                    url::Url::parse(&request.url).unwrap().host_str(),
                    Some("api.fanbox.cc")
                );
                let mut index = self.index.lock().unwrap();
                let reply = self
                    .input
                    .replies
                    .get(*index)
                    .cloned()
                    .ok_or_else(|| external("owned response sequence exhausted"))?;
                *index += 1;
                reply
            };
            if self.input.cancel == "transport" {
                self.context.cancel();
                return Err(external(request.context.error().unwrap().to_string()));
            }
            if !reply.transport_error.is_empty() {
                return Err(external(&reply.transport_error));
            }
            Ok(Some(RawResponse {
                status: if reply.status == 0 { 200 } else { reply.status },
                headers: [("Content-Type".into(), vec!["application/json".into()])].into(),
                content_length: -1,
                body: Some(Box::new(Body {
                    data: reply.body.as_bytes().to_vec(),
                    offset: 0,
                    failed: false,
                    reply,
                    observation: self.observation.clone(),
                })),
            }))
        })
    }
    fn close_idle_connections(&self) {
        let mut observed = self.observation.lock().unwrap();
        observed.idle_closes += 1;
        observed.trace.push("transport.idle-close".into());
    }
}
fn external(message: impl Into<String>) -> ExternalError {
    Box::new(io::Error::other(message.into()))
}
struct Body {
    data: Vec<u8>,
    offset: usize,
    failed: bool,
    reply: Reply,
    observation: Shared,
}
impl RawBody for Body {
    fn read<'a>(&'a mut self, output: &'a mut [u8]) -> BodyFuture<'a, RawRead> {
        Box::pin(async move {
            if !self.reply.read_error.is_empty() && !self.failed {
                self.failed = true;
                return RawRead {
                    count: 0,
                    eof: false,
                    error: Some(external(&self.reply.read_error)),
                };
            }
            let count = output.len().min(self.data.len() - self.offset);
            output[..count].copy_from_slice(&self.data[self.offset..self.offset + count]);
            self.offset += count;
            RawRead {
                count,
                eof: count == 0,
                error: None,
            }
        })
    }
    fn close(&mut self) -> BodyFuture<'_, Result<(), ExternalError>> {
        Box::pin(async move {
            {
                let mut observed = self.observation.lock().unwrap();
                observed.body_closes += 1;
                observed.trace.push("body.close".into());
            }
            if self.reply.close_error.is_empty() {
                Ok(())
            } else {
                Err(external(&self.reply.close_error))
            }
        })
    }
}
struct Reader {
    input: io::Cursor<Vec<u8>>,
    fail: bool,
    observation: Shared,
}
impl Read for Reader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.observation.lock().unwrap().stdin_reads += 1;
        if self.fail {
            return Err(io::Error::other("owned stdin failure"));
        }
        self.input.read(output)
    }
}
enum Destination {
    Memory(Vec<u8>),
    Pipe {
        writer: Option<File>,
        reader: Option<JoinHandle<Vec<u8>>>,
    },
}
struct Writer {
    destination: Destination,
    remaining: usize,
    mode: String,
    observation: Shared,
}
impl Writer {
    fn new(input: &Input, observation: Shared) -> Self {
        let destination = if input.output_pipe {
            #[cfg(unix)]
            {
                use std::os::fd::FromRawFd;
                let mut descriptors = [0; 2];
                assert_eq!(unsafe { libc::pipe(descriptors.as_mut_ptr()) }, 0);
                let mut reader = unsafe { File::from_raw_fd(descriptors[0]) };
                let writer = unsafe { File::from_raw_fd(descriptors[1]) };
                Destination::Pipe {
                    writer: Some(writer),
                    reader: Some(std::thread::spawn(move || {
                        let mut body = vec![];
                        reader.read_to_end(&mut body).unwrap();
                        body
                    })),
                }
            }
            #[cfg(not(unix))]
            {
                panic!("physical output pipe rows require the fixture's Unix boundary")
            }
        } else {
            Destination::Memory(vec![])
        };
        Self {
            destination,
            remaining: input.write_limit,
            mode: input.writer.clone(),
            observation,
        }
    }
    fn is_terminal(&self) -> bool {
        match &self.destination {
            Destination::Memory(_) => false,
            Destination::Pipe { writer, .. } => writer.as_ref().unwrap().is_terminal(),
        }
    }
    fn finish(mut self) -> Vec<u8> {
        match &mut self.destination {
            Destination::Memory(body) => std::mem::take(body),
            Destination::Pipe { writer, reader } => {
                drop(writer.take());
                reader.take().unwrap().join().unwrap()
            }
        }
    }
}
impl Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.observation.lock().unwrap().writes.push(bytes.len());
        if let Destination::Pipe { writer, .. } = &mut self.destination {
            return writer.as_mut().unwrap().write(bytes);
        }
        let Destination::Memory(body) = &mut self.destination else {
            unreachable!()
        };
        if self.mode.is_empty() {
            body.extend_from_slice(bytes);
            return Ok(bytes.len());
        }
        let count = bytes.len().min(self.remaining);
        self.remaining -= count;
        body.extend_from_slice(&bytes[..count]);
        if count == bytes.len() {
            return Ok(count);
        }
        match self.mode.as_str() {
            "short" => Ok(count),
            "pipe" => Err(io::Error::from_raw_os_error(32)),
            _ => Err(io::Error::other("owned writer failure")),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn seed(directory: &Path, input: &Input) {
    std::fs::create_dir_all(directory).unwrap();
    std::fs::write(directory.join("config.toml"), &input.config).unwrap();
    if input.db_failure == "corrupt" {
        std::fs::write(directory.join("pixiv-cli.db"), b"owned non-SQLite database").unwrap();
        return;
    }
    let database = Database::open(directory).unwrap();
    let connection = rusqlite::Connection::open(database.path()).unwrap();
    connection
        .execute("UPDATE schema_migration SET applied_at=10", [])
        .unwrap();
    connection.execute("INSERT INTO pixiv_account(user_id,sort_order,username,refresh_token,credential_revision,created_at,updated_at) VALUES(99,1,'pixiv canary',x'6e657665722d757365',1,11,22)",[]).unwrap();
    if input.saved != "none" {
        let session = match input.saved.as_str() {
            "empty" => "",
            "invalid" => "bad\r\nsecret-session",
            _ => "owned-session-42",
        };
        for (id, order, session) in [(42, 9, session), (7, 2, "owned-session-7")] {
            connection.execute("INSERT INTO fanbox_account(user_id,sort_order,display_name,creator_id,session_id,credential_revision,validated_at,created_at,updated_at) VALUES(?1,?2,'owned account',?3,?4,3,10,11,22)",rusqlite::params![id,order,id.to_string(),session.as_bytes()]).unwrap();
        }
    }
    if input.db_failure == "missing-table" {
        connection.execute("DROP TABLE fanbox_account", []).unwrap();
    }
}
fn hash(path: &Path) -> String {
    format!("{:x}", Sha256::digest(std::fs::read(path).unwrap()))
}
fn rows(path: &Path) -> Vec<String> {
    let connection =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let mut values = vec![];
    for query in [
        "SELECT user_id,sort_order,display_name,creator_id,hex(session_id),credential_revision,validated_at,created_at,updated_at FROM fanbox_account ORDER BY user_id",
        "SELECT user_id,sort_order,username,hex(refresh_token),credential_revision,schedulable,created_at,updated_at FROM pixiv_account ORDER BY user_id",
        "SELECT version,name,checksum,applied_at FROM schema_migration ORDER BY version",
    ] {
        let mut statement = match connection.prepare(query) {
            Ok(statement) => statement,
            Err(error) => {
                let source = std::error::Error::source(&error)
                    .unwrap()
                    .downcast_ref::<rusqlite::ffi::Error>()
                    .unwrap();
                match source.extended_code {
                    26 => assert_eq!(error.to_string(), "file is not a database"),
                    1 => assert!(
                        error
                            .to_string()
                            .starts_with("no such table: fanbox_account")
                    ),
                    code => panic!("unexpected actual query error code {code}: {error}"),
                }
                values.push(error.to_string());
                continue;
            }
        };
        let columns = statement.column_count();
        let result = statement
            .query_map([], |row| {
                let mut values = vec![];
                for index in 0..columns {
                    let value = match row.get_ref(index)? {
                        rusqlite::types::ValueRef::Null => Value::Null,
                        rusqlite::types::ValueRef::Integer(value) => json!(value),
                        rusqlite::types::ValueRef::Real(value) => json!(value),
                        rusqlite::types::ValueRef::Text(value) => {
                            json!(std::str::from_utf8(value).unwrap())
                        }
                        rusqlite::types::ValueRef::Blob(_) => {
                            panic!("persistence query must select hex blobs")
                        }
                    };
                    values.push(value);
                }
                Ok(serde_json::to_string(&values).unwrap())
            })
            .unwrap();
        values.extend(result.map(Result::unwrap));
    }
    values
}
