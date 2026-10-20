//! Newline-delimited messages, as MCP's stdio transport and Lectern's pipe carry them, read with a
//! cap on their length so a peer can't make the reader hold more than that.

use std::io::{self, BufRead, BufReader, ErrorKind, Read};

/// What [`LineReader::next_line`] read.
#[derive(Debug, PartialEq, Eq)]
pub enum Line {
    /// A line, without its `\n` (or `\r\n`). Bytes that aren't UTF-8 are replaced.
    Text(String),
    /// A line longer than the cap, skipped to its end.
    TooLong,
    /// The stream ended.
    End,
}

/// Reads lines of at most `cap` bytes from a stream.
pub struct LineReader<R> {
    inner: BufReader<R>,
    cap: usize,
}

impl<R: Read> LineReader<R> {
    pub fn new(inner: R, cap: usize) -> Self {
        Self {
            inner: BufReader::new(inner),
            cap,
        }
    }

    /// The next line. A last line that the stream ends without a `\n` still counts.
    pub fn next_line(&mut self) -> io::Result<Line> {
        let mut line = Vec::new();
        let mut too_long = false;
        loop {
            let chunk = match self.inner.fill_buf() {
                Ok(chunk) => chunk,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            if chunk.is_empty() {
                if !too_long && line.is_empty() {
                    return Ok(Line::End);
                }
                return Ok(self.finish(line, too_long));
            }
            let (part, ends) = match chunk.iter().position(|&b| b == b'\n') {
                Some(at) => (&chunk[..at], Some(at + 1)),
                None => (chunk, None),
            };
            // Kept only while the line fits; the rest of a long one is read past, never held.
            if !too_long && line.len() + part.len() <= self.cap + 1 {
                line.extend_from_slice(part);
            } else {
                too_long = true;
                line.clear();
            }
            let used = ends.unwrap_or(chunk.len());
            self.inner.consume(used);
            if ends.is_some() {
                return Ok(self.finish(line, too_long));
            }
        }
    }

    /// The line read, `bytes`, without a `\r` at its end; too long when it was cut off already
    /// (`too_long`) or is still over the cap.
    fn finish(&self, mut bytes: Vec<u8>, too_long: bool) -> Line {
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
        if too_long || bytes.len() > self.cap {
            return Line::TooLong;
        }
        Line::Text(text_of(bytes))
    }
}

/// `bytes` as text, any that aren't UTF-8 replaced.
fn text_of(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}
