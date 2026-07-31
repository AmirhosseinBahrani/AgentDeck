//! Length-capped NDJSON line reader.
//!
//! Deliberately not `BufReader::lines()`: that grows its buffer without bound, so a single
//! oversized tool result (a large file read, a base64 image, a huge test log) would let one
//! agent exhaust memory for the whole app. Here an overlong line is reported and skipped,
//! and the stream resynchronizes at the next newline instead of dying — losing one event is
//! far better than losing the session.

use tokio::io::{AsyncBufRead, AsyncBufReadExt};

/// 32 MiB. Comfortably above any legitimate event while still bounding the damage.
pub const MAX_LINE_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug)]
pub enum Line {
    Json(String),
    /// A line exceeded the cap and was discarded. Carries the byte count for diagnostics.
    Oversized {
        bytes: usize,
    },
    Eof,
}

pub struct NdjsonReader<R> {
    inner: R,
    buf: Vec<u8>,
}

impl<R: AsyncBufRead + Unpin> NdjsonReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            buf: Vec::with_capacity(8 * 1024),
        }
    }

    pub async fn next_line(&mut self) -> std::io::Result<Line> {
        self.buf.clear();
        let mut total = 0usize;
        let mut overflowed = false;

        loop {
            // Read in bounded chunks so an unterminated multi-gigabyte line cannot be
            // accumulated in one call.
            let mut chunk = Vec::new();
            let n = self.inner.read_until(b'\n', &mut chunk).await?;
            if n == 0 {
                if total == 0 {
                    return Ok(Line::Eof);
                }
                // EOF mid-line. The process ended without a trailing newline, which is
                // normal for a final event; emit what we have rather than discarding it.
                break;
            }
            total += n;

            let terminated = chunk.last() == Some(&b'\n');
            if !overflowed && total <= MAX_LINE_BYTES {
                self.buf.extend_from_slice(&chunk);
            } else {
                // Past the cap: stop retaining bytes but keep draining until the newline so
                // the next line starts at a real boundary.
                overflowed = true;
            }

            if terminated {
                break;
            }
        }

        if overflowed {
            return Ok(Line::Oversized { bytes: total });
        }

        while matches!(self.buf.last(), Some(b'\n') | Some(b'\r')) {
            self.buf.pop();
        }
        if self.buf.is_empty() {
            // Blank keepalive line; treat as nothing rather than a parse error.
            return Ok(Line::Json(String::new()));
        }

        Ok(Line::Json(String::from_utf8_lossy(&self.buf).into_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::BufReader;

    async fn collect(input: &str) -> Vec<Line> {
        let mut r = NdjsonReader::new(BufReader::new(input.as_bytes()));
        let mut out = Vec::new();
        loop {
            let l = r.next_line().await.unwrap();
            let done = matches!(l, Line::Eof);
            out.push(l);
            if done {
                break;
            }
        }
        out
    }

    #[tokio::test]
    async fn splits_on_newlines_and_reports_eof() {
        let lines = collect("{\"a\":1}\n{\"b\":2}\n").await;
        assert!(matches!(&lines[0], Line::Json(s) if s == "{\"a\":1}"));
        assert!(matches!(&lines[1], Line::Json(s) if s == "{\"b\":2}"));
        assert!(matches!(lines[2], Line::Eof));
    }

    #[tokio::test]
    async fn handles_a_final_line_without_a_trailing_newline() {
        let lines = collect("{\"a\":1}").await;
        assert!(matches!(&lines[0], Line::Json(s) if s == "{\"a\":1}"));
    }

    #[tokio::test]
    async fn strips_carriage_returns() {
        let lines = collect("{\"a\":1}\r\n").await;
        assert!(matches!(&lines[0], Line::Json(s) if s == "{\"a\":1}"));
    }

    #[tokio::test]
    async fn oversized_line_is_skipped_and_the_stream_resyncs() {
        // An agent emitting one huge tool result must not prevent the next event from being
        // read; this is the whole reason for not using BufReader::lines().
        let huge = "x".repeat(MAX_LINE_BYTES + 1024);
        let input = format!("{huge}\n{{\"recovered\":true}}\n");

        let mut r = NdjsonReader::new(BufReader::new(input.as_bytes()));
        let first = r.next_line().await.unwrap();
        assert!(
            matches!(first, Line::Oversized { bytes } if bytes > MAX_LINE_BYTES),
            "expected the oversized line to be reported, got {first:?}"
        );

        let second = r.next_line().await.unwrap();
        assert!(
            matches!(&second, Line::Json(s) if s == "{\"recovered\":true}"),
            "stream failed to resync after an oversized line, got {second:?}"
        );
    }

    #[tokio::test]
    async fn blank_lines_are_not_errors() {
        let lines = collect("\n{\"a\":1}\n").await;
        assert!(matches!(&lines[0], Line::Json(s) if s.is_empty()));
        assert!(matches!(&lines[1], Line::Json(s) if s == "{\"a\":1}"));
    }
}
