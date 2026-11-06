//! Bounded prefix I/O. Bodies and assets are runtime files, never discovery inputs.
use std::io::{BufRead, BufReader};
use std::path::Path;

use crate::runtime::CancellationSignal;

/// Only the definition header is bounded; SKILL.md and its body have no size cap.
pub(crate) const MAX_FRONTMATTER_BYTES: usize = 1024 * 1024;
const BUFFER_BYTES: usize = 4096;

#[cfg(test)]
thread_local! {
    pub(crate) static ON_CHUNK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    pub(crate) static AFTER_PREFIX: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    pub(crate) static READ_BYTES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

pub(crate) fn read(root: &Path, signal: &CancellationSignal) -> Result<Vec<u8>, String> {
    let directory = crate::package_io::PackageDirectory::open(root).map_err(|e| e.to_string())?;
    let prefix = || {
        let file = directory.file(Path::new(super::package::SKILL_MARKDOWN_FILE))?;
        // Bounded read-ahead, independent of the body size. Never validate body UTF-8.
        let mut reader = BufReader::with_capacity(BUFFER_BYTES, file);
        let mut header = Vec::new();
        let mut line_start = 0;
        let mut delimiters = 0;
        loop {
            crate::package_io::check_cancelled(signal)?;
            let bytes = reader.fill_buf()?;
            if bytes.is_empty() {
                if delimiters == 1 && matches!(&header[line_start..], b"---" | b"---\r") {
                    return Ok(header);
                }
                return Err(std::io::Error::other(
                    "SKILL.md frontmatter is missing its closing delimiter",
                ));
            }
            #[cfg(test)]
            READ_BYTES.with(|count| count.set(count.get() + bytes.len() as u64));
            #[cfg(test)]
            ON_CHUNK.with(|hook| {
                if let Some(hook) = hook.borrow_mut().take() {
                    hook();
                }
            });
            let mut used = 0;
            for &byte in bytes {
                used += 1;
                if header.len() == MAX_FRONTMATTER_BYTES {
                    return Err(std::io::Error::other("SKILL.md frontmatter exceeds 1 MiB"));
                }
                header.push(byte);
                if line_start == 0
                    && !b"---\n".starts_with(&header)
                    && !b"---\r\n".starts_with(&header)
                {
                    return Err(std::io::Error::other(
                        "SKILL.md must start with a YAML frontmatter delimiter",
                    ));
                }
                if byte == b'\n' {
                    if matches!(&header[line_start..], b"---\n" | b"---\r\n") {
                        delimiters += 1;
                    }
                    if delimiters == 2 {
                        return Ok(header);
                    }
                    line_start = header.len();
                }
            }
            reader.consume(used);
        }
    };
    let header = prefix().map_err(|e| e.to_string())?;
    #[cfg(test)]
    AFTER_PREFIX.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook();
        }
    });
    if prefix().map_err(|e| e.to_string())? != header {
        return Err("Skill definition changed during inspection".into());
    }
    crate::package_io::check_cancelled(signal).map_err(|e| e.to_string())?;
    Ok(header)
}
