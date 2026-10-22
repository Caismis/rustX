//! Descriptor-authorized preview reads. File budgets are independent of RPC framing.
use super::{SessionFileReadFailure, SessionFileReference, invalid, open_file};
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

pub(crate) const PAGE_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const PAGE_LINES: usize = 5000;
pub(crate) const FILE_BYTES: usize = 32 * 1024 * 1024;
pub(crate) const OFFICE_BYTES: usize = 50 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ByteRange {
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub offset: u64,
    #[schemars(range(max = 2_097_152))]
    pub length: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ReadOptions {
    Bytes {
        max_bytes: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        range: Option<ByteRange>,
    },
    Text {
        offset: usize,
        limit: usize,
    },
}

#[derive(Debug, Serialize)]
pub(crate) struct PreviewRead {
    pub version: String,
    pub bytes: u64,
    pub offset: u64,
    pub eof: bool,
    /// Present only for text pages. The body contains UTF-8 without a final LF.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lines: Option<usize>,
    #[serde(skip)]
    pub data: Vec<u8>,
}

fn version(metadata: &std::fs::Metadata) -> String {
    format!(
        "{}:{}:{}:{}:{}:{}:{}",
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec()
    )
}

fn too_large() -> io::Error {
    invalid(
        SessionFileReadFailure::TooLarge,
        "File exceeds preview read budget",
    )
}

/// One exact opened descriptor owns the complete read or selected page/window.
pub(crate) fn read_authorized(
    root: &Path,
    reference: &SessionFileReference,
    options: &ReadOptions,
    authorized: impl Fn() -> io::Result<()>,
) -> io::Result<PreviewRead> {
    authorized()?;
    let (mut opened, _) = open_file(root, &reference.path, Some(&reference.scope), || {})?;
    let metadata = opened.file.metadata()?;
    let initial_version = version(&metadata);
    authorized()?;
    let (data, offset, lines, eof) = match options {
        ReadOptions::Bytes { max_bytes, range } => {
            if !matches!(*max_bytes, FILE_BYTES | OFFICE_BYTES) {
                return Err(too_large());
            }
            let (offset, capacity) = match range {
                Some(range)
                    if range.length > 0
                        && range.length <= PAGE_BYTES
                        && range
                            .offset
                            .checked_add(range.length as u64)
                            .is_some_and(|end| end <= 9_007_199_254_740_991) =>
                {
                    (range.offset, range.length)
                }
                Some(_) => return Err(too_large()),
                None => {
                    if metadata.len() > *max_bytes as u64 {
                        return Err(too_large());
                    }
                    (0, *max_bytes)
                }
            };
            opened.file.seek(SeekFrom::Start(offset))?;
            let mut data = Vec::new();
            let mut buffer = vec![0; 64 * 1024];
            let bound = capacity + usize::from(range.is_none());
            while data.len() < bound {
                authorized()?;
                let remaining = (bound - data.len()).min(buffer.len());
                let count = opened.file.read(&mut buffer[..remaining])?;
                if count == 0 {
                    break;
                }
                data.extend_from_slice(&buffer[..count]);
            }
            if data.len() > capacity {
                return Err(too_large());
            }
            let eof = offset.saturating_add(data.len() as u64) >= metadata.len();
            (data, offset, None, eof)
        }
        ReadOptions::Text { offset, limit } => {
            if *offset == 0
                || *limit == 0
                || *limit > PAGE_LINES
                || offset
                    .checked_add(*limit)
                    .is_none_or(|end| end > 9_007_199_254_740_991)
            {
                return Err(too_large());
            }
            let (data, lines, eof) = page(&mut opened.file, *offset, *limit, &authorized)?;
            (data, *offset as u64, Some(lines), eof)
        }
    };
    opened.verify()?;
    if initial_version != version(&opened.file.metadata()?) {
        return Err(invalid(
            SessionFileReadFailure::Replaced,
            "File changed during preview read",
        ));
    }
    authorized()?;
    Ok(PreviewRead {
        version: initial_version,
        bytes: metadata.len(),
        offset,
        eof,
        lines,
        data,
    })
}

fn page(
    file: &mut std::fs::File,
    offset: usize,
    limit: usize,
    authorized: &impl Fn() -> io::Result<()>,
) -> io::Result<(Vec<u8>, usize, bool)> {
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let last = offset + limit - 1;
    let mut line = 1;
    let mut lines = 0;
    let mut pending = false;
    let mut data = Vec::new();
    let mut eof = true;
    loop {
        authorized()?;
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            break;
        }
        if line > last {
            eof = false;
            break;
        }
        let length = buffer
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(buffer.len(), |at| at + 1);
        let terminated = buffer[length - 1] == b'\n';
        if line >= offset {
            let segment = &buffer[..length - usize::from(terminated)];
            // Store separators only between lines, preserving CR and empty lines.
            let separator = usize::from(!pending && lines > 0);
            if data.len() + separator + segment.len() > PAGE_BYTES {
                return Err(too_large());
            }
            if separator != 0 {
                data.push(b'\n');
            }
            data.extend_from_slice(segment);
            pending = true;
            if terminated {
                lines += 1;
                pending = false;
            }
        }
        if terminated {
            line += 1;
        }
        reader.consume(length);
    }
    if pending {
        lines += 1;
    }
    if data.contains(&0) || std::str::from_utf8(&data).is_err() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Preview page is not UTF-8 text",
        ));
    }
    Ok((data, lines, eof))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(bytes: &[u8]) -> (tempfile::TempDir, SessionFileReference) {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("file.txt"), bytes).unwrap();
        let conversation = crate::runtime::identity::ConversationId::new(
            "conv_00000000-0000-7000-8000-000000000001",
        );
        let reference =
            super::super::declare(root.path(), &conversation, "file.txt", None).unwrap();
        (root, reference)
    }
    #[test]
    fn complete_bytes_and_ranges_are_independent_of_rpc_and_file_size() {
        let bytes = vec![b'x'; 600 * 1024];
        let (root, file) = fixture(&bytes);
        let options = ReadOptions::Bytes {
            max_bytes: FILE_BYTES,
            range: None,
        };
        assert_eq!(
            read_authorized(root.path(), &file, &options, || Ok(()))
                .unwrap()
                .data,
            bytes
        );
        std::fs::OpenOptions::new()
            .write(true)
            .open(root.path().join("file.txt"))
            .unwrap()
            .set_len((FILE_BYTES + 1) as u64)
            .unwrap();
        assert!(read_authorized(root.path(), &file, &options, || Ok(())).is_err());
        let window = ReadOptions::Bytes {
            max_bytes: FILE_BYTES,
            range: Some(ByteRange {
                offset: 0,
                length: 8,
            }),
        };
        let read = read_authorized(root.path(), &file, &window, || Ok(())).unwrap();
        assert_eq!(read.data, b"xxxxxxxx");
        assert!(!read.eof);
    }
    #[test]
    fn text_pages_cross_the_old_file_limit_and_preserve_line_boundaries() {
        let bytes = "中文\r\n".repeat(6000);
        let (root, file) = fixture(bytes.as_bytes());
        let first = read_authorized(
            root.path(),
            &file,
            &ReadOptions::Text {
                offset: 1,
                limit: 5000,
            },
            || Ok(()),
        )
        .unwrap();
        let second = read_authorized(
            root.path(),
            &file,
            &ReadOptions::Text {
                offset: 5001,
                limit: 5000,
            },
            || Ok(()),
        )
        .unwrap();
        assert_eq!(first.lines, Some(5000));
        assert!(!first.eof);
        assert_eq!(second.lines, Some(1000));
        assert!(second.eof);
        assert_eq!(first.version, second.version);
        assert_eq!(
            format!(
                "{}\n{}\n",
                String::from_utf8(first.data).unwrap(),
                String::from_utf8(second.data).unwrap()
            ),
            bytes
        );
        std::fs::write(root.path().join("file.txt"), vec![b'x'; 600 * 1024]).unwrap();
        assert_eq!(
            read_authorized(
                root.path(),
                &file,
                &ReadOptions::Text {
                    offset: 1,
                    limit: 5000
                },
                || Ok(())
            )
            .unwrap()
            .data
            .len(),
            600 * 1024
        );
        std::fs::write(root.path().join("file.txt"), vec![b'x'; PAGE_BYTES + 1]).unwrap();
        assert!(
            read_authorized(
                root.path(),
                &file,
                &ReadOptions::Text {
                    offset: 1,
                    limit: 5000
                },
                || Ok(())
            )
            .is_err()
        );
    }
}
