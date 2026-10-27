use super::{CHECKPOINT_ITEM_BYTES, bounded_bytes, bounded_len};
use crate::runtime_client::preflight;
use serde::Serialize;

fn matches_buffered<T: Serialize + ?Sized>(value: &T) {
    let mut limits = vec![0, 1, 2, 8, 32, 64, 128, 256, 512, 4096];
    if let Ok(bytes) = serde_json::to_vec(value) {
        limits.extend([bytes.len().saturating_sub(1), bytes.len(), bytes.len() + 1]);
    }
    if let Some(required) = preflight::required_budget(value, CHECKPOINT_ITEM_BYTES + 1) {
        limits.extend([required.saturating_sub(1), required, required + 1]);
    }
    limits.extend([
        CHECKPOINT_ITEM_BYTES - 1,
        CHECKPOINT_ITEM_BYTES,
        CHECKPOINT_ITEM_BYTES + 1,
    ]);
    limits.sort_unstable();
    limits.dedup();
    for limit in limits {
        assert_eq!(
            bounded_len(value, limit),
            bounded_bytes(value, limit).map(|bytes| bytes.len()),
            "native buffered admission and exact length at {limit} bytes"
        );
    }
}

#[test]
fn native_length_matches_exact_and_conservative_budget_boundaries() {
    matches_buffered("");
    matches_buffered(&());
    matches_buffered(&true);
    matches_buffered(&vec![1_u32; 33]);
    for value in [0_u64, 1, 9_007_199_254_740_991, u64::MAX] {
        matches_buffered(&value);
        let bytes = serde_json::to_vec(&value).unwrap();
        assert_eq!(bounded_len(&value, 4096), Some(bytes.len()));
    }
    // Numbers use a conservative preflight budget; the exact encoded size
    // alone must not change admission when only the writer is replaced.
    assert_eq!(bounded_len(&1_u64, 1), None);
    for extra in [0, 1, 2] {
        let value = "x".repeat(CHECKPOINT_ITEM_BYTES - 2 + extra);
        assert_eq!(
            bounded_len(&value, CHECKPOINT_ITEM_BYTES),
            (extra == 0).then_some(CHECKPOINT_ITEM_BYTES)
        );
        matches_buffered(&value);
    }
}

#[test]
fn native_length_counts_escaped_unicode_and_raw_json_bytes() {
    for repeats in [0, 1, 17, 4096] {
        matches_buffered(&"a\"\\\n\t\u{0}\u{8}水".repeat(repeats));
    }
    matches_buffered(&serde_json::json!({
        "escaped\"\n": [1.25, "水", null, true],
        "controls": "\u{0}\u{1}\u{8}\u{b}\u{c}\r\t"
    }));
    let raw =
        serde_json::value::RawValue::from_string("{ \"z\": 2, \"a\": \"水\" }".into()).unwrap();
    matches_buffered(raw.as_ref());
}

#[test]
fn native_length_keeps_opaque_integer_admission() {
    #[derive(Serialize)]
    struct Native {
        sequence: u64,
        opaque: serde_json::Value,
    }
    for value in [
        serde_json::json!(9_007_199_254_740_991_u64),
        serde_json::json!(9_007_199_254_740_992_u64),
        serde_json::json!(u64::MAX),
        serde_json::json!(i64::MIN),
    ] {
        matches_buffered(&value);
        matches_buffered(&Native {
            sequence: u64::MAX,
            opaque: value,
        });
    }
    assert!(bounded_len(&serde_json::json!(u64::MAX), 4096).is_none());
    assert!(
        bounded_len(
            &Native {
                sequence: 0,
                opaque: serde_json::json!({"wide": u64::MAX}),
            },
            4096,
        )
        .is_none()
    );
}

#[test]
fn native_length_keeps_uncertain_keys_and_serializer_failures() {
    use std::collections::BTreeMap;

    struct Invalid;
    impl Serialize for Invalid {
        fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("native value unavailable"))
        }
    }

    let uncertain = BTreeMap::from([(Some(1_u64), "value")]);
    assert_eq!(preflight::bounded_fit(&uncertain, 4096), None);
    matches_buffered(&uncertain);

    // Preflight deliberately admits uncertain key shapes, so the native
    // encoder must still reject this map after counting has started.
    let invalid_key = BTreeMap::from([([1_u8, 2], "value")]);
    assert!(preflight::fits(&invalid_key, 4096));
    assert!(bounded_len(&invalid_key, 4096).is_none());
    matches_buffered(&invalid_key);
    matches_buffered(&Invalid);
}
