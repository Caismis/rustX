//! Independent producer admission/byte oracle for parametric occurrence widths.
use super::{Connection, ConversationStoreError, Template, bounded, fragments, nodes, weights};
use crate::durable::observation::MessageHeader;
use crate::model::ModelInputMessage;
use crate::model::projected::FrozenModelInput;
use crate::observation::trace::{TraceContentBlock, TraceRequestMessage};
use crate::runtime::identity::ConversationId;
fn value(view: &TraceRequestMessage, header: Option<&MessageHeader>) -> serde_json::Value {
    match header {
        Some(header) => super::super::message_value_capture(header, view).unwrap().0,
        None => super::super::bounded_message_descriptor(view, view.blocks.len(), false).unwrap(),
    }
}
fn shape(value: &serde_json::Value) -> (usize, usize, bool) {
    (
        value["blocks"].as_array().unwrap().len(),
        value["content"].as_array().map_or(0, Vec::len),
        value["truncated"].as_bool().unwrap(),
    )
}
pub(in crate::durable::sqlite) fn boundary_length(
    connection: &Connection,
    scope: &ConversationId,
    message: &ModelInputMessage,
    frozen: &FrozenModelInput,
) -> usize {
    let (mut view, header) = super::super::input_view(connection, scope, message, frozen).unwrap();

    let bytes = super::super::provenance::Raw::frozen(frozen, &view.provenance)
        .encoded_bytes()
        .unwrap();
    let mut evaluate = |length, sequence| {
        let TraceContentBlock::Text { text } = view.blocks.first_mut().unwrap() else {
            panic!("one original text block")
        };
        text.text = "x".repeat(length);
        text.truncated = false;
        view.provenance_source = Some(super::super::provenance_source(sequence, 0, bytes));
        value(&view, header.as_ref())
    };
    let (mut low, mut high) = (0, 16385);
    while low + 1 < high {
        let middle = usize::midpoint(low, high);
        if shape(&evaluate(middle, 1)).0 > 0 {
            low = middle;
        } else {
            high = middle;
        }
    }
    for length in low.saturating_sub(64)..=(low + 64).min(16384) {
        if shape(&evaluate(length, 1)) != shape(&evaluate(length, 10)) {
            return length;
        }
    }
    panic!("fixture must exercise the actual one-digit J admission boundary");
}
struct Legacy<'a> {
    value: &'a serde_json::Value,
    view: &'a TraceRequestMessage,
    message: &'a ModelInputMessage,
    frozen: &'a FrozenModelInput,
}
impl serde::Serialize for Legacy<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.message {
            ModelInputMessage::Canonical(_) => serde::Serialize::serialize(
                &super::super::provenance::Canonical {
                    value: self.value,
                    provenance: super::super::provenance::Raw::frozen(
                        self.frozen,
                        &self.view.provenance,
                    ),
                },
                serializer,
            ),
            _ => serde::Serialize::serialize(
                &super::super::input::Input {
                    original: self.message,
                    view: self.view,
                },
                serializer,
            ),
        }
    }
}
impl schemars::JsonSchema for Legacy<'_> {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        <TraceRequestMessage as schemars::JsonSchema>::schema_name()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <TraceRequestMessage as schemars::JsonSchema>::json_schema(generator)
    }
}
fn actual_bytes(connection: &Connection, template: &Template, sequence: u64, rank: u64) -> Vec<u8> {
    let mut bytes =
        fragments::bytes(connection, &template.prefix, 0, template.prefix.length).unwrap();
    bytes.extend(nodes::hole(sequence, rank, template.provenance_bytes).unwrap());
    bytes
        .extend(fragments::bytes(connection, &template.suffix, 0, template.suffix.length).unwrap());
    bytes
}
fn assert_pair(
    connection: &Connection,
    leaf: &super::Leaf,
    view: &mut TraceRequestMessage,
    original: (&ModelInputMessage, Option<&MessageHeader>),
    frozen: &FrozenModelInput,
    sequence: u64,
    rank: u64,
) -> serde_json::Value {
    let provenance_bytes = super::super::provenance::Raw::frozen(frozen, &view.provenance)
        .encoded_bytes()
        .unwrap();
    view.provenance_source = Some(super::super::provenance_source(
        sequence,
        rank,
        provenance_bytes,
    ));
    let expected = value(view, original.1);
    let template = fragments::load_template(
        connection,
        leaf.templates[usize::from(leaf.variants[weights::class(sequence, rank).unwrap()])],
    )
    .unwrap();
    let mut actual_header = template.header_prefix.as_bytes().to_vec();
    actual_header.extend(nodes::hole(sequence, rank, provenance_bytes).unwrap());
    actual_header.extend_from_slice(template.header_suffix.as_bytes());
    assert_eq!(
        actual_header,
        bounded(&expected, 16384).unwrap(),
        "J={sequence}/rank={rank}"
    );
    let source = crate::runtime::read_layout::SeekableObservationSource::new(&Legacy {
        value: &expected,
        view,
        message: original.0,
        frozen,
    })
    .unwrap();
    let mut raw = Vec::new();
    source.copy_original_bytes_to(&mut raw).unwrap();
    assert_eq!(
        actual_bytes(connection, &template, sequence, rank),
        raw,
        "full original bytes J={sequence}/rank={rank}"
    );
    source
        .visit_fields(|path, _, _, start, end, _, _, _| {
            if path == ["provenance_source"] {
                assert_eq!(start, template.prefix.length);
                assert_eq!(
                    end - start,
                    nodes::hole(sequence, rank, provenance_bytes).unwrap().len() as u64
                );
            }
            Ok(())
        })
        .unwrap();
    expected
}
pub(in crate::durable::sqlite) fn assert_variants(
    connection: &Connection,
    scope: &ConversationId,
    message: &ModelInputMessage,
    frozen: &FrozenModelInput,
) {
    let leaf = fragments::capture(connection, scope, message, frozen).unwrap();
    let (mut view, header) = super::super::input_view(connection, scope, message, frozen).unwrap();

    assert_eq!(leaf.variants.len(), 80);
    assert!(
        leaf.templates.len() > 1,
        "actual admission variants required"
    );
    let mut sequences = Vec::new();
    for digits in 0..20 {
        let value = 10_u64.pow(digits);
        sequences.push(value);
        if value > 1 {
            sequences.push(value - 1);
        }
    }
    sequences.push(u64::MAX);
    for sequence in sequences {
        for rank in [0, 9, 10, 99, 100, 999, 1000, 8191] {
            assert_pair(
                connection,
                &leaf,
                &mut view,
                (message, header.as_ref()),
                frozen,
                sequence,
                rank,
            );
        }
    }
    let first = assert_pair(
        connection,
        &leaf,
        &mut view,
        (message, header.as_ref()),
        frozen,
        10,
        0,
    );
    let second = assert_pair(
        connection,
        &leaf,
        &mut view,
        (message, header.as_ref()),
        frozen,
        1,
        10,
    );
    assert_ne!(
        shape(&first),
        shape(&second),
        "same decimal-width sum has different original preflight admission"
    );
    assert!(matches!(
        weights::class(1, 8192),
        Err(ConversationStoreError::Storage(_))
    ));
    assert!(weights::class(0, 0).is_err());
    assert!(weights::class(1, u64::MAX).is_err());
}
