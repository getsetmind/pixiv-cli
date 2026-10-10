use super::transport::{BodyFuture, ExternalError, RawBody, RawRead};
use brotli::{BrotliDecompressStream, BrotliResult, BrotliState, HeapAlloc, HuffmanCode};
use brotli_decompressor::{BrotliDecoderHasMoreOutput, BrotliDecoderIsFinished};
use flate2::{Decompress, FlushDecompress, Status};
use std::{error::Error, fmt, io, sync::Arc};
use zstd::stream::raw::{DParameter, Decoder, InBuffer, Operation, OutBuffer};

#[derive(Clone, Debug)]
struct BodyError(Arc<dyn Error + Send + Sync>);
impl BodyError {
    fn message(message: impl Into<String>) -> Self {
        Self(Arc::new(io::Error::other(message.into())))
    }
    fn external(error: ExternalError) -> Self {
        Self(Arc::from(error))
    }
    fn boxed(&self) -> ExternalError {
        Box::new(self.clone())
    }
}
impl fmt::Display for BodyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, formatter)
    }
}
impl Error for BodyError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}
fn result(count: usize, eof: bool, error: Option<BodyError>) -> RawRead {
    RawRead {
        count,
        eof,
        error: error.map(|error| error.boxed()),
    }
}
fn unexpected() -> BodyError {
    BodyError::message("unexpected EOF")
}
fn validate_read(read: RawRead, length: usize) -> RawRead {
    if read.count > length {
        result(
            0,
            false,
            Some(BodyError::message("invalid response body read count")),
        )
    } else {
        read
    }
}

pub(super) async fn decode_body(mut body: Box<dyn RawBody>, encoding: &str) -> Box<dyn RawBody> {
    match encoding {
        "gzip" => Box::new(GzipBody::new(body)),
        "br" => Box::new(BrotliBody::new(body)),
        "zstd" => Box::new(ZstdBody::new(body)),
        "deflate" => {
            let mut prefix = [0; 2];
            let mut filled = 0;
            while filled < prefix.len() {
                let read = validate_read(
                    body.read(&mut prefix[filled..]).await,
                    prefix.len() - filled,
                );
                filled += read.count;
                if filled == prefix.len() {
                    break;
                }
                if read.error.is_some() || read.eof {
                    return body;
                }
                if read.count == 0 {
                    continue;
                }
            }
            if prefix[0] != 0x78 {
                return body;
            }
            let zlib = matches!(prefix[1], 0x9c | 0x01 | 0x5e | 0xda);
            let mut bytes = prefix.to_vec();
            loop {
                let mut output = [0; 32768];
                let read = validate_read(body.read(&mut output).await, output.len());
                bytes.extend_from_slice(&output[..read.count]);
                if read.error.is_some() || read.eof {
                    break;
                }
            }
            let _ = body.close().await;
            Box::new(DeflateBody {
                bytes,
                position: 0,
                zlib,
                decoder: None,
                terminal: None,
                finished: false,
            })
        }
        _ => body,
    }
}

struct Input {
    source: Box<dyn RawBody>,
    bytes: Vec<u8>,
    position: usize,
    pending: Option<BodyError>,
    eof: bool,
}
impl Input {
    fn new(source: Box<dyn RawBody>) -> Self {
        Self {
            source,
            bytes: Vec::new(),
            position: 0,
            pending: None,
            eof: false,
        }
    }
    async fn refill(&mut self, size: usize) -> Result<bool, BodyError> {
        if self.position < self.bytes.len() {
            return Ok(true);
        }
        if let Some(error) = &self.pending {
            return Err(error.clone());
        }
        if self.eof {
            return Ok(false);
        }
        let mut output = vec![0; size];
        let read = validate_read(self.source.read(&mut output).await, output.len());
        output.truncate(read.count);
        self.bytes = output;
        self.position = 0;
        self.pending = read.error.map(BodyError::external);
        self.eof = read.eof;
        if read.count > 0 {
            return Ok(true);
        }
        if let Some(error) = &self.pending {
            return Err(error.clone());
        }
        Ok(!read.eof)
    }
    async fn byte(&mut self) -> Result<Option<u8>, BodyError> {
        loop {
            if !self.refill(4096).await? {
                return Ok(None);
            }
            if self.position < self.bytes.len() {
                let byte = self.bytes[self.position];
                self.position += 1;
                return Ok(Some(byte));
            }
        }
    }
}

struct GzipBody {
    input: Input,
    header: Vec<u8>,
    footer: Vec<u8>,
    decoder: Option<Decompress>,
    crc: crc32fast::Hasher,
    size: u32,
    reading_footer: bool,
    pending_output: Vec<u8>,
    terminal: Option<BodyError>,
    eof: bool,
}
impl GzipBody {
    fn new(source: Box<dyn RawBody>) -> Self {
        Self {
            input: Input::new(source),
            header: Vec::new(),
            footer: Vec::new(),
            decoder: None,
            crc: crc32fast::Hasher::new(),
            size: 0,
            reading_footer: false,
            pending_output: Vec::new(),
            terminal: None,
            eof: false,
        }
    }
    async fn header(&mut self) -> Result<bool, BodyError> {
        loop {
            if let Some(length) = gzip_header_length(&self.header)? {
                if self.header[3] & 2 != 0 {
                    let stored =
                        u16::from_le_bytes(self.header[length - 2..length].try_into().unwrap());
                    if crc32fast::hash(&self.header[..length - 2]) as u16 != stored {
                        return Err(BodyError::message("gzip: invalid header"));
                    }
                }
                self.header.clear();
                self.decoder = Some(Decompress::new(false));
                self.crc = crc32fast::Hasher::new();
                self.size = 0;
                return Ok(true);
            }
            match self.input.byte().await? {
                Some(byte) => self.header.push(byte),
                None if self.header.is_empty() => return Ok(false),
                None => return Err(unexpected()),
            }
        }
    }
    async fn pump(&mut self, output: &mut [u8]) -> RawRead {
        if let Some(error) = &self.terminal {
            return self.finish(0, false, Some(error.clone()));
        }
        if self.eof {
            return self.finish(0, true, None);
        }
        let mut written = self.pending_output.len().min(output.len());
        output[..written].copy_from_slice(&self.pending_output[..written]);
        if written > 0 && written == output.len() {
            return self.finish(written, false, None);
        }
        loop {
            if self.decoder.is_none() {
                self.save_output(&output[..written]);
                match self.header().await {
                    Ok(true) => {}
                    Ok(false) => {
                        self.eof = true;
                        return self.finish(written, true, None);
                    }
                    Err(error) => {
                        self.terminal = Some(error.clone());
                        return self.finish(written, false, Some(error));
                    }
                }
            }
            if written == output.len() {
                return self.finish(written, false, None);
            }
            if self.reading_footer {
                while self.footer.len() < 8 {
                    self.save_output(&output[..written]);
                    match self.input.byte().await {
                        Ok(Some(byte)) => self.footer.push(byte),
                        value => {
                            let error = value.err().unwrap_or_else(unexpected);
                            self.terminal = Some(error.clone());
                            return self.finish(written, false, Some(error));
                        }
                    }
                }
                let crc = u32::from_le_bytes(self.footer[..4].try_into().unwrap());
                let size = u32::from_le_bytes(self.footer[4..].try_into().unwrap());
                if crc != self.crc.clone().finalize() || size != self.size {
                    let error = BodyError::message("gzip: invalid checksum");
                    self.terminal = Some(error.clone());
                    return self.finish(written, false, Some(error));
                }
                self.footer.clear();
                self.reading_footer = false;
                self.decoder = None;
                if written > 0 {
                    self.save_output(&output[..written]);
                    return match self.header().await {
                        Ok(true) => self.finish(written, false, None),
                        Ok(false) => {
                            self.eof = true;
                            self.finish(written, true, None)
                        }
                        Err(error) => {
                            self.terminal = Some(error.clone());
                            self.finish(written, false, Some(error))
                        }
                    };
                }
                continue;
            }
            self.save_output(&output[..written]);
            let available = match self.input.refill(4096).await {
                Ok(available) => available,
                Err(error) => {
                    self.terminal = Some(error.clone());
                    return self.finish(written, false, Some(error));
                }
            };
            let decoder = self.decoder.as_mut().unwrap();
            let before_in = decoder.total_in();
            let before_out = decoder.total_out();
            let decoded = decoder.decompress(
                &self.input.bytes[self.input.position..],
                &mut output[written..],
                FlushDecompress::None,
            );
            let consumed = (decoder.total_in() - before_in) as usize;
            let count = (decoder.total_out() - before_out) as usize;
            self.input.position += consumed;
            self.crc.update(&output[written..written + count]);
            self.size = self.size.wrapping_add(count as u32);
            written += count;
            match decoded {
                Ok(Status::StreamEnd) => {
                    self.reading_footer = true;
                }
                Err(_) => {
                    let error = BodyError::message(format!(
                        "flate: corrupt input before offset {}",
                        decoder.total_in()
                    ));
                    self.terminal = Some(error.clone());
                    return self.finish(written, false, Some(error));
                }
                _ if count == 0 && consumed == 0 && !available => {
                    let error = unexpected();
                    self.terminal = Some(error.clone());
                    return self.finish(written, false, Some(error));
                }
                _ => {}
            }
        }
    }
    fn save_output(&mut self, output: &[u8]) {
        if !output.is_empty() {
            self.pending_output.clear();
            self.pending_output.extend_from_slice(output);
        }
    }
    fn finish(&mut self, count: usize, eof: bool, error: Option<BodyError>) -> RawRead {
        self.pending_output
            .drain(..count.min(self.pending_output.len()));
        result(count, eof, error)
    }
}
impl RawBody for GzipBody {
    fn read<'a>(&'a mut self, output: &'a mut [u8]) -> BodyFuture<'a, RawRead> {
        Box::pin(async move { self.pump(output).await })
    }
    fn close(&mut self) -> BodyFuture<'_, Result<(), ExternalError>> {
        self.input.source.close()
    }
}
fn gzip_header_length(bytes: &[u8]) -> Result<Option<usize>, BodyError> {
    if bytes.len() < 10 {
        return Ok(None);
    }
    if bytes[..3] != [0x1f, 0x8b, 8] {
        return Err(BodyError::message("gzip: invalid header"));
    }
    let flags = bytes[3];
    let mut length = 10;
    if flags & 4 != 0 {
        if bytes.len() < length + 2 {
            return Ok(None);
        }
        let extra = u16::from_le_bytes(bytes[length..length + 2].try_into().unwrap()) as usize;
        length += 2 + extra;
        if bytes.len() < length {
            return Ok(None);
        }
    }
    for flag in [8, 16] {
        if flags & flag != 0 {
            let Some(end) = bytes[length..].iter().position(|byte| *byte == 0) else {
                if bytes.len() - length >= 512 {
                    return Err(BodyError::message("gzip: invalid header"));
                }
                return Ok(None);
            };
            if end >= 512 {
                return Err(BodyError::message("gzip: invalid header"));
            }
            length += end + 1;
        }
    }
    if flags & 2 != 0 {
        length += 2;
    }
    Ok((bytes.len() >= length).then_some(length))
}

struct DeflateBody {
    bytes: Vec<u8>,
    position: usize,
    zlib: bool,
    decoder: Option<Decompress>,
    terminal: Option<BodyError>,
    finished: bool,
}
impl RawBody for DeflateBody {
    fn read<'a>(&'a mut self, output: &'a mut [u8]) -> BodyFuture<'a, RawRead> {
        Box::pin(async move {
            if let Some(error) = &self.terminal {
                return result(0, false, Some(error.clone()));
            }
            if self.finished {
                return result(0, true, None);
            }
            let decoder = self
                .decoder
                .get_or_insert_with(|| Decompress::new(self.zlib));
            let before_in = decoder.total_in();
            let before_out = decoder.total_out();
            let decoded =
                decoder.decompress(&self.bytes[self.position..], output, FlushDecompress::None);
            self.position += (decoder.total_in() - before_in) as usize;
            let count = (decoder.total_out() - before_out) as usize;
            let error = match decoded {
                Ok(Status::StreamEnd) => {
                    self.finished = true;
                    None
                }
                Err(_) => Some(BodyError::message(format!(
                    "flate: corrupt input before offset {}",
                    decoder.total_in()
                ))),
                _ if self.position == self.bytes.len() && count < output.len() => {
                    Some(unexpected())
                }
                _ => None,
            };
            self.terminal = error.clone();
            result(count, self.finished, error)
        })
    }
    fn close(&mut self) -> BodyFuture<'_, Result<(), ExternalError>> {
        Box::pin(async move {
            if self.decoder.is_none() {
                // Frozen Go panics here; Rust keeps this unsupported state fallible.
                return Err(BodyError::message("deflate decoder is not initialized").boxed());
            }
            self.terminal
                .as_ref()
                .map_or(Ok(()), |error| Err(error.boxed()))
        })
    }
}

type BrotliDecoder = BrotliState<HeapAlloc<u8>, HeapAlloc<u32>, HeapAlloc<HuffmanCode>>;
struct BrotliBody {
    source: Box<dyn RawBody>,
    input: Vec<u8>,
    position: usize,
    decoder: Option<Box<BrotliDecoder>>,
    total_out: usize,
}
impl BrotliBody {
    fn new(source: Box<dyn RawBody>) -> Self {
        Self {
            source,
            input: Vec::new(),
            position: 0,
            decoder: None,
            total_out: 0,
        }
    }
    async fn refill(&mut self) -> RawRead {
        let mut input = vec![0; 32768];
        let read = validate_read(self.source.read(&mut input).await, input.len());
        input.truncate(read.count);
        self.input = input;
        self.position = 0;
        read
    }
    async fn pump(&mut self, output: &mut [u8]) -> RawRead {
        let decoder = self.decoder.get_or_insert_with(|| {
            Box::new(BrotliState::new_strict(
                HeapAlloc::default(),
                HeapAlloc::default(),
                HeapAlloc::default(),
            ))
        });
        if !BrotliDecoderHasMoreOutput(decoder) && self.position == self.input.len() {
            let read = self.refill().await;
            if read.count == 0 {
                if read.eof && !BrotliDecoderIsFinished(self.decoder.as_ref().unwrap()) {
                    return result(0, false, Some(unexpected()));
                }
                return read;
            }
        }
        if output.is_empty() {
            return result(0, false, None);
        }
        loop {
            let mut available_in = self.input.len() - self.position;
            let mut available_out = output.len();
            let mut output_offset = 0;
            let decoded = BrotliDecompressStream(
                &mut available_in,
                &mut self.position,
                &self.input,
                &mut available_out,
                &mut output_offset,
                output,
                &mut self.total_out,
                self.decoder.as_mut().unwrap(),
            );
            match decoded {
                BrotliResult::ResultSuccess => {
                    let error =
                        (available_in != 0).then(|| BodyError::message("brotli: excessive input"));
                    return result(output_offset, false, error);
                }
                BrotliResult::ResultFailure => {
                    let name = format!("{:?}", self.decoder.as_ref().unwrap().error_code);
                    let name = name
                        .strip_prefix("BROTLI_DECODER_ERROR_FORMAT_")
                        .or_else(|| name.strip_prefix("BROTLI_DECODER_ERROR_"))
                        .unwrap_or(&name);
                    return result(
                        output_offset,
                        false,
                        Some(BodyError::message(format!("brotli: {name}"))),
                    );
                }
                BrotliResult::NeedsMoreOutput => {
                    let error = (output_offset == 0).then(|| BodyError::message("short buffer"));
                    return result(output_offset, false, error);
                }
                BrotliResult::NeedsMoreInput => {}
            }
            if available_in != 0 {
                return result(0, false, Some(BodyError::message("brotli: invalid state")));
            }
            if output_offset > 0 {
                return result(output_offset, false, None);
            }
            let read = self.refill().await;
            if read.count == 0 {
                return if read.eof {
                    result(0, false, Some(unexpected()))
                } else {
                    read
                };
            }
        }
    }
}
impl RawBody for BrotliBody {
    fn read<'a>(&'a mut self, output: &'a mut [u8]) -> BodyFuture<'a, RawRead> {
        Box::pin(async move { self.pump(output).await })
    }
    fn close(&mut self) -> BodyFuture<'_, Result<(), ExternalError>> {
        self.source.close()
    }
}

#[derive(Clone, Copy)]
enum ZstdStage {
    SignatureFirst,
    SignatureRest,
    Descriptor,
    Window,
    Dictionary,
    ContentSize,
    SkipSize,
    SkipData,
    BlockHeader,
    Payload,
    Checksum,
}
struct ZstdBody {
    source: Box<dyn RawBody>,
    decoder: Option<Decoder<'static>>,
    stage: ZstdStage,
    segment: Vec<u8>,
    frame_header: Vec<u8>,
    block_header: Vec<u8>,
    descriptor: u8,
    window: u64,
    content_size: Option<u64>,
    frame_size: u64,
    payload_size: usize,
    last: bool,
    output: Vec<u8>,
    position: usize,
    pending_output: Vec<u8>,
    block_ready: bool,
    terminal: Option<BodyError>,
    eof: bool,
    closed: bool,
}
impl ZstdBody {
    fn new(source: Box<dyn RawBody>) -> Self {
        Self {
            source,
            decoder: None,
            stage: ZstdStage::SignatureFirst,
            segment: Vec::new(),
            frame_header: Vec::new(),
            block_header: Vec::new(),
            descriptor: 0,
            window: 0,
            content_size: None,
            frame_size: 0,
            payload_size: 0,
            last: false,
            output: Vec::new(),
            position: 0,
            pending_output: Vec::new(),
            block_ready: false,
            terminal: None,
            eof: false,
            closed: false,
        }
    }
    async fn segment(&mut self, size: usize) -> Result<bool, BodyError> {
        while self.segment.len() < size {
            let mut bytes = vec![0; size - self.segment.len()];
            let read = validate_read(self.source.read(&mut bytes).await, bytes.len());
            self.segment.extend_from_slice(&bytes[..read.count]);
            if self.segment.len() == size {
                return Ok(true);
            }
            if let Some(error) = read.error {
                return Err(BodyError::external(error));
            }
            if read.eof {
                return if self.segment.is_empty() {
                    Ok(false)
                } else {
                    Err(unexpected())
                };
            }
        }
        Ok(true)
    }
    fn feed(&mut self, bytes: &[u8]) -> Result<(), BodyError> {
        let decoder = self.decoder.as_mut().unwrap();
        let mut input = InBuffer::around(bytes);
        loop {
            let before = input.pos();
            let mut bytes = [0; 131072];
            let mut output = OutBuffer::around(&mut bytes[..]);
            let decoded = decoder.run(&mut input, &mut output);
            let count = output.pos();
            self.output.extend_from_slice(&bytes[..count]);
            if let Err(error) = decoded {
                return Err(BodyError::message(error.to_string()));
            }
            if input.pos() == input.src.len() && count < bytes.len() {
                return Ok(());
            }
            if input.pos() == before && count == 0 {
                return Err(BodyError::message("zstd decoder made no progress"));
            }
        }
    }
    fn prepare_frame(&mut self) -> Result<(), BodyError> {
        if self.window > (1 << 29) {
            return Err(BodyError::message("window size exceeded"));
        }
        if self.descriptor & 0x20 != 0 {
            self.window = self.content_size.unwrap_or(0).max(1024);
            if self.window > (1 << 36) {
                return Err(BodyError::message(
                    "decompressed size exceeds configured limit",
                ));
            }
        }
        if self.window > (1 << 36) || self.window > (1 << 29) {
            return Err(BodyError::message(
                "decompressed size exceeds configured limit",
            ));
        }
        if self.window < 1024 {
            return Err(BodyError::message("window size too small"));
        }
        if self.decoder.is_none() {
            let mut decoder =
                Decoder::new().map_err(|error| BodyError::message(error.to_string()))?;
            decoder
                .set_parameter(DParameter::WindowLogMax(29))
                .map_err(|error| BodyError::message(error.to_string()))?;
            self.decoder = Some(decoder);
        }
        let header = std::mem::take(&mut self.frame_header);
        self.feed(&header)?;
        self.stage = ZstdStage::BlockHeader;
        Ok(())
    }
    async fn next_block(&mut self) -> Result<bool, BodyError> {
        loop {
            let size = match self.stage {
                ZstdStage::SignatureFirst | ZstdStage::Descriptor | ZstdStage::Window => 1,
                ZstdStage::SignatureRest | ZstdStage::BlockHeader => 3,
                ZstdStage::Dictionary => match self.descriptor & 3 {
                    3 => 4,
                    value => value as usize,
                },
                ZstdStage::ContentSize => match self.descriptor >> 6 {
                    0 => usize::from(self.descriptor & 0x20 != 0),
                    value => 1 << value,
                },
                ZstdStage::SkipSize | ZstdStage::Checksum => 4,
                ZstdStage::SkipData => self.payload_size.min(32768),
                ZstdStage::Payload => self.payload_size,
            };
            if !self.segment(size).await? {
                return match self.stage {
                    ZstdStage::SignatureFirst | ZstdStage::SignatureRest => Ok(false),
                    _ => Err(unexpected()),
                };
            }
            let bytes = std::mem::take(&mut self.segment);
            match self.stage {
                ZstdStage::SignatureFirst => {
                    self.frame_header = bytes;
                    self.window = 0;
                    self.content_size = None;
                    self.frame_size = 0;
                    self.stage = ZstdStage::SignatureRest;
                }
                ZstdStage::SignatureRest => {
                    self.frame_header.extend_from_slice(&bytes);
                    if self.frame_header[0] & 0xf0 == 0x50
                        && self.frame_header[1..] == [0x2a, 0x4d, 0x18]
                    {
                        self.stage = ZstdStage::SkipSize;
                    } else if self.frame_header == [0x28, 0xb5, 0x2f, 0xfd] {
                        self.stage = ZstdStage::Descriptor;
                    } else {
                        return Err(BodyError::message("invalid input: magic number mismatch"));
                    }
                }
                ZstdStage::Descriptor => {
                    self.descriptor = bytes[0];
                    self.frame_header.extend_from_slice(&bytes);
                    if self.descriptor & 8 != 0 {
                        return Err(BodyError::message("reserved bit set on frame header"));
                    }
                    self.stage = if self.descriptor & 0x20 == 0 {
                        ZstdStage::Window
                    } else {
                        ZstdStage::Dictionary
                    };
                }
                ZstdStage::Window => {
                    let base = 1u64 << (10 + (bytes[0] >> 3));
                    self.window = base + (base / 8) * u64::from(bytes[0] & 7);
                    self.frame_header.extend_from_slice(&bytes);
                    self.stage = ZstdStage::Dictionary;
                }
                ZstdStage::Dictionary => {
                    self.frame_header.extend_from_slice(&bytes);
                    self.stage = ZstdStage::ContentSize;
                }
                ZstdStage::ContentSize => {
                    if !bytes.is_empty() {
                        let mut encoded = [0; 8];
                        encoded[..bytes.len()].copy_from_slice(&bytes);
                        self.content_size = Some(
                            u64::from_le_bytes(encoded) + if bytes.len() == 2 { 256 } else { 0 },
                        );
                    }
                    self.frame_header.extend_from_slice(&bytes);
                    self.prepare_frame()?;
                }
                ZstdStage::SkipSize => {
                    self.payload_size = u32::from_le_bytes(bytes.try_into().unwrap()) as usize;
                    self.stage = ZstdStage::SkipData;
                }
                ZstdStage::SkipData => {
                    self.payload_size -= bytes.len();
                    if self.payload_size == 0 {
                        self.stage = ZstdStage::SignatureFirst;
                    }
                }
                ZstdStage::BlockHeader => {
                    let value = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], 0]);
                    self.last = value & 1 != 0;
                    let kind = (value >> 1) & 3;
                    if kind == 3 {
                        return Err(BodyError::message("invalid block type"));
                    }
                    let size = (value >> 3) as usize;
                    if size > 131072 || size as u64 > self.window {
                        return Err(BodyError::message("window size exceeded"));
                    }
                    self.payload_size = if kind == 1 { 1 } else { size };
                    self.block_header = bytes;
                    self.stage = ZstdStage::Payload;
                }
                ZstdStage::Payload => {
                    let header = std::mem::take(&mut self.block_header);
                    self.feed(&header)?;
                    self.feed(&bytes)?;
                    self.frame_size += self.output.len() as u64;
                    if self.content_size.is_some_and(|size| self.frame_size > size) {
                        return Err(BodyError::message(
                            "decompressed size exceeds declared size",
                        ));
                    }
                    if self.last
                        && self
                            .content_size
                            .is_some_and(|size| self.frame_size != size)
                    {
                        return Err(BodyError::message(
                            "decompressed size does not match declared size",
                        ));
                    }
                    if self.last && self.descriptor & 4 != 0 {
                        self.stage = ZstdStage::Checksum;
                    } else {
                        self.stage = if self.last {
                            ZstdStage::SignatureFirst
                        } else {
                            ZstdStage::BlockHeader
                        };
                        if !self.output.is_empty() {
                            return Ok(true);
                        }
                    }
                }
                ZstdStage::Checksum => {
                    self.feed(&bytes)?;
                    self.stage = ZstdStage::SignatureFirst;
                    if !self.output.is_empty() {
                        return Ok(true);
                    }
                }
            }
        }
    }
    async fn pump(&mut self, output: &mut [u8]) -> RawRead {
        if self.closed {
            return self.finish(
                0,
                false,
                Some(BodyError::message("decoder used after Close")),
            );
        }
        if self.decoder.is_none() {
            let initialized = Decoder::new().and_then(|mut decoder| {
                decoder.set_parameter(DParameter::WindowLogMax(29))?;
                Ok(decoder)
            });
            match initialized {
                Ok(decoder) => self.decoder = Some(decoder),
                Err(error) => {
                    let error = BodyError::message(error.to_string());
                    self.terminal = Some(error.clone());
                    return self.finish(0, false, Some(error));
                }
            }
        }
        if output.is_empty() && self.pending_output.is_empty() && self.position == self.output.len()
        {
            if let Some(error) = &self.terminal {
                return self.finish(0, false, Some(error.clone()));
            }
            if self.eof {
                return self.finish(0, true, None);
            }
        }
        let mut count = self.pending_output.len().min(output.len());
        output[..count].copy_from_slice(&self.pending_output[..count]);
        loop {
            let buffered = if self.block_ready || self.terminal.is_some() {
                (self.output.len() - self.position).min(output.len() - count)
            } else {
                0
            };
            output[count..count + buffered]
                .copy_from_slice(&self.output[self.position..self.position + buffered]);
            self.position += buffered;
            count += buffered;
            if count == output.len() {
                return self.finish(count, false, None);
            }
            if self.position == self.output.len() || !self.block_ready {
                if let Some(error) = &self.terminal {
                    return self.finish(count, false, Some(error.clone()));
                }
                if self.eof {
                    return self.finish(count, true, None);
                }
                if count > 0 {
                    self.pending_output.clear();
                    self.pending_output.extend_from_slice(&output[..count]);
                }
                if self.block_ready {
                    self.output.clear();
                    self.position = 0;
                    self.block_ready = false;
                }
                match self.next_block().await {
                    Ok(true) => self.block_ready = true,
                    Ok(false) => {
                        self.eof = true;
                        return self.finish(count, true, None);
                    }
                    Err(error) => {
                        self.terminal = Some(error.clone());
                        return self.finish(count, false, Some(error));
                    }
                }
            }
        }
    }
    fn finish(&mut self, count: usize, eof: bool, error: Option<BodyError>) -> RawRead {
        self.pending_output
            .drain(..count.min(self.pending_output.len()));
        result(count, eof, error)
    }
}
impl RawBody for ZstdBody {
    fn read<'a>(&'a mut self, output: &'a mut [u8]) -> BodyFuture<'a, RawRead> {
        Box::pin(async move { self.pump(output).await })
    }
    fn close(&mut self) -> BodyFuture<'_, Result<(), ExternalError>> {
        let initialized = self.decoder.is_some();
        self.decoder = None;
        self.output.clear();
        self.position = 0;
        self.pending_output.clear();
        // An unopened Go wrapper has no decoder to destroy and remains lazy.
        if initialized
            || !matches!(self.stage, ZstdStage::SignatureFirst)
            || self.terminal.is_some()
            || self.eof
        {
            self.closed = true;
        }
        self.source.close()
    }
}
