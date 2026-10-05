//! Multi-threaded PLog parsing.
//!
//! PLog records are independent lines, so the reader thread cuts the input
//! into newline-aligned chunks, workers parse them, and the reader merges the
//! results back in input order. Merging in order keeps record order, line
//! numbers and "first error wins" identical to the sequential parser.

use std::collections::{BTreeMap, HashMap};
use std::io::{self, BufRead};
use std::sync::{Mutex, mpsc};
use std::thread;

use super::{RawRecord, TraceBuilder, parse_line};
use crate::error::{ParseError, ValidationError};
use crate::model::{Span, SpanName, SpanNames, Trace};

const CHUNK_BYTES: usize = 4 << 20;
const MAX_WORKERS: usize = 8;
const SMALL_TABLE: usize = 16;

pub(super) fn worker_count() -> usize {
    thread::available_parallelism()
        .map_or(1, |count| count.get())
        .min(MAX_WORKERS)
}

pub(super) fn parse_parallel<R: BufRead>(reader: R, workers: usize) -> Result<Trace, ParseError> {
    parse_in_chunks(reader, workers, CHUNK_BYTES)
}

fn parse_in_chunks<R: BufRead>(
    mut reader: R,
    workers: usize,
    chunk_bytes: usize,
) -> Result<Trace, ParseError> {
    let (chunk_sender, chunk_receiver) = mpsc::sync_channel::<(usize, Vec<u8>)>(workers);
    let chunk_receiver = Mutex::new(chunk_receiver);
    let (result_sender, result_receiver) = mpsc::channel::<(usize, Chunk)>();

    thread::scope(|scope| {
        for _ in 0..workers {
            let chunk_receiver = &chunk_receiver;
            let result_sender = result_sender.clone();
            scope.spawn(move || {
                loop {
                    let next = chunk_receiver
                        .lock()
                        .expect("chunk queue lock is never poisoned")
                        .recv();
                    let Ok((index, bytes)) = next else { break };
                    if result_sender.send((index, parse_chunk(&bytes))).is_err() {
                        break;
                    }
                }
            });
        }
        drop(result_sender);

        let mut merger = Merger::default();
        let mut sent = 0;
        let mut read_error = None;
        let mut saw_input = false;
        while merger.error.is_none() {
            let (bytes, error) = read_chunk(&mut reader, chunk_bytes);
            saw_input |= !bytes.is_empty() || error.is_some();
            if !bytes.is_empty() {
                if chunk_sender.send((sent, bytes)).is_err() {
                    break;
                }
                sent += 1;
            }
            if error.is_some() || !reader_has_more(&mut reader, &mut read_error) {
                read_error = read_error.or(error);
                break;
            }
            while let Ok((index, chunk)) = result_receiver.try_recv() {
                merger.accept(index, chunk);
            }
        }
        drop(chunk_sender);

        while merger.error.is_none() && merger.next < sent {
            let Ok((index, chunk)) = result_receiver.recv() else {
                break;
            };
            merger.accept(index, chunk);
        }

        if let Some(error) = merger.error {
            return Err(error);
        }
        if let Some(error) = read_error {
            return Err(ParseError::line(merger.lines + 1, error.to_string()));
        }
        if !saw_input {
            return Err(ParseError::EmptyInput);
        }
        merger.builder.finish()
    })
}

/// Peeks for EOF so the loop does not send an empty trailing chunk.
fn reader_has_more<R: BufRead>(reader: &mut R, error: &mut Option<io::Error>) -> bool {
    loop {
        match reader.fill_buf() {
            Ok(buffer) => return !buffer.is_empty(),
            Err(read_error) if read_error.kind() == io::ErrorKind::Interrupted => {}
            Err(read_error) => {
                *error = Some(read_error);
                return false;
            }
        }
    }
}

/// Reads about `chunk_bytes`, extended to the end of the current line. On a
/// read error, returns the complete lines read so far alongside the error.
fn read_chunk<R: BufRead>(reader: &mut R, chunk_bytes: usize) -> (Vec<u8>, Option<io::Error>) {
    let mut chunk = Vec::with_capacity(chunk_bytes + 256);
    while chunk.len() < chunk_bytes {
        let buffer = match reader.fill_buf() {
            Ok(buffer) => buffer,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return (complete_lines(chunk), Some(error)),
        };
        if buffer.is_empty() {
            return (chunk, None);
        }
        let take = buffer.len().min(chunk_bytes - chunk.len());
        chunk.extend_from_slice(&buffer[..take]);
        reader.consume(take);
    }
    if chunk.last() == Some(&b'\n') {
        return (chunk, None);
    }
    match reader.read_until(b'\n', &mut chunk) {
        Ok(_) => (chunk, None),
        Err(error) => (complete_lines(chunk), Some(error)),
    }
}

fn complete_lines(mut chunk: Vec<u8>) -> Vec<u8> {
    let end = chunk
        .iter()
        .rposition(|&byte| byte == b'\n')
        .map_or(0, |newline| newline + 1);
    chunk.truncate(end);
    chunk
}

#[derive(Default)]
struct Chunk {
    builder: TraceBuilder,
    headers: Vec<u32>,
    lines: usize,
    /// The first malformed line, numbered from the start of the chunk.
    error: Option<(usize, String)>,
}

fn parse_chunk(bytes: &[u8]) -> Chunk {
    let (text, invalid_utf8) = match std::str::from_utf8(bytes) {
        Ok(text) => (text, false),
        Err(error) => {
            let valid = &bytes[..error.valid_up_to()];
            let end = valid
                .iter()
                .rposition(|&byte| byte == b'\n')
                .map_or(0, |newline| newline + 1);
            let text = std::str::from_utf8(&bytes[..end]).expect("prefix is valid UTF-8");
            (text, true)
        }
    };

    let mut chunk = Chunk::default();
    for line in text.split_terminator('\n') {
        chunk.lines += 1;
        let line = line.strip_suffix('\r').unwrap_or(line);
        match parse_line(line) {
            // Duplicate headers are detected when chunks are merged, where the
            // previous chunks' headers are known.
            Ok(RawRecord::Header(version)) => chunk.headers.push(version),
            Ok(record) => {
                if let Err(error) = chunk.builder.push(record) {
                    unreachable!("only header records fail to push: {error}");
                }
            }
            Err(message) => {
                chunk.error = Some((chunk.lines, message));
                return chunk;
            }
        }
    }
    if invalid_utf8 {
        chunk.error = Some((
            chunk.lines + 1,
            "stream did not contain valid UTF-8".to_owned(),
        ));
    }
    chunk
}

#[derive(Default)]
struct Merger {
    builder: TraceBuilder,
    pending: BTreeMap<usize, Chunk>,
    next: usize,
    lines: usize,
    error: Option<ParseError>,
}

impl Merger {
    fn accept(&mut self, index: usize, chunk: Chunk) {
        self.pending.insert(index, chunk);
        while self.error.is_none() {
            let Some(chunk) = self.pending.remove(&self.next) else {
                break;
            };
            self.next += 1;
            if let Err(error) = self.merge(chunk) {
                self.error = Some(error);
            }
        }
    }

    fn merge(&mut self, chunk: Chunk) -> Result<(), ParseError> {
        let Chunk {
            builder,
            headers,
            lines,
            error,
        } = chunk;

        // A chunk's headers all precede its first malformed line.
        for version in headers {
            if self.builder.version.replace(version).is_some() {
                return Err(ValidationError::DuplicateHeader.into());
            }
        }
        if let Some((line, message)) = error {
            return Err(ParseError::line(self.lines + line, message));
        }
        self.lines += lines;

        let TraceBuilder {
            span_names,
            version: _,
            mut meta,
            mut stages,
            mut lanes,
            mut instructions,
            spans,
            mut events,
            mut counters,
            mut retires,
        } = builder;
        let target = &mut self.builder;
        target.meta.append(&mut meta);
        target.stages.append(&mut stages);
        target.lanes.append(&mut lanes);
        target.instructions.append(&mut instructions);
        target.events.append(&mut events);
        target.counters.append(&mut counters);
        target.retires.append(&mut retires);
        adopt_spans(
            &mut target.span_names,
            &mut target.spans,
            spans,
            &span_names,
        );
        Ok(())
    }
}

/// Moves a chunk's spans into the trace, swapping each chunk-local name for
/// the trace-wide interned one so every name keeps a single allocation.
fn adopt_spans(
    names: &mut SpanNames,
    target: &mut Vec<Span>,
    mut spans: Vec<Span>,
    local_names: &SpanNames,
) {
    let table = local_names
        .iter()
        .map(|local| (local.as_ptr(), names.intern(local)))
        .collect::<Vec<_>>();
    let hashed =
        (table.len() > SMALL_TABLE).then(|| table.iter().cloned().collect::<HashMap<_, _>>());
    let shared = |local: &SpanName| -> SpanName {
        let pointer = local.as_ptr();
        let shared = match &hashed {
            Some(hashed) => hashed.get(&pointer),
            None => table
                .iter()
                .find(|(candidate, _)| *candidate == pointer)
                .map(|(_, shared)| shared),
        };
        shared
            .expect("chunk spans use the chunk's interned names")
            .clone()
    };
    for span in &mut spans {
        span.lane = shared(&span.lane);
        span.stage = shared(&span.stage);
    }
    target.append(&mut spans);
}

#[cfg(test)]
mod tests {
    use std::io::{self, BufReader, Cursor, Read};

    use super::parse_in_chunks;
    use crate::error::ParseError;
    use crate::model::Trace;
    use crate::parser::parse_plog_reader_with_limit;

    const FIXTURE: &str = include_str!("../../examples/classic_ooo_bottleneck.plog");

    fn sequential(input: &[u8]) -> Result<Trace, ParseError> {
        parse_plog_reader_with_limit(Cursor::new(input), None)
    }

    fn assert_matches_sequential(input: &[u8]) {
        let expected = sequential(input);
        for chunk_bytes in [1, 7, 64, 4096, 1 << 20] {
            for workers in [1, 3] {
                assert_eq!(
                    parse_in_chunks(Cursor::new(input), workers, chunk_bytes),
                    expected,
                    "chunk_bytes={chunk_bytes} workers={workers}"
                );
            }
        }
    }

    fn body(lines: usize) -> String {
        let mut text = String::from("PLOG\t1\nSTAGE\tIF\tFetch\nLANE\tmain\tMain\n");
        for id in 0..lines {
            text.push_str(&format!(
                "I\t{id}\tpc=0x{id:x}\nB\t{id}\t1\t{id}\tmain\tIF\n"
            ));
        }
        text
    }

    #[test]
    fn parses_fixtures_like_the_sequential_parser() {
        assert_matches_sequential(FIXTURE.as_bytes());
        assert_matches_sequential(body(50).as_bytes());
    }

    #[test]
    fn interns_each_name_once_across_chunks() {
        let trace = parse_in_chunks(Cursor::new(body(50)), 3, 16).expect("valid trace");
        let first = &trace.spans[0];
        assert!(trace.spans.iter().all(|span| {
            std::ptr::eq(span.stage.as_str(), first.stage.as_str())
                && std::ptr::eq(span.lane.as_str(), first.lane.as_str())
        }));
    }

    #[test]
    fn handles_line_endings_and_empty_input() {
        assert_matches_sequential(b"");
        assert_matches_sequential(b"\n");
        assert_matches_sequential(body(5).trim_end().as_bytes());
        assert_matches_sequential(body(5).replace('\n', "\r\n").as_bytes());
    }

    #[test]
    fn reports_the_first_error_with_its_line_number() {
        let mut late_error = body(40);
        late_error.push_str("X\t1\n");
        late_error.push_str(&body(5)[7..]);
        assert_matches_sequential(late_error.as_bytes());

        let mut duplicate_then_error = body(20);
        duplicate_then_error.push_str("PLOG\t1\nI\tnot-a-number\n");
        assert_matches_sequential(duplicate_then_error.as_bytes());

        let mut error_then_duplicate = body(20);
        error_then_duplicate.push_str("I\tnot-a-number\nPLOG\t1\n");
        assert_matches_sequential(error_then_duplicate.as_bytes());

        let mut invalid_utf8 = body(30).into_bytes();
        invalid_utf8.extend_from_slice(b"META\tname\t\xff\n");
        invalid_utf8.extend_from_slice(body(3).as_bytes());
        assert_matches_sequential(&invalid_utf8);
    }

    struct FailAfter {
        inner: Cursor<Vec<u8>>,
        remaining: usize,
    }

    impl Read for FailAfter {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.remaining == 0 {
                return Err(io::Error::other("disk on fire"));
            }
            let limit = buf.len().min(self.remaining);
            let read = self.inner.read(&mut buf[..limit])?;
            self.remaining -= read;
            Ok(read)
        }
    }

    #[test]
    fn reports_read_errors_at_the_line_being_read() {
        let input = body(40).into_bytes();
        for fail_at in [0, 1, 30, 31, 500, 777] {
            let reader = |capacity| {
                BufReader::with_capacity(
                    capacity,
                    FailAfter {
                        inner: Cursor::new(input.clone()),
                        remaining: fail_at,
                    },
                )
            };
            let expected = parse_plog_reader_with_limit(reader(16), None);
            for chunk_bytes in [1, 64, 4096] {
                assert_eq!(
                    parse_in_chunks(reader(16), 3, chunk_bytes),
                    expected,
                    "fail_at={fail_at} chunk_bytes={chunk_bytes}"
                );
            }
        }
    }
}
