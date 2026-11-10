use super::*;
use std::io::Write as _;

fn raw_source(raw: &[u8], chunks: usize) -> SeekableObservationSource {
    let mut layout = Layout::unindexed();
    for chunk in raw.chunks(chunks) {
        layout.write_all(chunk).unwrap();
    }
    layout
        .initialize_index(raw.len() <= MEMORY_INDEX_BYTES)
        .unwrap();
    layout.bytes.seal();
    let reader = std::io::BufReader::new(layout.open_bytes().unwrap());
    LayoutParser {
        reader,
        position: 0,
        layout: &mut layout,
    }
    .value(None, 0, None, 0)
    .unwrap();
    layout.seal().unwrap();
    SeekableObservationSource { layout }
}

#[test]
fn resident_threshold_and_split_writes_preserve_exact_source_bytes() {
    for length in [4095, 4096, 4097, 16_383, 16_384, 16_385, 65_537] {
        let raw = format!("\"{}\"", "x".repeat(length - 2)).into_bytes();
        for chunk in [1, 4095, 65_536] {
            FILE_WORK.with(|work| work.set((0, 0)));
            let source = raw_source(&raw, chunk);
            assert_eq!(source.layout.written, length as u64);
            assert_eq!(
                source.layout.directory.is_none(),
                length <= MEMORY_INDEX_BYTES
            );
            assert_eq!(
                matches!(source.layout.bytes, byte_carrier::Bytes::Resident(_)),
                length <= byte_carrier::RESIDENT_BYTES
            );
            let reader = source.byte_reader().unwrap();
            let mut actual = Vec::new();
            for start in (0..length).step_by(1024) {
                actual.extend(
                    reader
                        .indexed_span(start as u64, (start + 1024).min(length) as u64, 1024)
                        .unwrap(),
                );
            }
            assert_eq!(actual, raw);
            if length <= byte_carrier::RESIDENT_BYTES {
                assert_eq!(FILE_WORK.with(std::cell::Cell::get), (0, 0));
            }
        }
    }
}

#[test]
fn resident_navigation_copy_and_reader_pins_keep_lexical_json() {
    let raw = br#"{"z":-0.0,"n":1.000e+3,"u":18446744073709551615,"escaped":"\u6c34","children":[true,null,"x"]}"#;
    let cache: ObservationLayoutCache = Arc::new(std::sync::OnceLock::new());
    cache.set(Ok(raw_source(raw, 3))).unwrap();
    let pinned = cache.clone();
    drop(cache);
    let source = pinned.get().unwrap().as_ref().unwrap();
    assert!(source.layout.directory.is_none());
    assert_eq!(
        source.read(&["n".into()], 0, 64).unwrap().bytes,
        b"1.000e+3"
    );
    assert_eq!(source.read(&["z".into()], 0, 64).unwrap().bytes, b"-0.0");
    assert_eq!(
        source.read(&["u".into()], 0, 64).unwrap().bytes,
        b"18446744073709551615"
    );
    assert_eq!(
        source.read(&["escaped".into()], 0, 64).unwrap().bytes,
        br#""\u6c34""#
    );
    let page = source.page(&[], 0, 2).unwrap();
    assert_eq!(page.total, 5);
    assert_eq!(page.fields.len(), 2);
    assert_eq!(page.next_offset, Some(2));
    let mut copied = Vec::new();
    assert_eq!(
        source.copy_original_bytes_to(&mut copied).unwrap(),
        raw.len() as u64
    );
    assert_eq!(copied, raw);
    let a = source.byte_reader().unwrap();
    let b = source.byte_reader().unwrap();
    assert_eq!(a.indexed_span(0, 10, 10).unwrap(), raw[..10]);
    drop(a);
    assert_eq!(b.indexed_span(10, 20, 10).unwrap(), raw[10..20]);
}

#[test]
fn failed_spill_does_not_replace_existing_bytes_or_lose_resident_prefix() {
    let mut directory = Some(tempfile::tempdir().unwrap());
    let path = directory.as_ref().unwrap().path().join("bytes");
    std::fs::write(&path, b"existing owner").unwrap();
    let mut bytes = byte_carrier::Bytes::default();
    assert_eq!(
        bytes
            .write(&vec![b'x'; byte_carrier::RESIDENT_BYTES], &mut directory)
            .unwrap(),
        byte_carrier::RESIDENT_BYTES
    );
    assert!(bytes.write(b"y", &mut directory).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"existing owner");
    assert_eq!(
        std::fs::read_dir(directory.as_ref().unwrap().path())
            .unwrap()
            .count(),
        1
    );
    let reader = bytes.reader(directory.as_ref()).unwrap();
    let mut original = vec![0; byte_carrier::RESIDENT_BYTES];
    reader.read_exact_at(&mut original, 0).unwrap();
    assert_eq!(original, vec![b'x'; byte_carrier::RESIDENT_BYTES]);
}

#[test]
fn sealed_resident_storage_rejects_writes_and_out_of_range_reads() {
    let mut bytes = byte_carrier::Bytes::default();
    let mut directory = None;
    bytes.write(b"original", &mut directory).unwrap();
    bytes.seal();
    assert!(bytes.write(b"replacement", &mut directory).is_err());
    let reader = bytes.reader(None).unwrap();
    assert!(reader.read_exact_at(&mut [0; 2], 7).is_err());
    assert!(reader.read_exact_at(&mut [0; 1], u64::MAX).is_err());
    assert!(directory.is_none());
}
