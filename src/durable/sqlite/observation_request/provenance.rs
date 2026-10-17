//! Complete allowlisted original provenance bytes; finite previews are separate.
use crate::model::projected::{CanonicalMessageRef, DerivedModelMessage, FrozenModelInput};
use crate::runtime_client::trace::TraceInputProvenance;
use serde::Serialize;

pub(in crate::durable::sqlite) enum Raw<'a> {
    Original(&'a CanonicalMessageRef, &'a TraceInputProvenance),
    Derived(&'a DerivedModelMessage),
    RequestOnly,
}
impl<'a> Raw<'a> {
    pub(in crate::durable::sqlite) fn frozen(
        original: &'a FrozenModelInput,
        view: &'a TraceInputProvenance,
    ) -> Self {
        match original {
            FrozenModelInput::Original(value) => Self::Original(value, view),
            FrozenModelInput::Derived(value) => Self::Derived(value),
            FrozenModelInput::RequestOnly(_) => Self::RequestOnly,
        }
    }
    pub(in crate::durable::sqlite) fn encoded_bytes(
        &self,
    ) -> Result<u64, super::ConversationStoreError> {
        let root = crate::runtime_client::structural_json::root::<TraceInputProvenance>();
        super::super::observation_request_summary::serialized_bytes(
            &crate::runtime_client::structural_json::Encoded {
                value: self,
                schema: &root,
                root: &root,
            },
        )
    }
}
impl schemars::JsonSchema for Raw<'_> {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        <TraceInputProvenance as schemars::JsonSchema>::schema_name()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <TraceInputProvenance as schemars::JsonSchema>::json_schema(generator)
    }
}
impl Serialize for Raw<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::{Error as _, SerializeStruct as _};
        match self {
            Self::Original(original, view) => {
                let TraceInputProvenance::Original {
                    locator,
                    message_id_source,
                    ..
                } = view
                else {
                    return Err(S::Error::custom("original provenance display kind differs"));
                };
                let mut value = serializer.serialize_struct("TraceInputProvenance", 6)?;
                value.serialize_field("type", "original")?;
                value.serialize_field("source", &Some(original))?;
                value.serialize_field("conversation_id", &original.conversation_id)?;
                value.serialize_field("content_digest", &original.content_digest)?;
                value.serialize_field("locator", locator)?;
                value.serialize_field("message_id_source", message_id_source)?;
                value.end()
            }
            Self::Derived(original) => {
                let mut value = serializer.serialize_struct("TraceInputProvenance", 5)?;
                value.serialize_field("type", "derived")?;
                value.serialize_field("sources", &original.sources)?;
                value.serialize_field("checkpoint_sources", &original.checkpoint_sources)?;
                value.serialize_field("transformations", &original.transformations)?;
                value.serialize_field("truncated", &false)?;
                value.end()
            }
            Self::RequestOnly => {
                let mut value = serializer.serialize_struct("TraceInputProvenance", 1)?;
                value.serialize_field("type", "request_only")?;
                value.end()
            }
        }
    }
}
pub(super) struct Canonical<'a> {
    pub(super) value: &'a serde_json::Value,
    pub(super) provenance: Raw<'a>,
}
impl Serialize for Canonical<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::{Error as _, SerializeMap as _};
        let value = self
            .value
            .as_object()
            .ok_or_else(|| S::Error::custom("canonical request descriptor is not object"))?;
        let mut map = serializer.serialize_map(Some(value.len()))?;
        for (key, value) in value {
            if key == "provenance" {
                map.serialize_entry(key, &self.provenance)?;
            } else {
                map.serialize_entry(key, value)?;
            }
        }
        map.end()
    }
}
