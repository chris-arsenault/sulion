//! Reading the appended region of a transcript without trusting its size.
//!
//! Transcripts are written by agents, so neither the delta nor a single line is
//! trustworthy input. The ingester reads a bounded window per tick and, when a
//! line outgrows that window, skips to the next boundary rather than waiting
//! forever for a newline that may never come.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Ceiling on how much of a file's delta is buffered in one tick, and with it
/// the longest line the ingester will accept. An unterminated line is never
/// committed and would otherwise be re-read in full on every tick, growing
/// without bound until the ingester OOMs and restarts.
pub(super) const MAX_READ_BYTES: usize = 8 * 1024 * 1024;

/// Offset just past the next newline at or after `from`, or `None` if the file
/// ends first. Scans in fixed-size chunks so skipping an arbitrarily long line
/// costs bounded memory.
pub(super) fn next_line_boundary(path: &Path, from: i64) -> std::io::Result<Option<i64>> {
    const CHUNK: usize = 64 * 1024;
    let mut file = std::fs::File::open(path)?;
    file.seek(SeekFrom::Start(from as u64))?;
    let mut chunk = vec![0u8; CHUNK];
    let mut position = from;
    loop {
        let read = file.read(&mut chunk)?;
        if read == 0 {
            return Ok(None);
        }
        if let Some(index) = chunk[..read].iter().position(|&b| b == b'\n') {
            return Ok(Some(position + index as i64 + 1));
        }
        position += read as i64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_line_boundary_finds_the_offset_past_the_newline() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        std::fs::write(&path, b"abc\ndef\n").unwrap();

        assert_eq!(next_line_boundary(&path, 0).unwrap(), Some(4));
        assert_eq!(next_line_boundary(&path, 4).unwrap(), Some(8));
    }

    #[test]
    fn next_line_boundary_spans_chunks_and_reports_unterminated_tails() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");

        // Longer than the 64 KiB scan chunk, so the newline is only found on a
        // later iteration — this is the case that skips an oversized line.
        let mut body = vec![b'x'; 200 * 1024];
        body.push(b'\n');
        body.extend_from_slice(b"next\n");
        std::fs::write(&path, &body).unwrap();
        assert_eq!(next_line_boundary(&path, 0).unwrap(), Some(200 * 1024 + 1));

        // A tail still being written has no boundary to resume from yet.
        std::fs::write(&path, vec![b'x'; 100 * 1024]).unwrap();
        assert_eq!(next_line_boundary(&path, 0).unwrap(), None);
    }
}
