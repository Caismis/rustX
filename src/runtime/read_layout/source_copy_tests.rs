use super::*;
use std::io::Write as _;

fn fixture() -> (SeekableObservationSource, Vec<u8>) {
    #[derive(serde::Serialize, schemars::JsonSchema)]
    struct Native {
        revision: u64,
        opaque: Value,
    }
    let value = Native {
        revision: u64::MAX,
        opaque: serde_json::json!({
            "prefix": "x".repeat(65_525),
            "unicode": "水🙂\\\"\n".repeat(30_000),
            "escaped/~": [{"@0": u64::MAX}, -0.0, 1.000_125, true, null],
        }),
    };
    let mut expected = Vec::new();
    crate::observation::structural_json::to_writer(&mut expected, &value).unwrap();
    assert!(expected.len() > 3 * 65_536);
    (SeekableObservationSource::new(&value).unwrap(), expected)
}

#[derive(Default)]
struct WindowWriter {
    bytes: Vec<u8>,
    writes: usize,
    flushes: usize,
    fail_at: Option<usize>,
}
impl std::io::Write for WindowWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        assert!(!bytes.is_empty());
        assert!(
            bytes.len() <= 65_536,
            "each original read window is bounded"
        );
        self.writes += 1;
        if self.fail_at == Some(self.writes) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "original destination failure",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.flushes += 1;
        Ok(())
    }
}

#[test]
fn original_source_copy_preserves_exact_bytes_and_extent_without_index_reopens() {
    let (source, expected) = fixture();
    let original_end = source.layout.written;
    assert_eq!(original_end, u64::try_from(expected.len()).unwrap());
    let chunks = expected.len().div_ceil(65_536);
    INDEX_OPENS.with(|opens| opens.set(0));
    FILE_WORK.with(|work| work.set((0, 0)));
    let mut old_bytes = Vec::new();
    let mut offset = 0;
    loop {
        let chunk = source.read(&[], offset, 65_536).unwrap();
        assert_eq!(chunk.readable_end, original_end);
        old_bytes.extend_from_slice(&chunk.bytes);
        let Some(next) = chunk.next_offset else {
            break;
        };
        offset = next;
    }
    assert_eq!(old_bytes, expected);
    assert_eq!(INDEX_OPENS.with(std::cell::Cell::get), chunks);
    assert_eq!(FILE_WORK.with(std::cell::Cell::get), (0, chunks));

    INDEX_OPENS.with(|opens| opens.set(0));
    FILE_WORK.with(|work| work.set((0, 0)));
    READ_WORK.with(|work| work.set((0, 0)));
    let mut destination = WindowWriter::default();
    assert_eq!(
        source.copy_original_bytes_to(&mut destination).unwrap(),
        original_end
    );
    assert_eq!(destination.bytes, expected);
    assert_eq!((destination.writes, destination.flushes), (chunks, 0));
    assert_eq!(INDEX_OPENS.with(std::cell::Cell::get), 0);
    assert_eq!(FILE_WORK.with(std::cell::Cell::get), (0, 1));
    assert_eq!(
        READ_WORK.with(std::cell::Cell::get),
        (chunks, expected.len())
    );
    assert_eq!(source.layout.written, original_end);
    assert_eq!(
        source.read(&["revision".into()], 0, 64).unwrap().bytes,
        serde_json::to_vec(&u64::MAX.to_string()).unwrap()
    );
}

#[test]
fn original_source_copy_stops_at_original_extent_and_releases_its_file_owner() {
    let (source, expected) = fixture();
    let directory = source.layout.directory.path().to_owned();
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(directory.join("bytes"))
        .unwrap();
    file.write_all(b"untrusted trailing bytes beyond the original extent")
        .unwrap();
    drop(file);
    let mut destination = WindowWriter::default();
    assert_eq!(
        source.copy_original_bytes_to(&mut destination).unwrap(),
        u64::try_from(expected.len()).unwrap()
    );
    assert_eq!(destination.bytes, expected);
    assert!(directory.exists());
    drop(source);
    assert!(!directory.exists());
    assert_eq!(destination.bytes, expected);
}

#[test]
fn original_source_copy_rejects_premature_eof_without_shortening_the_extent() {
    let (source, expected) = fixture();
    let original_end = source.layout.written;
    std::fs::OpenOptions::new()
        .write(true)
        .open(source.layout.directory.path().join("bytes"))
        .unwrap()
        .set_len(original_end - 1)
        .unwrap();
    let mut destination = WindowWriter::default();
    let error = source.copy_original_bytes_to(&mut destination).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::UnexpectedEof);
    let complete_prefix = (expected.len() - 1) / 65_536 * 65_536;
    assert_eq!(destination.bytes, expected[..complete_prefix]);
    assert_eq!(destination.flushes, 0);
    assert_eq!(source.layout.written, original_end);
    assert!(source.read(&[], original_end - 1, 1).is_err());
}

#[test]
fn original_source_copy_preserves_destination_error_and_stops_without_flushing() {
    let (source, expected) = fixture();
    let mut destination = WindowWriter {
        fail_at: Some(2),
        ..WindowWriter::default()
    };
    INDEX_OPENS.with(|opens| opens.set(0));
    READ_WORK.with(|work| work.set((0, 0)));
    let error = source.copy_original_bytes_to(&mut destination).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    assert_eq!(error.to_string(), "original destination failure");
    assert_eq!((destination.writes, destination.flushes), (2, 0));
    assert_eq!(destination.bytes, expected[..65_536]);
    assert_eq!(INDEX_OPENS.with(std::cell::Cell::get), 0);
    assert_eq!(READ_WORK.with(std::cell::Cell::get), (2, 2 * 65_536));
    let mut recovered = Vec::new();
    assert_eq!(
        source.copy_original_bytes_to(&mut recovered).unwrap(),
        source.layout.written
    );
    assert_eq!(recovered, expected);
}
