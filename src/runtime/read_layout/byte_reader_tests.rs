use super::{FILE_WORK, INDEX_OPENS, ObservationReadError, READ_WORK, SeekableObservationSource};
use serde_json::Value;

#[derive(serde::Serialize, schemars::JsonSchema)]
struct Fixture {
    revision: u64,
    opaque: Value,
    text: String,
}
fn source() -> (SeekableObservationSource, Vec<u8>) {
    let fixture = Fixture {
        revision: u64::MAX,
        opaque: serde_json::json!({
            "escaped": "\"\\\n\t水🙂", "unsafe": u64::MAX,
            "raw": " {\"wide\":18446744073709551615} ",
            "@0": {"$key": -0.0}, "long-key".repeat(100): [null, true, -1.25],
            "at_limit": "x".repeat(16_382), "over_limit": "x".repeat(16_383),
        }),
        text: "水🙂".repeat(12_000),
    };
    let mut original = Vec::new();
    crate::observation::structural_json::to_writer(&mut original, &fixture).unwrap();
    (SeekableObservationSource::new(&fixture).unwrap(), original)
}
fn probes(source: &SeekableObservationSource, length: u64) -> Vec<(u64, u64)> {
    let mut probes = Vec::new();
    source
        .visit_fields(|_, _, _, start, end, _, key, _| {
            probes.push((start, start + 1));
            if end - start <= 16_384 {
                probes.push((start, end));
            }
            if let Some((start, end)) = key.filter(|(start, end)| end - start <= 256) {
                probes.push((start, end));
            }
            Ok(())
        })
        .unwrap();
    for start in (0..length).step_by(8192) {
        probes.push((start, (start + 8192).min(length)));
    }
    probes
}
fn original_slice(original: &[u8], start: u64, end: u64) -> &[u8] {
    &original[usize::try_from(start).unwrap()..usize::try_from(end).unwrap()]
}
#[test]
fn t08_t15_t20_scoped_byte_reader_preserves_every_probe_with_one_file_and_no_index_opens() {
    let (source, original) = source();
    let probes = probes(&source, u64::try_from(original.len()).unwrap());
    INDEX_OPENS.with(|work| work.set(0));
    FILE_WORK.with(|work| work.set((0, 0)));
    let mut total_bytes = 0;
    for &(start, end) in &probes {
        let count = u32::try_from(end - start).unwrap();
        let bytes = source.read(&[], start, count).unwrap().bytes;
        assert_eq!(bytes, original_slice(&original, start, end));
        assert_eq!(READ_WORK.with(std::cell::Cell::get), (1, bytes.len()));
        total_bytes += bytes.len();
    }
    assert_eq!(INDEX_OPENS.with(std::cell::Cell::get), probes.len());
    assert_eq!(FILE_WORK.with(std::cell::Cell::get), (0, probes.len()));
    INDEX_OPENS.with(|work| work.set(0));
    FILE_WORK.with(|work| work.set((0, 0)));
    READ_WORK.with(|work| work.set((0, 0)));
    let reader = source.byte_reader().unwrap();
    for &(start, end) in &probes {
        let bytes = reader
            .indexed_span(start, end, u32::try_from(end - start).unwrap())
            .unwrap();
        assert_eq!(bytes, original_slice(&original, start, end));
    }
    assert_eq!(INDEX_OPENS.with(std::cell::Cell::get), 0);
    assert_eq!(FILE_WORK.with(std::cell::Cell::get), (0, 1));
    assert_eq!(
        READ_WORK.with(std::cell::Cell::get),
        (probes.len(), total_bytes)
    );
}
#[test]
fn t08_t12_t20_scoped_byte_reader_checks_bounds_and_keeps_physical_errors_and_source_lifetime() {
    let (source, original) = source();
    let directory = source.layout.directory.path().to_owned();
    let reader = source.byte_reader().unwrap();
    READ_WORK.with(|work| work.set((0, 0)));
    let length = u64::try_from(original.len()).unwrap();
    for (start, end, max) in [
        (2, 1, 64),
        (0, 1, 0),
        (0, 1, 65_537),
        (0, 65_537, 65_536),
        (length, length + 1, 1),
    ] {
        assert!(matches!(
            reader.indexed_span(start, end, max),
            Err(ObservationReadError::InvalidRequest(_))
        ));
    }
    assert_eq!(READ_WORK.with(std::cell::Cell::get), (0, 0));
    assert_eq!(
        reader.indexed_span(0, 65_536, 65_536).unwrap(),
        original[..65_536]
    );
    assert!(directory.exists());
    std::fs::OpenOptions::new()
        .write(true)
        .open(directory.join("bytes"))
        .unwrap()
        .set_len(0)
        .unwrap();
    assert!(matches!(
        reader.indexed_span(0, 1, 1),
        Err(ObservationReadError::Unavailable(_))
    ));
    drop(reader);
    drop(source);
    assert!(
        !directory.exists(),
        "no reader or scratch owner escaped the scope"
    );
}
