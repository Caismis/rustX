use super::*;

fn bytes(value: &Overlay) -> Vec<u8> {
    let mut bytes = Vec::new();
    value
        .serialize(
            &mut rmp_serde::Serializer::new(&mut bytes)
                .with_struct_tuple()
                .with_bytes(rmp_serde::config::BytesMode::ForceAll),
        )
        .unwrap();
    bytes
}
fn record(bytes: &[u8]) -> Overlay {
    let mut decoder = rmp_serde::Deserializer::new(std::io::Cursor::new(bytes));
    Overlay::deserialize(&mut decoder).unwrap()
}

#[test]
fn quote_overlay_authenticates_every_byte_and_retains_exact_scalar_distinctions() {
    let native = br#"{"sequence":18446744073709551615,"opaque":{"number":18446744073709551615,"text":"\"123\""},"duration":7}"#;
    let structural = br#"{"sequence":"18446744073709551615","opaque":{"number":18446744073709551615,"text":"\"123\""},"duration":"7"}"#;
    let encoded = encode(native, structural).unwrap().unwrap();
    assert_eq!(restore(native, &encoded).unwrap(), structural);
    assert_eq!(record(&encoded).quotes.len(), 4);
    for index in 0..native.len() {
        let mut corrupt = native.to_vec();
        corrupt[index] ^= 1;
        assert!(restore(&corrupt, &encoded).is_err(), "native byte {index}");
    }
    for index in 0..encoded.len() {
        let mut corrupt = encoded.clone();
        corrupt[index] ^= 1;
        assert!(restore(native, &corrupt).is_err(), "overlay byte {index}");
    }
}

#[test]
fn quote_overlay_rejects_noncanonical_positions_lengths_trailing_and_hostile_counts() {
    let native = br#"{"n":1234}"#;
    let structural = br#"{"n":"1234"}"#;
    let encoded = encode(native, structural).unwrap().unwrap();
    for quotes in [vec![5, 5], vec![9, 5], vec![5, 11], vec![6, 9]] {
        let mut overlay = record(&encoded);
        overlay.quotes = quotes;
        assert!(restore(native, &bytes(&overlay)).is_err());
    }
    let mut overlay = record(&encoded);
    overlay.native_length += 1;
    assert!(restore(native, &bytes(&overlay)).is_err());
    let mut overlay = record(&encoded);
    overlay.source_length -= 1;
    assert!(restore(native, &bytes(&overlay)).is_err());
    let mut trailing = encoded.clone();
    trailing.push(0xc0);
    assert!(restore(native, &trailing).is_err());
    // Keep the actual valid prefix, replace its final coordinates with an array
    // declaring u32::MAX items but providing none. No allocation follows that hint.
    let mut hostile = encoded[..encoded.len() - 3].to_vec();
    hostile.extend_from_slice(&[0xdd, 0xff, 0xff, 0xff, 0xff]);
    assert!(restore(native, &hostile).is_err());
    assert!(restore(native, br#"{"native_length":10}"#).is_err());
}

#[test]
fn quote_overlay_storage_choice_preserves_exact_byte_boundary_and_rejects_other_edits() {
    for length in [1, MAX_BYTES - 1, MAX_BYTES, MAX_BYTES + 1] {
        let native = vec![b'1'; length];
        let encoded = encode(&native, &native).unwrap();
        assert_eq!(encoded.is_some(), length <= MAX_BYTES);
        if let Some(encoded) = encoded {
            assert_eq!(restore(&native, &encoded).unwrap(), native);
        }
    }
    let native = vec![b'1'; MAX_BYTES - 2];
    let mut source = vec![b'"'];
    source.extend_from_slice(&native);
    source.push(b'"');
    let encoded = encode(&native, &source).unwrap().unwrap();
    assert_eq!(restore(&native, &encoded).unwrap(), source);
    source.push(b' ');
    assert!(encode(&native, &source).unwrap().is_none());
    for (native, source) in [
        (b"1234".as_slice(), b"1235".as_slice()),
        (b"1234", b"123"),
        (b"1234", b" 1234"),
        (b"1234", b"1234 "),
    ] {
        assert!(encode(native, source).unwrap().is_none());
    }
}
