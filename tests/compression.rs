use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use pipeview::parser::parse_plog;
use pipeview::plog_io::{
    compress_plog_file, compressed_path, read_plog_text, read_plog_text_with_limit, read_plog_trace,
};

const PLOG: &str = concat!(
    "PLOG\t1\n",
    "STAGE\tIF\tFetch\n",
    "LANE\tmain\tMain\n",
    "I\t1\tpc=0x80000000\n",
    "B\t1\t1\t1\tmain\tIF\n",
    "R\t2\t1\tretire\n",
);

#[test]
fn compressed_plog_replaces_source_and_remains_readable() {
    let dir = test_dir("compress-roundtrip");
    fs::create_dir_all(&dir).expect("create temp dir");
    let input_path = dir.join("trace.plog");
    fs::write(&input_path, PLOG).expect("write plog");

    let output_path = compress_plog_file(&input_path).expect("compress plog");

    assert_eq!(output_path, dir.join("trace.plog.zst"));
    assert!(!input_path.exists());
    assert!(output_path.exists());

    let text = read_plog_text(&output_path).expect("read compressed plog");
    let trace = parse_plog(&text).expect("parse compressed plog text");
    assert_eq!(trace.instructions.len(), 1);

    fs::remove_dir_all(&dir).expect("remove temp dir");
}

#[test]
fn compressed_path_appends_zstd_extension() {
    assert_eq!(
        compressed_path(PathBuf::from("example.plog").as_path()),
        PathBuf::from("example.plog.zst")
    );
}

#[test]
fn user_provided_compressed_plog_is_decompressed_for_parsing() {
    let dir = test_dir("decompress-user-input");
    fs::create_dir_all(&dir).expect("create temp dir");
    let input_path = dir.join("external.plog.zst");
    let compressed = zstd::stream::encode_all(PLOG.as_bytes(), 3).expect("compress plog text");
    fs::write(&input_path, compressed).expect("write compressed plog");

    let text = read_plog_text(&input_path).expect("read compressed plog");
    let trace = parse_plog(&text).expect("parse decompressed plog");

    assert_eq!(text, PLOG);
    assert_eq!(trace.retires.len(), 1);

    fs::remove_dir_all(&dir).expect("remove temp dir");
}

#[test]
fn plain_plog_can_be_streamed_into_trace() {
    let dir = test_dir("stream-plain");
    fs::create_dir_all(&dir).expect("create temp dir");
    let input_path = dir.join("trace.plog");
    fs::write(&input_path, PLOG).expect("write plog");

    let trace = read_plog_trace(&input_path, 1024).expect("stream plain plog");

    assert_eq!(trace.instructions.len(), 1);
    assert_eq!(trace.spans.len(), 1);

    fs::remove_dir_all(&dir).expect("remove temp dir");
}

#[test]
fn compressed_plog_can_be_streamed_into_trace() {
    let dir = test_dir("stream-zstd");
    fs::create_dir_all(&dir).expect("create temp dir");
    let input_path = dir.join("trace.plog.zst");
    let compressed = zstd::stream::encode_all(PLOG.as_bytes(), 3).expect("compress plog text");
    fs::write(&input_path, compressed).expect("write compressed plog");

    let trace = read_plog_trace(&input_path, 1024).expect("stream compressed plog");

    assert_eq!(trace.instructions.len(), 1);
    assert_eq!(trace.retires.len(), 1);

    fs::remove_dir_all(&dir).expect("remove temp dir");
}

#[test]
fn already_compressed_plog_is_not_compressed_again() {
    let dir = test_dir("compress-reject-zst");
    fs::create_dir_all(&dir).expect("create temp dir");
    let input_path = dir.join("trace.plog.zst");
    fs::write(&input_path, PLOG).expect("write fake compressed plog");

    assert!(compress_plog_file(&input_path).is_err());

    fs::remove_dir_all(&dir).expect("remove temp dir");
}

#[test]
fn plain_plog_over_input_limit_is_rejected_before_parsing() {
    let dir = test_dir("plain-limit");
    fs::create_dir_all(&dir).expect("create temp dir");
    let input_path = dir.join("trace.plog");
    fs::write(&input_path, PLOG).expect("write plog");

    let err = read_plog_text_with_limit(&input_path, 8).expect_err("limit rejects input");

    assert!(err.to_string().contains("input limit"));

    fs::remove_dir_all(&dir).expect("remove temp dir");
}

#[test]
fn compressed_plog_over_decompressed_limit_is_rejected() {
    let dir = test_dir("zstd-limit");
    fs::create_dir_all(&dir).expect("create temp dir");
    let input_path = dir.join("trace.plog.zst");
    let compressed = zstd::stream::encode_all(PLOG.as_bytes(), 3).expect("compress plog text");
    fs::write(&input_path, compressed).expect("write compressed plog");

    let err = read_plog_text_with_limit(&input_path, 8).expect_err("limit rejects input");

    assert!(format!("{err:#}").contains("decompressed input limit"));

    fs::remove_dir_all(&dir).expect("remove temp dir");
}

#[test]
fn compressed_stream_over_decompressed_limit_is_rejected() {
    let dir = test_dir("zstd-stream-limit");
    fs::create_dir_all(&dir).expect("create temp dir");
    let input_path = dir.join("trace.plog.zst");
    let compressed = zstd::stream::encode_all(PLOG.as_bytes(), 3).expect("compress plog text");
    fs::write(&input_path, compressed).expect("write compressed plog");

    let err = read_plog_trace(&input_path, 8).expect_err("limit rejects input");

    assert!(format!("{err:#}").contains("decompressed input limit"));

    fs::remove_dir_all(&dir).expect("remove temp dir");
}

fn test_dir(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    std::env::temp_dir().join(format!("pipeview-{name}-{}-{now}", std::process::id()))
}
