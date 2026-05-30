use std::ffi::OsString;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

const ZSTD_EXTENSION: &str = "zst";
const ZSTD_LEVEL: i32 = 3;

pub fn read_plog_text(path: &Path) -> Result<String> {
    if is_zstd_path(path) {
        read_zstd_text(path)
    } else {
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))
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

fn read_zstd_text(path: &Path) -> Result<String> {
    let input = File::open(path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut decoder = zstd::Decoder::new(input)
        .with_context(|| format!("failed to decode {}", path.display()))?;
    let mut text = String::new();
    decoder
        .read_to_string(&mut text)
        .with_context(|| format!("failed to read decoded text from {}", path.display()))?;
    Ok(text)
}

fn is_zstd_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case(ZSTD_EXTENSION))
}
