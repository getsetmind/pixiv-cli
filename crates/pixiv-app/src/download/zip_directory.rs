use std::{
    fmt,
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::Path,
};

#[derive(Debug)]
pub(super) enum ZipDirectoryError {
    Format,
    Eof,
    UnexpectedEof,
    Io(io::Error),
}

impl fmt::Display for ZipDirectoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Format => f.write_str("zip: not a valid zip file"),
            Self::Eof => f.write_str("EOF"),
            Self::UnexpectedEof => f.write_str("unexpected EOF"),
            Self::Io(error) => error.fmt(f),
        }
    }
}

impl From<io::Error> for ZipDirectoryError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub(super) fn read_names(path: &Path) -> Result<Vec<Vec<u8>>, ZipDirectoryError> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    let (start, count) = directory_end(&mut file, size)?;
    file.seek(SeekFrom::Start(start))?;
    let mut names = Vec::new();
    loop {
        match directory_header(&mut file) {
            Ok(name) => names.push(name),
            Err(error @ (ZipDirectoryError::Format | ZipDirectoryError::UnexpectedEof)) => {
                // Go accepts a truncated 16-bit directory count, including ZIP64 metadata.
                if names.len() as u16 == count as u16 {
                    return Ok(names);
                }
                return Err(error);
            }
            Err(error) => return Err(error),
        }
    }
}

fn directory_end(file: &mut File, size: u64) -> Result<(u64, u64), ZipDirectoryError> {
    let mut found = None;
    for window in [1024, 65 * 1024] {
        let length = size.min(window);
        let mut block = vec![0; length as usize];
        read_at(file, size - length, &mut block)?;
        if let Some(index) = end_signature(&block) {
            found = Some((size - length + index as u64, block[index..].to_vec()));
            break;
        }
        if length == size {
            break;
        }
    }
    let (mut end_offset, footer) = found.ok_or(ZipDirectoryError::Format)?;
    let mut count = u64::from(le16(&footer, 10));
    let mut directory_size = u64::from(le32(&footer, 12));
    let mut directory_offset = u64::from(le32(&footer, 16));
    if (count == 0xffff || directory_size == 0xffff_ffff || directory_offset == 0xffff_ffff)
        && let Some(locator_offset) = end_offset.checked_sub(20)
    {
        let mut locator = [0; 20];
        read_at(file, locator_offset, &mut locator)?;
        if le32(&locator, 0) == 0x0706_4b50 && le32(&locator, 4) == 0 && le32(&locator, 16) == 1 {
            let offset = le64(&locator, 8);
            if offset <= i64::MAX as u64 {
                let mut zip64 = [0; 56];
                read_at(file, offset, &mut zip64)?;
                if le32(&zip64, 0) != 0x0606_4b50 {
                    return Err(ZipDirectoryError::Format);
                }
                end_offset = offset;
                count = le64(&zip64, 32);
                directory_size = le64(&zip64, 40);
                directory_offset = le64(&zip64, 48);
            }
        }
    }
    if directory_size > i64::MAX as u64 || directory_offset > i64::MAX as u64 {
        return Err(ZipDirectoryError::Format);
    }
    let start = end_offset
        .checked_sub(directory_size)
        .filter(|start| *start < size)
        .ok_or(ZipDirectoryError::Format)?;
    let base = i128::from(end_offset) - i128::from(directory_size) - i128::from(directory_offset);
    if base > 0 && directory_offset < size {
        file.seek(SeekFrom::Start(directory_offset))?;
        if directory_header(file).is_ok() {
            return Ok((directory_offset, count));
        }
    }
    Ok((start, count))
}

fn end_signature(block: &[u8]) -> Option<usize> {
    let last = block.len().checked_sub(22)?;
    for index in (0..=last).rev() {
        if le32(block, index) == 0x0605_4b50 {
            let comment = usize::from(le16(block, index + 20));
            return (index + 22 + comment <= block.len()).then_some(index);
        }
    }
    None
}

fn directory_header(file: &mut File) -> Result<Vec<u8>, ZipDirectoryError> {
    let mut header = [0; 46];
    read_full(file, &mut header)?;
    if le32(&header, 0) != 0x0201_4b50 {
        return Err(ZipDirectoryError::Format);
    }
    let name_length = usize::from(le16(&header, 28));
    let extra_length = usize::from(le16(&header, 30));
    let comment_length = usize::from(le16(&header, 32));
    let mut fields = vec![0; name_length + extra_length + comment_length];
    read_full(file, &mut fields)?;
    validate_zip64_extra(&header, &fields[name_length..name_length + extra_length])?;
    Ok(fields[..name_length].to_vec())
}

fn validate_zip64_extra(header: &[u8], mut extra: &[u8]) -> Result<(), ZipDirectoryError> {
    let mut header_offset = u64::from(le32(header, 42));
    while extra.len() >= 4 {
        let tag = le16(extra, 0);
        let size = usize::from(le16(extra, 2));
        extra = &extra[4..];
        if size > extra.len() {
            break;
        }
        if tag == 1 {
            let size_fields = [24, 20]
                .into_iter()
                .filter(|offset| le32(header, *offset) == u32::MAX)
                .count()
                * 8;
            let offset_field = header_offset == u64::from(u32::MAX);
            let required = size_fields + usize::from(offset_field) * 8;
            if size < required {
                return Err(ZipDirectoryError::Format);
            }
            if offset_field {
                header_offset = le64(extra, size_fields);
            }
        }
        extra = &extra[size..];
    }
    Ok(())
}

fn read_at(file: &mut File, offset: u64, bytes: &mut [u8]) -> Result<(), ZipDirectoryError> {
    file.seek(SeekFrom::Start(offset))?;
    read_full(file, bytes).map_err(|error| match error {
        ZipDirectoryError::UnexpectedEof => ZipDirectoryError::Eof,
        error => error,
    })
}

fn read_full(file: &mut File, bytes: &mut [u8]) -> Result<(), ZipDirectoryError> {
    let mut used = 0;
    while used < bytes.len() {
        match file.read(&mut bytes[used..]) {
            Ok(0) if used == 0 => return Err(ZipDirectoryError::Eof),
            Ok(0) => return Err(ZipDirectoryError::UnexpectedEof),
            Ok(length) => used += length,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn le16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}
fn le32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("fixed ZIP field"),
    )
}
fn le64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(
        bytes[offset..offset + 8]
            .try_into()
            .expect("fixed ZIP field"),
    )
}
