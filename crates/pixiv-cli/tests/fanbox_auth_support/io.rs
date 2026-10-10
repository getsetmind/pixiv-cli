use super::{Shared, schema::WriteAttempt, trace};
use std::io::{self, Cursor, Read, Write};

pub struct Reader {
    bytes: Cursor<Vec<u8>>,
    failure: bool,
    observed: Shared,
}
impl Reader {
    pub fn new(input_hex: &str, failure: bool, observed: Shared) -> Self {
        Self {
            bytes: Cursor::new(decode_hex(input_hex)),
            failure,
            observed,
        }
    }
}
impl Read for Reader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        {
            let mut observed = self.observed.lock().unwrap();
            observed.stdin_reads += 1;
            observed.trace.push("stdin.read".into());
        }
        if self.failure {
            return Err(io::Error::other("owned fixture stdin failure"));
        }
        let count = self.bytes.read(output)?;
        self.observed.lock().unwrap().stdin_bytes += count;
        Ok(count)
    }
}

pub struct Writer {
    mode: String,
    name: &'static str,
    observed: Shared,
    bytes: Vec<u8>,
    attempts: Vec<WriteAttempt>,
}
impl Writer {
    pub fn new(mode: &str, name: &'static str, observed: Shared) -> Self {
        Self {
            mode: mode.into(),
            name,
            observed,
            bytes: Vec::new(),
            attempts: Vec::new(),
        }
    }
    pub fn record(self) {
        let mut observed = self.observed.lock().unwrap();
        let bytes = String::from_utf8(self.bytes).expect("CLI output must be UTF-8");
        if self.name == "stdout" {
            observed.stdout = bytes;
            observed.output_writes = self.attempts;
        } else {
            observed.stderr = bytes;
            observed.error_writes = self.attempts;
        }
    }
}
impl Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let (count, error) = match self.mode.as_str() {
            "short" => (0, None),
            "error" => (0, Some(io::Error::other("owned fixture writer failure"))),
            "epipe" => (0, Some(io::Error::from_raw_os_error(32))),
            "partial-error" => (
                bytes.len() / 2,
                Some(io::Error::other("owned fixture writer failure")),
            ),
            _ => (bytes.len(), None),
        };
        self.bytes.extend_from_slice(&bytes[..count]);
        self.attempts.push(WriteAttempt {
            bytes: String::from_utf8(bytes.into()).expect("CLI write must be UTF-8"),
            n: count,
            error: error.as_ref().map_or(String::new(), |error| {
                if error.kind() == io::ErrorKind::BrokenPipe {
                    "broken pipe".into()
                } else {
                    error.to_string()
                }
            }),
        });
        trace(&self.observed, format!("{}.write", self.name));
        match error {
            Some(error) => Err(error),
            None => Ok(count),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub fn decode_hex(value: &str) -> Vec<u8> {
    assert_eq!(value.len() % 2, 0);
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).unwrap())
        .collect()
}
pub fn encode_hex(value: &[u8]) -> String {
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}
