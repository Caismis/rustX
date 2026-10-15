use super::*;
use serde_json::json;

fn key() -> Key {
    Key {
        position: 1,
        scope: "original".into(),
        digest: [b'0'; 64],
        policy: POLICY,
        header_digest: [0; 32],
    }
}
fn slot(name: &str, fragment: Option<Arc<TrustedFragment>>) -> Slot {
    Slot {
        name: name.into(),
        quoted: serde_json::to_vec(name).unwrap().into(),
        fragment,
    }
}
fn entry(slots: Vec<Slot>) -> Entry {
    Entry {
        key: key(),
        slots,
        provenance_bytes: 2,
        bytes: 0,
        touched: 0,
    }
}
fn fragment(value: &serde_json::Value) -> Arc<TrustedFragment> {
    SeekableObservationSource::new(value)
        .unwrap()
        .capture_fragment(&[], ENTRY_BYTES, ENTRY_NODES)
        .unwrap()
        .unwrap()
}
// Independent original fit/construction, including its first-key allowance.
fn legacy_header(item: &Item<'_>) -> Option<Vec<u8>> {
    let mut bytes = Vec::with_capacity(HEADER_BYTES);
    bytes.push(b'{');
    for (index, slot) in item.entry.slots.iter().enumerate() {
        let value = slot
            .fragment
            .as_ref()
            .map_or(item.dynamic.get().as_bytes(), |fragment| {
                fragment.raw_bytes()
            });
        if bytes.len() + slot.quoted.len() + value.len() + 3 > HEADER_BYTES {
            return None;
        }
        if index != 0 {
            bytes.push(b',');
        }
        bytes.extend_from_slice(&slot.quoted);
        bytes.push(b':');
        bytes.extend_from_slice(value);
    }
    bytes.push(b'}');
    Some(bytes)
}
#[test]
fn t08_t14_t19_t20_fragment_header_length_preserves_exact_original_fit_and_bytes_at_the_boundary() {
    let dynamic = serde_json::value::RawValue::from_string("null".into()).unwrap();
    let fixed = 1
        + serde_json::to_vec("static").unwrap().len()
        + 1
        + 1
        + serde_json::to_vec("provenance_source").unwrap().len()
        + 1
        + dynamic.get().len()
        + 1;
    let capture = FragmentCapture::new(0);
    for extra in [-1_i32, 0, 1] {
        let raw_length =
            usize::try_from(i64::try_from(HEADER_BYTES - fixed).unwrap() + i64::from(extra))
                .unwrap();
        let entry = entry(vec![
            slot("static", Some(fragment(&json!("x".repeat(raw_length - 2))))),
            slot("provenance_source", None),
        ]);
        let item = Item {
            entry: &entry,
            dynamic: serde_json::value::RawValue::from_string("null".into()).unwrap(),
            capture: &capture,
        };
        let expected = legacy_header(&item);
        assert_eq!(item.header_length(), expected.as_ref().map(Vec::len));
        assert_eq!(item.header(), expected);
        if extra == 0 {
            assert_eq!(item.header_length(), Some(HEADER_BYTES));
        }
        if extra > 0 {
            assert!(item.header_length().is_none());
        }
    }
    // A single first property has the original conservative closing allowance.
    let quoted = serde_json::to_vec("only").unwrap().len();
    for extra in [0, 1] {
        let raw_length = HEADER_BYTES - quoted - 4 + extra;
        let entry = entry(vec![slot(
            "only",
            Some(fragment(&json!("x".repeat(raw_length - 2)))),
        )]);
        let item = Item {
            entry: &entry,
            dynamic: serde_json::value::RawValue::from_string("null".into()).unwrap(),
            capture: &capture,
        };
        assert_eq!(item.header(), legacy_header(&item));
        assert_eq!(
            item.header_length(),
            (extra == 0).then_some(HEADER_BYTES - 1)
        );
    }
    let entry = entry(vec![
        slot(
            "static",
            Some(fragment(
                &json!({"escaped":"水\n\"\\","n":u64::MAX,"zero":-0.0,"raw":"{\"n\":1.000}"}),
            )),
        ),
        slot("provenance_source", None),
    ]);
    let item = Item {
        entry: &entry,
        dynamic,
        capture: &capture,
    };
    assert_eq!(item.header(), legacy_header(&item));
}

#[test]
fn t20_fragment_cache_hit_admission_allocates_no_header_before_the_one_captured_output() {
    let key = key();
    let mut cache = Cache::default();
    cache.entries.push(entry(vec![
        slot("static", Some(fragment(&json!({"text":"original"})))),
        slot("provenance_source", None),
    ]));
    let session = cache.session();
    let mut session = session.borrow_mut();
    HEADER_ALLOCATIONS.with(|work| work.set(0));
    let item = session.item(&key, 123, 7).unwrap().unwrap();
    assert_eq!(
        HEADER_ALLOCATIONS.with(std::cell::Cell::get),
        0,
        "fit admission only measures the bounded immutable values"
    );
    let captured = item.header().unwrap();
    assert_eq!(
        HEADER_ALLOCATIONS.with(std::cell::Cell::get),
        1,
        "only the header passed to the existing producer is allocated"
    );
    assert_eq!(captured, legacy_header(&item).unwrap());
    assert_eq!(Some(captured.len()), item.header_length());
}
