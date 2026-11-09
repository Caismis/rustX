//! Bounded prefix I/O. Bodies and assets are runtime files, never discovery inputs.
use std::io::{BufRead, BufReader};
use std::path::Path;

use crate::runtime::CancellationSignal;

const BUFFER_BYTES: usize = 4096;

#[cfg(test)]
thread_local! {
    pub(crate) static ON_CHUNK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    pub(crate) static AFTER_PREFIX: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    pub(crate) static READ_BYTES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

pub(crate) fn read(path: &Path, signal: &CancellationSignal) -> Result<Vec<u8>, String> {
    let parent = path.parent().ok_or("Skill file has no parent directory")?;
    let name = path.file_name().ok_or("Skill file has no name")?;
    let directory = crate::package_io::PackageDirectory::open(parent).map_err(|e| e.to_string())?;
    let prefix = || {
        let file = directory.file(Path::new(name))?;
        // Bounded read-ahead, independent of the body size. Never validate body UTF-8.
        let mut reader = BufReader::with_capacity(BUFFER_BYTES, file);
        let mut header = Vec::new();
        let mut first = true;
        let mut previous_cr = false;
        loop {
            crate::package_io::check_cancelled(signal)?;
            let bytes = reader.fill_buf()?;
            if bytes.is_empty() {
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
            let used = bytes.len();
            let bytes = if first {
                first = false;
                bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes)
            } else {
                bytes
            };
            for &byte in bytes {
                let skip = previous_cr && byte == b'\n';
                previous_cr = byte == b'\r';
                if skip {
                    continue;
                }
                header.push(if previous_cr { b'\n' } else { byte });
                if header.len() <= 3 && !b"---".starts_with(&header) {
                    return Err(std::io::Error::other(
                        "SKILL.md must start with a YAML frontmatter delimiter",
                    ));
                }
                if header.len() >= 7 && header.ends_with(b"\n---") {
                    return Ok(header);
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
