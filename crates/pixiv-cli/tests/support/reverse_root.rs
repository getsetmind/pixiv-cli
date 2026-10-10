use clap::Parser;
use pixiv_app::{
    callback_handler::CallbackResult,
    config::Store,
    database::{Database, PixivAccount},
    lifecycle::Context,
};
use pixiv_cli_rs::search::{SearchInput, SearchOptions};
use pixiv_sdk::transport::{Request, Response, Transport};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub const PAYLOAD: &[u8] = b"owned synthetic reverse-search payload\n";
pub const REMAINDER: [usize; 22] = [
    16, 17, 18, 19, 20, 21, 44, 72, 73, 74, 93, 94, 96, 129, 130, 132, 134, 136, 138, 140, 142, 144,
];

#[derive(Deserialize)]
pub struct Fixture {
    pub reference: String,
    pub cases: Vec<Row>,
}
#[derive(Deserialize)]
pub struct Row {
    pub name: String,
    pub input: Value,
    pub observation: Value,
}
pub fn fixture() -> Fixture {
    let value: Fixture =
        serde_json::from_str(include_str!("../fixtures/reverse-search-cli.json")).unwrap();
    assert_eq!(value.reference, "4b4426487ef18bed276706daec385e0d0a6979f9");
    assert_eq!(value.cases.len(), 148);
    value
}
pub fn text<'a>(value: &'a Value, field: &str) -> &'a str {
    value[field].as_str().unwrap_or("")
}

#[derive(Parser)]
#[command(args_override_self = true)]
pub struct Arguments {
    #[command(flatten)]
    pub input: SearchInput,
    #[command(flatten)]
    pub options: SearchOptions,
}
impl Arguments {
    pub fn parse(row: &Row) -> Self {
        Self::try_parse_from(
            std::iter::once("pixiv".to_owned()).chain(
                row.input["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .skip(1)
                    .map(|arg| arg.as_str().unwrap().to_owned()),
            ),
        )
        .unwrap()
    }
}

#[derive(Default)]
pub struct Writer {
    pub bytes: Vec<u8>,
    pub writes: Vec<usize>,
}
impl Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.writes.push(bytes.len());
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[derive(Clone, Default)]
pub struct SharedWriter(pub Arc<Mutex<Writer>>);
impl Write for SharedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.lock().unwrap().flush()
    }
}

pub struct Reader<'a> {
    pub remaining: &'a [u8],
    pub failure: bool,
    pub reads: usize,
    pub bytes: usize,
}
impl Read for Reader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.reads += 1;
        if self.failure {
            return Err(io::Error::other("owned stdin failure"));
        }
        let count = self.remaining.read(buffer)?;
        self.bytes += count;
        Ok(count)
    }
}

#[derive(Default)]
pub struct Startup {
    pub failure: bool,
    pub calls: Mutex<Vec<&'static str>>,
}
impl pixiv_cli_rs::startup::StartupHooks for Startup {
    fn cleanup_pending_update(&self) -> CallbackResult<()> {
        self.calls.lock().unwrap().push("cleanup");
        if self.failure {
            return Err(Box::new(io::Error::other("owned startup failure")));
        }
        Ok(())
    }
    fn automatic_supported(&self) -> bool {
        self.calls.lock().unwrap().push("handler-check");
        false
    }
    fn ensure_if_needed(&self, _: &Context) -> CallbackResult<()> {
        panic!("disabled automatic support must not alter URL associations")
    }
}

pub struct Owned {
    pub home: tempfile::TempDir,
    pub directory: PathBuf,
    pub store: Store,
}
impl Owned {
    pub fn new(row: &Row) -> Self {
        let home = tempfile::tempdir().unwrap();
        let directory = home.path().join(".pixiv-cli");
        let path = directory.join("config.toml");
        if let Some(body) = row.input["config_before"].as_str() {
            std::fs::create_dir(&directory).unwrap();
            std::fs::write(&path, body).unwrap();
        }
        for filename in ["image.png", "owned image.png", "ftp:owned.png"] {
            std::fs::write(home.path().join(filename), PAYLOAD).unwrap();
        }
        for name in ["directory", "snapshots"] {
            std::fs::create_dir(home.path().join(name)).unwrap();
        }
        Self {
            home,
            directory,
            store: Store::new(path),
        }
    }
    pub fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(self.home.path())
            .env_clear()
            .env("HOME", self.home.path())
            .env("USERPROFILE", self.home.path())
            .env("PIXIV_REQUEST_INTERVAL", "0")
            .env("TZ", "UTC")
            .env("XDG_CONFIG_HOME", self.home.path().join("xdg-config"))
            .env("XDG_CACHE_HOME", self.home.path().join("xdg-cache"))
            .env("TMPDIR", self.home.path().join("snapshots"));
        command
    }
    pub fn assert_preserved(&self, row: &Row) {
        let after = std::fs::read(self.store.path());
        match row.observation["config_after"].as_str() {
            Some(expected) => {
                assert_eq!(after.unwrap(), expected.as_bytes(), "{} config", row.name)
            }
            None => assert_eq!(
                after.unwrap_err().kind(),
                io::ErrorKind::NotFound,
                "{} config",
                row.name
            ),
        }
        assert_eq!(
            self.directory.join("pixiv-cli.db").exists(),
            row.observation["database"].as_bool().unwrap(),
            "{} database",
            row.name,
        );
        for filename in ["image.png", "owned image.png", "ftp:owned.png"] {
            assert_eq!(
                std::fs::read(self.home.path().join(filename)).unwrap(),
                PAYLOAD,
                "{} source",
                row.name
            );
        }
        assert_eq!(
            std::fs::read_dir(self.home.path().join("snapshots"))
                .unwrap()
                .count(),
            0,
            "{} snapshots",
            row.name
        );
        assert!(!self.home.path().join(".pixiv-cli-rs").exists());
    }
    pub fn seeded_database(&self, row: &Row) -> Arc<Mutex<Database>> {
        let mut database = Database::open(&self.directory).unwrap();
        for id in row.input["saved_accounts"].as_array().unwrap() {
            let id = id.as_i64().unwrap();
            database
                .save_pixiv_credential(&PixivAccount::new(
                    id,
                    "owned account",
                    format!("synthetic-refresh-secret-{id}").as_bytes(),
                ))
                .unwrap();
        }
        database.set_all_pixiv_schedulable(true).unwrap();
        Arc::new(Mutex::new(database))
    }
}

pub fn wait_owned(mut child: Child) -> Output {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!("owned fixture process exceeded its deadline: {output:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
pub fn run_binary(owned: &Owned, row: &Row) -> Output {
    let mut command = owned.command(env!("CARGO_BIN_EXE_pixiv"));
    command.args(
        row.input["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|arg| arg.as_str().unwrap()),
    );
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if let Some(mut input) = child.stdin.take() {
        let body = text(&row.input, "stdin").as_bytes();
        if !body.is_empty() {
            match input.write_all(body) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {}
                Err(error) => panic!("{} stdin: {error}", row.name),
            }
        }
    }
    wait_owned(child)
}
pub fn assert_result(row: &Row, output: &Writer, errors: &Writer, exit: i32) {
    assert_eq!(
        exit,
        row.observation["exit"].as_i64().unwrap() as i32,
        "{} exit",
        row.name
    );
    assert_eq!(
        output.bytes,
        text(&row.observation, "stdout").as_bytes(),
        "{} stdout",
        row.name
    );
    assert_eq!(
        errors.bytes,
        text(&row.observation, "stderr").as_bytes(),
        "{} stderr",
        row.name
    );
    assert_eq!(
        json!(output.writes),
        row.observation["output_writes"],
        "{} output writes",
        row.name
    );
    assert_eq!(
        json!(errors.writes),
        row.observation["error_writes"],
        "{} diagnostic writes",
        row.name
    );
}
pub fn accounts(database: &Database) -> Value {
    json!(
        database
            .list_pixiv()
            .unwrap()
            .into_iter()
            .map(|account| json!({
                "id": account.user_id,
                "revision": account.credential_revision,
                "refresh": String::from_utf8(account.refresh_token_copy()).unwrap(),
                "selected": account.pool_last_selected,
                "schedulable": account.schedulable,
            }))
            .collect::<Vec<_>>()
    )
}

#[derive(Default)]
pub struct SdkObserved {
    pub constructors: Vec<(String, Duration)>,
    pub requests: Vec<Request>,
    pub drops: usize,
}
pub struct Sdk {
    pub body: Value,
    pub database: Arc<Mutex<Database>>,
    pub observed: Arc<Mutex<SdkObserved>>,
}
impl Drop for Sdk {
    fn drop(&mut self) {
        self.observed.lock().unwrap().drops += 1;
    }
}
impl Transport for Sdk {
    async fn send(&self, request: Request) -> pixiv_sdk::Result<Response> {
        let oauth = request.operation == "Open";
        let body = if oauth {
            assert_eq!(request.method.as_str(), "POST");
            assert_eq!(request.url, "https://oauth.secure.pixiv.net/auth/token");
            let refresh = request
                .parameters
                .iter()
                .find(|(key, _)| key == "refresh_token")
                .unwrap()
                .1
                .clone();
            let id = refresh.rsplit('-').next().unwrap().parse::<i64>().unwrap();
            let account = self.database.lock().unwrap().get_pixiv(id).unwrap();
            assert_eq!(account.credential_revision, 1);
            assert_eq!(account.refresh_token_copy(), refresh.as_bytes());
            json!({"access_token":format!("synthetic-access-secret-{id}"),"refresh_token":format!("synthetic-rotated-secret-{id}"),"expires_in":3600,"user":{"id":id,"name":"owned account"}})
        } else {
            assert_eq!(request.method.as_str(), "GET");
            assert_eq!(request.url, "https://app-api.pixiv.net/v1/search/illust");
            let auth = request
                .headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case("authorization"))
                .unwrap()
                .1
                .clone();
            let id = auth.rsplit('-').next().unwrap().parse::<i64>().unwrap();
            let account = self.database.lock().unwrap().get_pixiv(id).unwrap();
            assert_eq!(account.credential_revision, 2);
            assert_eq!(
                account.refresh_token_copy(),
                format!("synthetic-rotated-secret-{id}").as_bytes()
            );
            self.body.clone()
        };
        self.observed.lock().unwrap().requests.push(request);
        Ok(Response {
            status: 200,
            retry_after: None,
            body,
        })
    }
}

pub fn multimap(
    pairs: impl IntoIterator<Item = (String, String)>,
) -> BTreeMap<String, Vec<String>> {
    let mut result = BTreeMap::<String, Vec<String>>::new();
    for (key, value) in pairs {
        result.entry(key).or_default().push(value);
    }
    result
}
pub fn assert_sdk_requests(actual: &[Request], expected: &Value, name: &str) {
    let expected = expected.as_array().unwrap();
    assert_eq!(actual.len(), expected.len(), "{name} SDK request count");
    for (actual, expected) in actual.iter().zip(expected) {
        assert_eq!(
            actual.method.as_str(),
            text(expected, "method"),
            "{name} method"
        );
        let url = url::Url::parse(text(expected, "url")).unwrap();
        let mut endpoint = url.clone();
        endpoint.set_query(None);
        assert_eq!(actual.url, endpoint.as_str(), "{name} destination");
        let expected_parameters = if actual.method.as_str() == "POST" {
            serde_json::from_value(expected["form"].clone()).unwrap()
        } else {
            multimap(
                url.query_pairs()
                    .map(|(key, value)| (key.into_owned(), value.into_owned())),
            )
        };
        assert_eq!(
            multimap(actual.parameters.clone()),
            expected_parameters,
            "{name} parameters"
        );
        let expected_headers: BTreeMap<String, Vec<String>> =
            serde_json::from_value(expected["headers"].clone()).unwrap();
        let expected_headers = expected_headers
            .into_iter()
            .map(|(key, values)| (key.to_ascii_lowercase(), values))
            .collect::<BTreeMap<_, _>>();
        let actual_headers = multimap(
            actual
                .headers
                .iter()
                .map(|(key, value)| (key.to_ascii_lowercase(), value.clone())),
        );
        assert_eq!(actual_headers, expected_headers, "{name} headers");
    }
}
pub fn assert_empty_accounts(directory: &Path, row: &Row) {
    let actual = if directory.join("pixiv-cli.db").exists() {
        accounts(&Database::open(directory).unwrap())
    } else {
        json!([])
    };
    assert_eq!(
        actual, row.observation["accounts_after"],
        "{} account preservation",
        row.name
    );
}
