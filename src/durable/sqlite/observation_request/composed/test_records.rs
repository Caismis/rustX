//! Typed test bridges let existing coherent-corruption fixtures change geometry
//! and recompute exact stored hashes without assuming a textual storage codec.
use super::{Augmentation, Root, Template, record};

pub(in crate::durable::sqlite) fn inspect_node_record(bytes: &[u8]) -> serde_json::Value {
    serde_json::to_value(record::decode::<Augmentation>(bytes).unwrap()).unwrap()
}
pub(in crate::durable::sqlite) fn encode_test_node_record(value: serde_json::Value) -> Vec<u8> {
    record::encode(
        &serde_json::from_value::<Augmentation>(value).unwrap(),
        65536,
    )
    .unwrap()
}
pub(in crate::durable::sqlite) fn inspect_root_record(bytes: &[u8]) -> serde_json::Value {
    serde_json::to_value(record::decode::<Root>(bytes).unwrap()).unwrap()
}
pub(in crate::durable::sqlite) fn encode_test_root_record(value: serde_json::Value) -> Vec<u8> {
    record::encode(&serde_json::from_value::<Root>(value).unwrap(), 16384).unwrap()
}
pub(in crate::durable::sqlite) fn inspect_template_record(bytes: &[u8]) -> serde_json::Value {
    serde_json::to_value(record::decode::<Template>(bytes).unwrap()).unwrap()
}
pub(in crate::durable::sqlite) fn encode_test_template_record(value: serde_json::Value) -> Vec<u8> {
    record::encode(&serde_json::from_value::<Template>(value).unwrap(), 65536).unwrap()
}
