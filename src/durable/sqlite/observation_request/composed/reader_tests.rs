use super::{Connection, SeekableObservationSource, bytes, fact, geometry, scalar};
use crate::runtime::read_layout::{INDEX_OPENS, take_file_work};
use base64::Engine as _;

#[derive(serde::Serialize, schemars::JsonSchema)]
struct ScalarFixture {
    messages: Vec<u8>,
    native: u64,
    opaque: serde_json::Value,
    prompt: String,
}
type ExpectedField = (Vec<String>, u64, u64, u64, Vec<u8>);
fn baseline_probes(
    source: &SeekableObservationSource,
    original: &[u8],
) -> (usize, Vec<ExpectedField>) {
    let mut span_probes = 0;
    let mut fields = Vec::new();
    source
        .visit_fields(|path, _, _, start, end, children, key, preview| {
            let mut spans = vec![(start, start + 1)];
            if end - start <= 16_384 {
                spans.push((start, end));
            }
            if let Some(span) = key.filter(|(start, end)| end - start <= 256) {
                spans.push(span);
            }
            for (start, end) in spans {
                span_probes += 1;
                let chunk = source
                    .read(&[], start, u32::try_from(end - start).unwrap())
                    .unwrap();
                assert_eq!(chunk.bytes, original_slice(original, start, end));
            }
            fields.push((
                path.to_vec(),
                start,
                end,
                children,
                crate::runtime_client::checkpoint::bounded_bytes(preview, 16_384)
                    .unwrap_or_else(|| b"null".to_vec()),
            ));
            Ok(())
        })
        .unwrap();
    (span_probes, fields)
}
fn original_slice(original: &[u8], start: u64, end: u64) -> &[u8] {
    &original[usize::try_from(start).unwrap()..usize::try_from(end).unwrap()]
}
#[test]
fn t08_t15_t20_scalar_capture_scopes_span_io_without_changing_any_original_field_or_bytes() {
    let fixture = ScalarFixture {
        messages: Vec::new(),
        native: u64::MAX,
        opaque: serde_json::json!({"unsafe":u64::MAX,"escaped":"水🙂\"\\\n",
            "raw":" {\"n\":1.00} ", "@0":{"$key":true},
            "huge".repeat(100):"aliased key", "limit":"x".repeat(16_382)}),
        prompt: "λ🙂".repeat(12_000),
    };
    let mut original = Vec::new();
    crate::runtime_client::structural_json::to_writer(&mut original, &fixture).unwrap();
    let source = SeekableObservationSource::new(&fixture).unwrap();
    INDEX_OPENS.with(|work| work.set(0));
    take_file_work();
    geometry(&source, &["messages".into()]).unwrap();
    source.read(&[], 0, 1).unwrap();
    let (probes, expected) = baseline_probes(&source, &original);
    let chunks = original.len().div_ceil(8192);
    for start in (0..original.len()).step_by(8192) {
        source
            .read(&[], u64::try_from(start).unwrap(), 8192)
            .unwrap();
    }
    assert_eq!(INDEX_OPENS.with(std::cell::Cell::get), probes + chunks + 3);
    assert_eq!(take_file_work(), (0, probes + chunks + 3));
    INDEX_OPENS.with(|work| work.set(0));
    let connection = Connection::open_in_memory().unwrap();
    super::super::create_schema(&connection).unwrap();
    let (fragment, _, _) = scalar(&connection, &source, None).unwrap();
    assert_eq!(INDEX_OPENS.with(std::cell::Cell::get), 3);
    assert_eq!(take_file_work(), (0, 4));
    assert!(!expected.is_empty());
    for (path, start, end, children, preview) in expected {
        let actual = fact(&connection, &fragment, &path).unwrap().unwrap();
        assert_eq!(
            (actual.start, actual.end, actual.children),
            (start, end, children)
        );
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(actual.header)
                .unwrap(),
            preview,
            "original preview at {path:?}",
        );
    }
    let mut offset = 0;
    while offset < fragment.length {
        let through = (offset + 65_536).min(fragment.length);
        assert_eq!(
            bytes(&connection, &fragment, offset, through).unwrap(),
            original_slice(&original, offset, through)
        );
        offset = through;
    }
}
