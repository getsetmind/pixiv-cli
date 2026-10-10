use super::{Shared, schema::Input, trace};
use serde_json::json;
use std::io::{self, Read, Write};

pub struct Reader {
    input: io::Cursor<Vec<u8>>,
    fail: bool,
    observed: Shared,
}
impl Reader {
    pub fn new(input: &Input, observed: Shared) -> Self {
        Self {
            input: io::Cursor::new(input.stdin.as_bytes().to_vec()),
            fail: input.stdin_error,
            observed,
        }
    }
}
impl Read for Reader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.observed.lock().unwrap().stdin_reads += 1;
        if self.fail {
            Err(io::Error::other("owned stdin failure"))
        } else {
            self.input.read(output)
        }
    }
}
pub struct Writer {
    pub bytes: Vec<u8>,
    mode: String,
    remaining: usize,
    observed: Shared,
}
impl Writer {
    pub fn new(input: &Input, observed: Shared) -> Self {
        Self {
            bytes: vec![],
            mode: input.writer.clone(),
            remaining: input.write_limit,
            observed,
        }
    }
}
impl Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let count = if self.mode.is_empty() {
            bytes.len()
        } else {
            bytes.len().min(self.remaining)
        };
        if !self.mode.is_empty() {
            self.remaining -= count;
        }
        self.bytes.extend_from_slice(&bytes[..count]);
        let error = if count < bytes.len() {
            match self.mode.as_str() {
                "short" => None,
                "pipe" => Some(io::Error::from_raw_os_error(32)),
                _ => Some(io::Error::other("owned writer failure")),
            }
        } else {
            None
        };
        {
            let mut observed = self.observed.lock().unwrap();
            observed.writes.push(bytes.len());
            observed.output_writes.push(json!({"bytes":std::str::from_utf8(bytes).unwrap(),"n":count,"error":error.as_ref().map(ToString::to_string).unwrap_or_default()}));
        }
        trace(&self.observed, "stdout.write");
        match error {
            Some(error) => Err(error),
            None => Ok(count),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
