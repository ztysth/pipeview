use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::konata::{parse_konata_preview_reader, parse_konata_reader};
use crate::model::Trace;
use crate::parser::{parse_plog_preview_reader, parse_plog_reader};

const ZSTD_EXTENSION: &str = "zst";
const ZSTD_LEVEL: i32 = 3;
pub const DEFAULT_MAX_INPUT_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputFormat {
    Auto,
    Plog,
    Konata,
}

pub fn read_plog_text(path: &Path) -> Result<String> {
    read_plog_text_with_limit(path, DEFAULT_MAX_INPUT_BYTES)
}

pub fn read_plog_text_with_limit(path: &Path, max_bytes: u64) -> Result<String> {
    if is_zstd_path(path) {
        read_zstd_text(path, max_bytes)
    } else {
        read_plain_text(path, max_bytes)
    }
}

pub fn read_plog_trace(path: &Path, max_bytes: u64) -> Result<Trace> {
    read_trace(path, max_bytes, InputFormat::Plog)
}

pub fn read_plog_preview_trace(path: &Path, max_bytes: u64, span_limit: usize) -> Result<Trace> {
    if is_zstd_path(path) {
        let input =
            File::open(path).with_context(|| format!("failed to read {}", path.display()))?;
        let decoder = zstd::Decoder::new(input)
            .with_context(|| format!("failed to decode {}", path.display()))?;
        let reader = BufReader::new(LimitedReader::new(decoder, max_bytes, path));
        parse_plog_preview_reader(reader, span_limit)
            .with_context(|| format!("failed to parse {}", path.display()))
    } else {
        check_plain_size(path, max_bytes)?;
        let input =
            File::open(path).with_context(|| format!("failed to read {}", path.display()))?;
        let reader = BufReader::new(LimitedReader::new(input, max_bytes, path));
        parse_plog_preview_reader(reader, span_limit)
            .with_context(|| format!("failed to parse {}", path.display()))
    }
}

pub fn read_trace(path: &Path, max_bytes: u64, format: InputFormat) -> Result<Trace> {
    let format = resolve_input_format(path, format);
    if is_zstd_path(path) {
        read_zstd_trace(path, max_bytes, format)
    } else {
        read_plain_trace(path, max_bytes, format)
    }
}

pub fn read_konata_trace(path: &Path, max_bytes: u64) -> Result<Trace> {
    read_trace(path, max_bytes, InputFormat::Konata)
}

pub fn read_konata_preview_trace(
    path: &Path,
    max_bytes: u64,
    instruction_limit: usize,
) -> Result<Trace> {
    if is_zstd_path(path) {
        let input =
            File::open(path).with_context(|| format!("failed to read {}", path.display()))?;
        let decoder = zstd::Decoder::new(input)
            .with_context(|| format!("failed to decode {}", path.display()))?;
        let reader = BufReader::new(LimitedReader::new(decoder, max_bytes, path));
        parse_konata_preview_reader(reader, instruction_limit)
            .with_context(|| format!("failed to parse {}", path.display()))
    } else {
        check_plain_size(path, max_bytes)?;
        let input =
            File::open(path).with_context(|| format!("failed to read {}", path.display()))?;
        let reader = BufReader::new(LimitedReader::new(input, max_bytes, path));
        parse_konata_preview_reader(reader, instruction_limit)
            .with_context(|| format!("failed to parse {}", path.display()))
    }
}

fn parse_reader<R: std::io::BufRead>(reader: R, format: InputFormat) -> Result<Trace> {
    match format {
        InputFormat::Auto => unreachable!("input format must be resolved before parsing"),
        InputFormat::Plog => parse_plog_reader(reader).map_err(Into::into),
        InputFormat::Konata => parse_konata_reader(reader).map_err(Into::into),
    }
}

pub fn resolve_input_format(path: &Path, format: InputFormat) -> InputFormat {
    match format {
        InputFormat::Auto if is_konata_log_path(path) => InputFormat::Konata,
        InputFormat::Auto => InputFormat::Plog,
        explicit => explicit,
    }
}

pub fn compress_plog_file(path: &Path) -> Result<PathBuf> {
    if is_zstd_path(path) {
        bail!("{} is already a zstd-compressed file", path.display());
    }

    let output_path = compressed_path(path);
    let input = File::open(path).with_context(|| format!("failed to read {}", path.display()))?;
    let output = File::options()
        .write(true)
        .create_new(true)
        .open(&output_path)
        .with_context(|| format!("failed to create {}", output_path.display()))?;

    zstd::stream::copy_encode(input, output, ZSTD_LEVEL)
        .with_context(|| format!("failed to compress {}", path.display()))?;
    fs::remove_file(path).with_context(|| format!("failed to remove {}", path.display()))?;

    Ok(output_path)
}

pub fn compressed_path(path: &Path) -> PathBuf {
    let mut name: OsString = path.as_os_str().to_owned();
    name.push(format!(".{ZSTD_EXTENSION}"));
    PathBuf::from(name)
}

fn read_plain_text(path: &Path, max_bytes: u64) -> Result<String> {
    check_plain_size(path, max_bytes)?;

    let input = File::open(path).with_context(|| format!("failed to read {}", path.display()))?;
    read_limited_utf8(input, max_bytes, path, "read")
}

fn read_plain_trace(path: &Path, max_bytes: u64, format: InputFormat) -> Result<Trace> {
    check_plain_size(path, max_bytes)?;

    let input = File::open(path).with_context(|| format!("failed to read {}", path.display()))?;
    let reader = BufReader::new(LimitedReader::new(input, max_bytes, path));
    parse_reader(reader, format).with_context(|| format!("failed to parse {}", path.display()))
}

fn read_zstd_text(path: &Path, max_bytes: u64) -> Result<String> {
    let input = File::open(path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut decoder = zstd::Decoder::new(input)
        .with_context(|| format!("failed to decode {}", path.display()))?;
    read_limited_utf8(&mut decoder, max_bytes, path, "read decoded text from")
}

fn read_zstd_trace(path: &Path, max_bytes: u64, format: InputFormat) -> Result<Trace> {
    let input = File::open(path).with_context(|| format!("failed to read {}", path.display()))?;
    let decoder = zstd::Decoder::new(input)
        .with_context(|| format!("failed to decode {}", path.display()))?;
    let reader = BufReader::new(LimitedReader::new(decoder, max_bytes, path));
    parse_reader(reader, format).with_context(|| format!("failed to parse {}", path.display()))
}

fn check_plain_size(path: &Path, max_bytes: u64) -> Result<()> {
    let metadata =
        fs::metadata(path).with_context(|| format!("failed to stat {}", path.display()))?;
    if metadata.len() > max_bytes {
        bail!(
            "{} is {} MiB, above the configured input limit of {} MiB",
            path.display(),
            bytes_to_mib(metadata.len()),
            bytes_to_mib(max_bytes)
        );
    }

    Ok(())
}

fn read_limited_utf8<R: Read>(
    reader: R,
    max_bytes: u64,
    path: &Path,
    operation: &str,
) -> Result<String> {
    let limit = max_bytes
        .checked_add(1)
        .context("configured input limit is too large")?;
    let mut limited = reader.take(limit);
    let mut bytes = Vec::new();
    limited
        .read_to_end(&mut bytes)
        .with_context(|| format!("failed to {operation} {}", path.display()))?;

    if bytes.len() as u64 > max_bytes {
        bail!(
            "{} exceeds the configured decompressed input limit of {} MiB",
            path.display(),
            bytes_to_mib(max_bytes)
        );
    }

    String::from_utf8(bytes).with_context(|| format!("{} is not valid UTF-8", path.display()))
}

fn bytes_to_mib(bytes: u64) -> u64 {
    bytes.div_ceil(1024 * 1024)
}

struct LimitedReader<'a, R> {
    inner: R,
    read_bytes: u64,
    max_bytes: u64,
    path: &'a Path,
}

impl<'a, R> LimitedReader<'a, R> {
    fn new(inner: R, max_bytes: u64, path: &'a Path) -> Self {
        Self {
            inner,
            read_bytes: 0,
            max_bytes,
            path,
        }
    }
}

impl<R: Read> Read for LimitedReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let count = self.inner.read(buf)?;
        self.read_bytes = self.read_bytes.saturating_add(count as u64);

        if self.read_bytes > self.max_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{} exceeds the configured decompressed input limit of {} MiB",
                    self.path.display(),
                    bytes_to_mib(self.max_bytes)
                ),
            ));
        }

        Ok(count)
    }
}

fn is_zstd_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case(ZSTD_EXTENSION))
}

fn is_konata_log_path(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let name = name.to_ascii_lowercase();
    name.ends_with(".log") || name.ends_with(".log.zst")
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{InputFormat, resolve_input_format};

    #[test]
    fn auto_input_format_uses_konata_for_log_paths() {
        assert_eq!(
            resolve_input_format(Path::new("trace.log"), InputFormat::Auto),
            InputFormat::Konata
        );
        assert_eq!(
            resolve_input_format(Path::new("trace.log.zst"), InputFormat::Auto),
            InputFormat::Konata
        );
        assert_eq!(
            resolve_input_format(Path::new("trace.plog.zst"), InputFormat::Auto),
            InputFormat::Plog
        );
    }
}
