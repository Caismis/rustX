//! Bounded prefix I/O. Bodies and assets are runtime files, never discovery inputs.
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;

use crate::runtime::CancellationSignal;

/// Bound the metadata read independently of the size of the body.
pub(crate) const MAX_FRONTMATTER_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const DEFAULT_DISABLED_KEY: &str = "rustx-default-disabled";
const BUFFER_BYTES: usize = 4096;

#[cfg(test)]
thread_local! {
    pub(crate) static ON_CHUNK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    pub(crate) static AFTER_PREFIX: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    pub(crate) static READ_BYTES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

pub(crate) fn read(path: &Path, signal: &CancellationSignal) -> Result<Vec<u8>, String> {
    verified_prefix(path, signal).map(|(header, _)| header)
}

fn verified_prefix(path: &Path, signal: &CancellationSignal) -> Result<(Vec<u8>, usize), String> {
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
        let mut remaining = MAX_FRONTMATTER_BYTES;
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
                let prefix = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
                remaining -= bytes.len() - prefix.len();
                prefix
            } else {
                bytes
            };
            for &byte in bytes {
                remaining = remaining
                    .checked_sub(1)
                    .ok_or_else(|| std::io::Error::other("SKILL.md frontmatter exceeds 2 MiB"))?;
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
                    return Ok((header, MAX_FRONTMATTER_BYTES - remaining));
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

/// Rewrite only package identity and the copy's default selection. The closing
/// delimiter and instruction body after it retain their original bytes.
pub(crate) fn rewrite(
    path: &Path,
    name: &str,
    copy: bool,
    signal: &CancellationSignal,
) -> io::Result<()> {
    let (header, end) = verified_prefix(path, signal).map_err(io::Error::other)?;
    let yaml = &header[3..header.len() - 3];
    let mut fields: serde_yaml::Mapping = serde_yaml::from_slice(yaml).map_err(io::Error::other)?;
    fields.insert("name".into(), name.into());
    if copy {
        let metadata = fields
            .entry("metadata".into())
            .or_insert_with(|| serde_yaml::Value::Mapping(serde_yaml::Mapping::new()));
        if !metadata.is_mapping() {
            *metadata = serde_yaml::Value::Mapping(serde_yaml::Mapping::new());
        }
        metadata
            .as_mapping_mut()
            .expect("metadata is a mapping")
            .insert(DEFAULT_DISABLED_KEY.into(), true.into());
    }
    let yaml = serde_yaml::to_string(&fields).map_err(io::Error::other)?;
    if yaml.len() + 7 > MAX_FRONTMATTER_BYTES {
        return Err(io::Error::other("Skill frontmatter exceeds 2 MiB"));
    }
    let mut original = std::fs::File::open(path)?;
    let mut file = tempfile::NamedTempFile::new_in(
        path.parent()
            .ok_or_else(|| io::Error::other("no package parent"))?,
    )?;
    file.write_all(b"---\n")?;
    file.write_all(yaml.as_bytes())?;
    file.write_all(b"---")?;
    io::Seek::seek(&mut original, io::SeekFrom::Start(end as u64))?;
    io::copy(&mut original, &mut file)?;
    file.as_file()
        .set_permissions(original.metadata()?.permissions())?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    Ok(())
}
