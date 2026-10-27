use super::{
    CHECKPOINT_TARGET_BYTES, bounded_bytes, bounded_structural_len,
    bounded_structural_len_with_budget,
};
use crate::runtime_client::{preflight, structural_json};
use serde::Serialize;

fn matches_buffered<T: Serialize + schemars::JsonSchema>(value: &T) {
    let schema = structural_json::root::<T>();
    let encoded = structural_json::Encoded {
        value,
        schema: &schema,
        root: &schema,
    };
    let mut limits = vec![0, 1, 32, 64, 128, 256, 512, 4096];
    if let Ok(bytes) = serde_json::to_vec(&encoded) {
        limits.extend([bytes.len().saturating_sub(1), bytes.len(), bytes.len() + 1]);
    }
    if let Some(required) = preflight::required_budget(&encoded, CHECKPOINT_TARGET_BYTES + 1) {
        limits.extend([required.saturating_sub(1), required, required + 1]);
    }
    limits.extend([
        CHECKPOINT_TARGET_BYTES - 1,
        CHECKPOINT_TARGET_BYTES,
        CHECKPOINT_TARGET_BYTES + 1,
    ]);
    limits.sort_unstable();
    limits.dedup();
    let prepared_budget =
        preflight::required_safe_integer_budget(value, CHECKPOINT_TARGET_BYTES + 1);
    for limit in limits {
        let expected = bounded_bytes(&encoded, limit).map(|bytes| bytes.len());
        assert_eq!(
            bounded_structural_len(value, limit),
            expected,
            "original buffered admission and exact length at {limit} bytes"
        );
        assert_eq!(
            bounded_structural_len_with_budget(value, limit, prepared_budget),
            expected,
            "prepared admission retains buffered validation and length at {limit} bytes"
        );
        assert_eq!(
            preflight::with_structural_admission_only(|| bounded_structural_len(value, limit)),
            expected,
            "schema-aware fallback at {limit} bytes"
        );
    }
}

#[test]
fn structural_length_preserves_native_and_opaque_integer_domains() {
    #[derive(Serialize, schemars::JsonSchema)]
    struct Native {
        sequence: u64,
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        offset: u64,
        signed: i64,
        opaque: serde_json::Value,
        text: String,
    }
    let mut common = Native {
        sequence: 7,
        offset: 0,
        signed: -17,
        opaque: serde_json::json!({"escaped\"\n": [1.25, "水", null, true]}),
        text: "a\"\\\n\u{0}水".repeat(3),
    };
    assert_eq!(
        preflight::bounded_safe_integer_fit(&common, 4096),
        Some(true),
        "ordinary native values use raw admission"
    );
    matches_buffered(&common);
    for sequence in [0, 9_007_199_254_740_991, 9_007_199_254_740_992, u64::MAX] {
        for offset in [0, 9_007_199_254_740_991, u64::MAX] {
            for signed in [-9_007_199_254_740_991, i64::MIN, i64::MAX] {
                common.sequence = sequence;
                common.offset = offset;
                common.signed = signed;
                matches_buffered(&common);
            }
        }
    }
    common.offset = 0;
    common.signed = 0;
    common.opaque = serde_json::json!({"wide": u64::MAX});
    matches_buffered(&common);
}

#[test]
fn structural_length_matches_event_and_escaped_budget_boundaries() {
    use crate::runtime::identity::AttemptId;
    use crate::runtime_client::event::RuntimeClientEvent;

    matches_buffered(&RuntimeClientEvent::TraceChanged);
    for turn in [0, 1, u32::MAX] {
        matches_buffered(&RuntimeClientEvent::AttemptTurnUpdated {
            attempt_id: AttemptId::new("escaped\"\\\n\u{0}水"),
            turn,
        });
    }
    // A string has exact admission, so exercise both sides of the actual
    // publication ceiling without conflating it with conservative number cost.
    for extra in [0, 1, 2] {
        let value = "x".repeat(CHECKPOINT_TARGET_BYTES - 2 + extra);
        assert_eq!(
            bounded_structural_len(&value, CHECKPOINT_TARGET_BYTES),
            (extra == 0).then_some(CHECKPOINT_TARGET_BYTES)
        );
        matches_buffered(&value);
    }
    matches_buffered(&"\n\"\\\t水".repeat(CHECKPOINT_TARGET_BYTES / 10));
}

#[test]
fn structural_length_preserves_uncertain_keys_and_serialization_failures() {
    use std::collections::BTreeMap;

    #[derive(schemars::JsonSchema)]
    struct Invalid;
    impl Serialize for Invalid {
        fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("native value unavailable"))
        }
    }

    let uncertain = BTreeMap::from([(Some(1_u64), "value")]);
    assert_eq!(preflight::bounded_safe_integer_fit(&uncertain, 4096), None);
    matches_buffered(&uncertain);

    let invalid = BTreeMap::from([([1_u8, 2], "value")]);
    assert_eq!(preflight::bounded_safe_integer_fit(&invalid, 4096), None);
    assert!(bounded_structural_len(&invalid, 4096).is_none());
    matches_buffered(&invalid);

    matches_buffered(&Invalid);
}

#[test]
fn prepared_length_preserves_float_bits_and_object_insertion_order() {
    for number in [0.0_f64, -0.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        matches_buffered(&number);
    }
    let mut left = serde_json::Map::new();
    left.insert("first".into(), serde_json::json!(-0.0));
    left.insert("second".into(), serde_json::json!("水\n\""));
    let mut right = serde_json::Map::new();
    right.insert("second".into(), serde_json::json!("水\n\""));
    right.insert("first".into(), serde_json::json!(-0.0));
    matches_buffered(&serde_json::Value::Object(left));
    matches_buffered(&serde_json::Value::Object(right));
}
