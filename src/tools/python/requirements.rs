//! Incremental requirements syntax over the already-owned anonymous capture.
//! Only effective declaration spans are read back and retained. Comments and
//! leading/trailing whitespace, even on newline-free lines, need constant state.
use super::{PythonToolError, io_error};
use std::fs::File;
use std::os::unix::fs::FileExt;

fn invalid_utf8() -> PythonToolError {
    PythonToolError::InvalidPackage("the file is not valid UTF-8".into())
}

#[derive(Default)]
pub(super) struct Parser {
    utf8: Vec<u8>,
    position: u64,
    start: Option<u64>,
    end: u64,
    line: usize,
    quote: Option<char>,
    escaped: bool,
    previous_whitespace: bool,
    comment: bool,
    declarations: Vec<String>,
}

impl Parser {
    pub(super) fn new(offset: u64) -> Self {
        Self {
            position: offset,
            ..Self::default()
        }
    }
    pub(super) fn push(&mut self, bytes: &[u8], capture: &File) -> Result<(), PythonToolError> {
        let mut buffer = std::mem::take(&mut self.utf8);
        buffer.extend_from_slice(bytes);
        let valid = match std::str::from_utf8(&buffer) {
            Ok(text) => text.len(),
            Err(error) if error.error_len().is_none() => error.valid_up_to(),
            Err(_) => return Err(invalid_utf8()),
        };
        for character in std::str::from_utf8(&buffer[..valid]).unwrap().chars() {
            if character == '\n' {
                self.finish_line(capture)?;
            } else if !self.comment {
                if self.escaped {
                    self.escaped = false;
                } else if character == '\\' && self.quote.is_some() {
                    self.escaped = true;
                } else if matches!(character, '\'' | '"') {
                    if self.quote == Some(character) {
                        self.quote = None;
                    } else if self.quote.is_none() {
                        self.quote = Some(character);
                    }
                } else if character == '#'
                    && self.quote.is_none()
                    && (self.start.is_none() || self.previous_whitespace)
                {
                    self.comment = true;
                }
                if !self.comment && !character.is_whitespace() {
                    self.start.get_or_insert(self.position);
                    self.end = self.position + character.len_utf8() as u64;
                }
                self.previous_whitespace = character.is_whitespace();
            }
            self.position += character.len_utf8() as u64;
        }
        buffer.drain(..valid);
        self.utf8 = buffer;
        Ok(())
    }

    fn finish_line(&mut self, capture: &File) -> Result<(), PythonToolError> {
        self.line += 1;
        if let Some(start) = self.start.take() {
            let length = usize::try_from(self.end - start).map_err(|_| {
                PythonToolError::Storage("dependency declaration exceeds addressable memory".into())
            })?;
            let mut bytes = vec![0; length];
            capture.read_exact_at(&mut bytes, start).map_err(io_error)?;
            let text = std::str::from_utf8(&bytes).map_err(|_| invalid_utf8())?;
            self.declarations.push(
                super::parse_requirement(text, self.line)
                    .map_err(PythonToolError::InvalidPackage)?,
            );
        }
        self.quote = None;
        self.escaped = false;
        self.previous_whitespace = false;
        self.comment = false;
        Ok(())
    }

    pub(super) fn finish(mut self, capture: &File) -> Result<Vec<String>, PythonToolError> {
        if !self.utf8.is_empty() {
            return Err(invalid_utf8());
        }
        self.finish_line(capture)?;
        Ok(self.declarations)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn frozen_capture_read_failure_is_storage_failure() {
        for newline in [false, true] {
            let mut capture = tempfile::tempfile().unwrap();
            capture.write_all(b"six==1.16.0").unwrap();
            let mut parser = Parser::default();
            parser.push(b"six==1.16.0", &capture).unwrap();
            capture.set_len(0).unwrap();
            let result = if newline {
                parser.push(b"\n", &capture).map(|()| Vec::new())
            } else {
                parser.finish(&capture)
            };
            assert!(matches!(result, Err(PythonToolError::Storage(_))));
        }
    }

    #[test]
    fn issue527_large_comment_only_file_and_whitespace_retain_no_raw_lines() {
        let mut capture = tempfile::tempfile().unwrap();
        let mut parser = Parser::default();
        let mut write = |bytes: &[u8]| {
            capture.write_all(bytes).unwrap();
            parser.push(bytes, &capture).unwrap();
            assert!(parser.utf8.len() <= 3);
            assert!(parser.utf8.capacity() <= 2 * 65_536 + 8);
            assert!(parser.declarations.is_empty());
        };
        write(b"#");
        let comment = vec![b'x'; 65_536];
        for _ in 0..1024 {
            write(&comment);
        }
        write(b"\n");
        let whitespace = vec![b' '; 65_536];
        for _ in 0..1024 {
            write(&whitespace);
        }
        write(b"#");
        for _ in 0..1024 {
            write(&comment);
        }
        assert!(parser.finish(&capture).unwrap().is_empty());
    }

    #[test]
    fn capture_offsets_unicode_whitespace_and_quoted_comments_preserve_declarations() {
        let mut capture = tempfile::tempfile().unwrap();
        let prefix = b"earlier binary asset\0";
        capture.write_all(prefix).unwrap();
        let mut parser = Parser::new(prefix.len() as u64);
        let declaration = "demo @ https://example.invalid/a.whl#sha256=123 ; os_name == 'a # b'";
        let input = format!("\u{2003}{declaration}\u{2003}# ignored 🦀\r\n");
        for chunk in input.as_bytes().chunks(3) {
            capture.write_all(chunk).unwrap();
            parser.push(chunk, &capture).unwrap();
        }
        assert_eq!(parser.finish(&capture).unwrap(), [declaration]);
    }
}
