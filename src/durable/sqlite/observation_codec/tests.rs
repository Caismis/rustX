use super::super::{observation_event, observation_members, observation_trace};
use super::*;
use crate::durable::observation::{
    AttemptMemberCounts, ObservationEventWitness, TraceFactData, TraceFactHeader, TraceFactKind,
    TraceTerminalClass, TraceTerminalHeader,
};
use crate::runtime::identity::{AttemptId, ConversationId, EventId, MessageId, TurnId};
use crate::runtime_client::checkpoint::bounded_bytes;

fn witness() -> ObservationEventWitness {
    ObservationEventWitness {
        occupancy: None,
        native_direct: false,
        trace_total: Some(u64::MAX),
        source_unavailable: false,
        sequence: u64::MAX,
        transcript_cursor: None,
        event_id: Some(EventId::new("event-水")),
        attempt_id: Some(AttemptId::new("attempt")),
        turn_id: Some(TurnId::new("turn")),
        timestamp: chrono::DateTime::from_timestamp(1_700_000_000, 123_456_789).unwrap(),
        event_kind: "attempt_started".into(),
        tool_occurrence: None,
        result_ledger_position: Some(u64::MAX),
        value: serde_json::from_str(
            r#"{"z":-0.0,"a":[9007199254740991,-9007199254740991,3.25,"水\n\""]}"#,
        )
        .unwrap(),
        fields: Vec::new(),
        deferred: false,
    }
}

fn trace() -> TraceFactHeader {
    TraceFactHeader {
        sequence: u64::MAX,
        timestamp: witness().timestamp,
        kind: TraceFactKind::ModelRequestCompleted,
        owner_anchor: Some(u64::MAX),
        location: crate::runtime_client::trace::TraceLocation::default(),
        fields: Vec::new(),
        fact: TraceFactData::Terminal(TraceTerminalHeader {
            class: TraceTerminalClass::Completed,
            tool_outcome: None,
            detail: None,
            failure_kind: None,
            usage: None,
            generation: Some(crate::model::generation_evidence::GenerationEvidence {
                dispatch_after_start_ms: None,
                first_output_ms: Some(1),
                last_output_ms: None,
                terminal_ms: u64::MAX,
            }),
        }),
        bootstrap: None,
        canonical_fields_ready: Some(false),
    }
}

fn members() -> AttemptMemberCounts {
    use crate::runtime_client::content::{
        CanonicalContentComponent, CanonicalContentLocator, ContentRef, ContentRepresentation,
        ContentSource,
    };
    AttemptMemberCounts {
        message_count: u64::MAX,
        tool_call_count: 2,
        first_cursor: Some(crate::durable::TranscriptCursor::new(1)),
        final_message_id: Some(MessageId::new("final")),
        final_message_locator: Some(CanonicalContentLocator::ByLedgerPosition {
            position: u64::MAX,
        }),
        final_message_id_deferred: true,
        final_message_id_source: Some(ContentRef {
            conversation_id: ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18"),
            source: ContentSource::CanonicalMessageBlock {
                component: CanonicalContentComponent::MessageMetadata,
                locator: CanonicalContentLocator::ByMessageId {
                    message_id: MessageId::new("final"),
                },
                block_index: crate::message::types::ContentBlockIndex::new(0),
            },
            representation: ContentRepresentation::RawJsonUtf8,
            version: 1,
            content_revision: 1,
            readable_end: u64::MAX,
        }),
        completed_response: true,
        process: None,
    }
}

#[test]
fn compact_observation_records_preserve_public_bytes_tags_options_and_opaque_numbers() {
    let original = witness();
    let bytes = observation_event::encode_witness(&original).unwrap();
    let restored = observation_event::decode_witness(&bytes, false, false, None).unwrap();
    assert_eq!(
        serde_json::to_vec(&restored).unwrap(),
        serde_json::to_vec(&original).unwrap()
    );
    assert!(bytes.len() < serde_json::to_vec(&original).unwrap().len());
    let original = trace();
    let bytes = observation_trace::encode_fact(&original).unwrap();
    assert_eq!(
        serde_json::to_vec(&observation_trace::decode_fact(&bytes).unwrap()).unwrap(),
        serde_json::to_vec(&original).unwrap()
    );
    assert!(bytes.len() < serde_json::to_vec(&original).unwrap().len());
    let original = members();
    let bytes = observation_members::encode(&original).unwrap();
    assert_eq!(
        serde_json::to_vec(&observation_members::decode(&bytes).unwrap()).unwrap(),
        serde_json::to_vec(&original).unwrap()
    );
    assert!(bytes.len() < serde_json::to_vec(&original).unwrap().len());
}

#[test]
fn compact_observation_borrowed_decoder_rejects_every_trailing_marker_and_truncated_suffix() {
    fn check<T: Serialize>(
        original: &T,
        bytes: &[u8],
        read: impl Fn(&[u8]) -> Result<T, ConversationStoreError>,
    ) {
        assert_eq!(
            serde_json::to_vec(&read(bytes).unwrap()).unwrap(),
            serde_json::to_vec(original).unwrap()
        );
        for marker in u8::MIN..=u8::MAX {
            let mut damaged = bytes.to_vec();
            damaged.push(marker);
            assert!(read(&damaged).is_err(), "trailing marker {marker:#04x}");
        }
        for (marker, payload_bytes) in [
            (0xcc, 1),
            (0xcd, 2),
            (0xce, 4),
            (0xcf, 8),
            (0xd0, 1),
            (0xd1, 2),
            (0xd2, 4),
            (0xd3, 8),
            (0xca, 4),
            (0xcb, 8),
        ] {
            for present in 0..payload_bytes {
                let mut damaged = bytes.to_vec();
                damaged.push(marker);
                damaged.extend(std::iter::repeat_n(0, present));
                assert!(
                    read(&damaged).is_err(),
                    "truncated numeric {marker:#04x}/{present}"
                );
            }
        }
        for suffix in [
            &[0x91, 0x91][..],
            &[0x81, 0xa1, b'x'],
            &[0xdc, 0],
            &[0xdc, 0, 2, 0xc0],
            &[0xdf, 0, 0, 0, 1, 0xa1, b'x'],
            &[0xdb, 0, 0, 0, 2, b'x'],
            &[0xc7, 1, 0],
        ] {
            let mut damaged = bytes.to_vec();
            damaged.extend_from_slice(suffix);
            assert!(read(&damaged).is_err(), "truncated suffix {suffix:?}");
        }
    }
    let original = witness();
    check(
        &original,
        &observation_event::encode_witness(&original).unwrap(),
        |bytes| observation_event::decode_witness(bytes, false, false, None),
    );
    let original = trace();
    check(
        &original,
        &observation_trace::encode_fact(&original).unwrap(),
        observation_trace::decode_fact,
    );
    let original = members();
    check(
        &original,
        &observation_members::encode(&original).unwrap(),
        observation_members::decode,
    );
}

#[test]
fn compact_observation_records_reject_old_json_wrong_arity_trailing_and_expanded_oversize() {
    let witness = witness();
    let trace = trace();
    let members = members();
    assert!(
        observation_event::decode_witness(
            &serde_json::to_vec(&witness).unwrap(),
            false,
            false,
            None
        )
        .is_err()
    );
    assert!(observation_trace::decode_fact(&serde_json::to_vec(&trace).unwrap()).is_err());
    assert!(observation_members::decode(&serde_json::to_vec(&members).unwrap()).is_err());
    for bytes in [vec![0x90], vec![0x91, 0xc0]] {
        assert!(observation_event::decode_witness(&bytes, false, false, None).is_err());
        assert!(observation_trace::decode_fact(&bytes).is_err());
        assert!(observation_members::decode(&bytes).is_err());
    }
    let mut bytes = observation_event::encode_witness(&witness).unwrap();
    bytes.push(0xc0);
    assert!(observation_event::decode_witness(&bytes, false, false, None).is_err());
    let mut expanded = witness.clone();
    expanded.event_kind = "q".repeat(4000);
    let mut bytes = observation_event::encode_witness(&expanded).unwrap();
    let offset = bytes
        .windows(4000)
        .position(|part| part.iter().all(|byte| *byte == b'q'))
        .unwrap();
    bytes[offset..offset + 4000].fill(0);
    assert!(observation_event::decode_witness(&bytes, false, false, None).is_err());
    let bytes = encode_admitted(&(vec!["\u{0000}"; 4000],)).unwrap();
    let decoded: (Vec<String>,) = decode(&bytes).unwrap();
    assert!(bytes.len() < 16_384);
    assert!(public_length(&decoded, 16_384).is_err());
}

#[test]
fn compact_observation_prescan_rejects_hostile_lengths_depth_and_non_json_types_before_typed_decode()
 {
    thread_local! { static TYPED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
    struct Probe;
    impl<'de> Deserialize<'de> for Probe {
        fn deserialize<D: de::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
            TYPED.with(|calls| calls.set(calls.get() + 1));
            de::IgnoredAny::deserialize(decoder).map(|_| Self)
        }
    }
    let mut depth = vec![0x91; 130];
    depth.push(0xc0);
    for bytes in [
        vec![0x91, 0xdd, 0xff, 0xff, 0xff, 0xff],
        vec![0x91, 0xdf, 0xff, 0xff, 0xff, 0xff],
        vec![0x91, 0xdb, 0xff, 0xff, 0xff, 0xff],
        vec![0x91, 0xc4, 0x01, 0x41],
        vec![0x91, 0xd4, 0, 0],
        vec![0x91, 0x81, 0x01, 0xc0],
        vec![0x91, 0x81, 0xc4, 0x01, b'x', 0xc0],
        depth,
    ] {
        assert!(decode::<Probe>(&bytes).is_err());
        assert_eq!(TYPED.with(std::cell::Cell::get), 0);
    }
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let bytes = encode_admitted(&(value,)).unwrap();
        assert!(decode::<Probe>(&bytes).is_err());
        assert_eq!(TYPED.with(std::cell::Cell::get), 0);
    }
    let mut bytes = observation_event::encode_witness(&witness()).unwrap();
    let needle = [vec![0xcb], 3.25_f64.to_be_bytes().to_vec()].concat();
    let offset = bytes
        .windows(needle.len())
        .position(|part| part == needle)
        .unwrap();
    bytes[offset + 1..offset + 9].copy_from_slice(&f64::NAN.to_be_bytes());
    assert!(observation_event::decode_witness(&bytes, false, false, None).is_err());
}

#[test]
fn compact_observation_writers_keep_original_json_admission_at_exact_boundaries() {
    let mut unsafe_opaque = witness();
    unsafe_opaque.value = serde_json::json!([u64::MAX, i64::MIN]);
    assert!(bounded_bytes(&unsafe_opaque, 16_256).is_none());
    assert!(observation_event::encode_witness(&unsafe_opaque).is_err());
    for count in [0, 1, 600, 650, 680] {
        let mut witness = witness();
        witness.value = serde_json::json!(vec![1.0_f64; count]);
        assert_eq!(
            observation_event::encode_witness(&witness).is_ok(),
            bounded_bytes(&witness, 16_256).is_some()
        );
    }
    let floats = (vec![1.0_f64; 600],);
    let json = bounded_bytes(&floats, 16_384).unwrap();
    let packed = encode(&floats, &floats, 16_384).unwrap();
    assert!(
        packed.len() > json.len(),
        "binary floats can grow despite valid original admission"
    );
    assert_eq!(
        serde_json::to_vec(&decode::<(Vec<f64>,)>(&packed).unwrap()).unwrap(),
        json
    );
    for count in [3900, 4000] {
        let floats = (vec![1.0_f64; count],);
        let packed = encode_admitted(&floats).unwrap();
        assert!(packed.len() > 32_768);
        assert!(public_length(&floats, 16_384).is_ok());
        assert_eq!(
            serde_json::to_vec(&decode::<(Vec<f64>,)>(&packed).unwrap()).unwrap(),
            serde_json::to_vec(&floats).unwrap()
        );
        assert!(
            encode(&floats, &floats, 16_384).is_err(),
            "actual-byte read bound does not broaden original writer admission"
        );
    }
    let mut witness = witness();
    let mut lower = 0;
    let mut upper = 20_000;
    while lower + 1 < upper {
        let middle = usize::midpoint(lower, upper);
        witness.event_kind = "x".repeat(middle);
        if bounded_bytes(&witness, 16_256).is_some() {
            lower = middle;
        } else {
            upper = middle;
        }
    }
    for size in [lower, upper] {
        witness.event_kind = "x".repeat(size);
        assert_eq!(
            observation_event::encode_witness(&witness).is_ok(),
            size == lower
        );
    }
    let mut members = members();
    let mut lower = 0;
    let mut upper = 20_000;
    while lower + 1 < upper {
        let middle = usize::midpoint(lower, upper);
        members.final_message_id = Some(MessageId::new("x".repeat(middle)));
        if bounded_bytes(&members, 16_384).is_some() {
            lower = middle;
        } else {
            upper = middle;
        }
    }
    for size in [lower, upper] {
        members.final_message_id = Some(MessageId::new("x".repeat(size)));
        assert_eq!(observation_members::encode(&members).is_ok(), size == lower);
    }
}

#[test]
fn compact_witness_transcript_patch_preserves_native_terminal_when_derived_update_fails() {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    connection.execute_batch("CREATE TABLE events(sequence INTEGER PRIMARY KEY,event_json TEXT NOT NULL,observation_witness BLOB); INSERT INTO events VALUES(1,'native-terminal',NULL);").unwrap();
    let bytes = observation_event::encode_witness(&witness()).unwrap();
    connection
        .execute("UPDATE events SET observation_witness=?1", [&bytes])
        .unwrap();
    let cursor = crate::durable::TranscriptCursor::new(u64::MAX);
    observation_event::attach_transcript(&connection, 1, cursor).unwrap();
    let patched: Vec<u8> = connection
        .query_row("SELECT observation_witness FROM events", [], |row| {
            row.get(0)
        })
        .unwrap();
    let mut expected = witness();
    expected.transcript_cursor = Some(cursor);
    assert_eq!(
        serde_json::to_vec(
            &observation_event::decode_witness(&patched, false, false, None).unwrap()
        )
        .unwrap(),
        serde_json::to_vec(&expected).unwrap()
    );
    connection.execute_batch("CREATE TEMP TRIGGER refuse_witness BEFORE UPDATE OF observation_witness ON events BEGIN SELECT RAISE(FAIL,'derived only'); END;").unwrap();
    assert!(
        observation_event::attach_transcript(
            &connection,
            1,
            crate::durable::TranscriptCursor::new(2)
        )
        .is_err()
    );
    let row: (String, Vec<u8>) = connection
        .query_row(
            "SELECT event_json,observation_witness FROM events",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(row, ("native-terminal".into(), patched));
    connection
        .execute_batch("DROP TRIGGER refuse_witness; UPDATE events SET observation_witness=x'7b';")
        .unwrap();
    assert!(observation_event::attach_transcript(&connection, 1, cursor).is_err());
    assert_eq!(
        connection
            .query_row("SELECT event_json FROM events", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "native-terminal"
    );
}

#[test]
fn compact_observation_write_certificate_skips_json_rewalk_but_checks_uncertain_keys() {
    struct Counted(std::cell::Cell<usize>);
    impl Serialize for Counted {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            self.0.set(self.0.get() + 1);
            serializer.serialize_str("known public header")
        }
    }
    let public = Counted(std::cell::Cell::new(0));
    encode(&public, &("private record",), 128).unwrap();
    assert_eq!(
        public.0.get(),
        1,
        "one conservative public walk, no discarded JSON encoding"
    );
    let uncertain = std::collections::BTreeMap::from([(Some("key"), 3_u64)]);
    assert_eq!(
        crate::runtime_client::preflight::bounded_fit(&uncertain, 128),
        None
    );
    let bytes = encode(&uncertain, &(&uncertain,), 128).unwrap();
    let (restored,): (std::collections::BTreeMap<Option<String>, u64>,) = decode(&bytes).unwrap();
    assert_eq!(
        serde_json::to_vec(&restored).unwrap(),
        bounded_bytes(&uncertain, 128).unwrap()
    );
    let invalid = std::collections::BTreeMap::from([((1_u8, 2_u8), 3_u64)]);
    assert_eq!(
        crate::runtime_client::preflight::bounded_fit(&invalid, 128),
        None
    );
    assert!(bounded_bytes(&invalid, 128).is_none());
    assert!(encode(&invalid, &(&invalid,), 128).is_err());
}

#[test]
fn compact_observation_depth_boundary_matches_original_json_reader() {
    for depth in [126, 127, 128] {
        let mut nested = serde_json::Value::Null;
        for _ in 0..depth {
            nested = serde_json::Value::Array(vec![nested]);
        }
        let original = (nested,);
        let json = serde_json::to_vec(&original).unwrap();
        let bytes = encode_admitted(&original).unwrap();
        let old = serde_json::from_slice::<(serde_json::Value,)>(&json);
        let new = decode::<(serde_json::Value,)>(&bytes);
        assert_eq!(new.is_ok(), old.is_ok(), "depth {depth}");
        if let Ok(restored) = new {
            assert_eq!(serde_json::to_vec(&restored).unwrap(), json);
        }
    }
}

#[test]
fn compact_observation_read_certificate_visits_normal_public_values_once() {
    struct Counted<'a, T> {
        value: &'a T,
        calls: std::cell::Cell<usize>,
    }
    impl<T: Serialize> Serialize for Counted<'_, T> {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            self.calls.set(self.calls.get() + 1);
            self.value.serialize(serializer)
        }
    }
    fn check(value: &impl Serialize) {
        let limit = crate::durable::observation::HEADER_ENCODED_MAX;
        assert_eq!(
            crate::runtime_client::preflight::bounded_fit(value, limit),
            Some(true)
        );
        assert!(serde_json::to_vec(value).unwrap().len() <= limit);
        let counted = Counted {
            value,
            calls: std::cell::Cell::new(0),
        };
        validate_public(&counted, limit).unwrap();
        assert_eq!(counted.calls.get(), 1, "no subsequent JSON encoding walk");
    }
    check(&witness());
    check(&trace());
    check(&members());
}

// A valid stored cell need not pass today's conservative writer admission.
// Construct the closed private record directly to exercise that reader boundary.
fn admitted_witness(value: &ObservationEventWitness) -> Vec<u8> {
    encode_admitted(&(
        &value.occupancy,
        value.native_direct,
        value.trace_total,
        value.source_unavailable,
        value.sequence,
        value.transcript_cursor,
        &value.event_id,
        &value.attempt_id,
        &value.turn_id,
        &value.timestamp,
        &value.event_kind,
        &value.tool_occurrence,
        value.result_ledger_position,
        &value.value,
        &value.fields,
        value.deferred,
    ))
    .unwrap()
}

#[test]
fn compact_observation_read_certificate_keeps_dense_floats_and_wide_opaque_integers() {
    for opaque in [
        serde_json::json!(vec![1.0_f64; 3900]),
        serde_json::json!([u64::MAX, i64::MIN, -0.0]),
    ] {
        let mut value = witness();
        value.value = opaque;
        let expected = serde_json::to_vec(&value).unwrap();
        assert!(expected.len() <= 16_384);
        assert_eq!(
            crate::runtime_client::preflight::bounded_fit(&value, 16_384),
            Some(false)
        );
        validate_public(&value, 16_384).unwrap();
        let restored =
            observation_event::decode_witness(&admitted_witness(&value), false, false, None)
                .unwrap();
        assert_eq!(serde_json::to_vec(&restored).unwrap(), expected);
        assert!(observation_event::encode_witness(&value).is_err());
    }
}

#[test]
fn compact_observation_read_certificate_checks_uncertain_keys_and_serializer_errors() {
    struct Broken(std::cell::Cell<usize>);
    impl Serialize for Broken {
        fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            self.0.set(self.0.get() + 1);
            Err(serde::ser::Error::custom("invalid public value"))
        }
    }
    let uncertain = std::collections::BTreeMap::from([(Some("key"), 3_u64)]);
    let invalid = std::collections::BTreeMap::from([((1_u8, 2_u8), 3_u64)]);
    assert_eq!(
        crate::runtime_client::preflight::bounded_fit(&uncertain, 128),
        None
    );
    assert_eq!(
        crate::runtime_client::preflight::bounded_fit(&invalid, 128),
        None
    );
    assert_eq!(
        validate_public(&uncertain, 128).is_ok(),
        serde_json::to_vec(&uncertain).is_ok()
    );
    assert!(validate_public(&invalid, 128).is_err());
    let broken = Broken(std::cell::Cell::new(0));
    assert!(validate_public(&broken, 128).is_err());
    assert_eq!(broken.0.get(), 2, "negative proof still tries actual JSON");
}

#[test]
fn compact_observation_read_certificate_preserves_exact_public_byte_boundary() {
    let mut value = witness();
    value.event_kind.clear();
    let base = serde_json::to_vec(&value).unwrap().len();
    for size in [16_383, 16_384, 16_385] {
        value.event_kind = "x".repeat(size - base);
        assert_eq!(serde_json::to_vec(&value).unwrap().len(), size);
        assert_eq!(
            crate::runtime_client::preflight::bounded_fit(&value, 16_384),
            Some(false)
        );
        assert_eq!(validate_public(&value, 16_384).is_ok(), size <= 16_384);
        assert_eq!(
            observation_event::decode_witness(&admitted_witness(&value), false, false, None)
                .is_ok(),
            size <= 16_384
        );
    }
}

#[test]
fn compact_tuple_reader_counts_actual_json_before_ignoring_fields() {
    #[derive(Deserialize)]
    struct Selected {
        keep: u64,
    }
    for ignored in [
        serde_json::json!(null),
        serde_json::json!("水\n\u{0000}"),
        serde_json::json!(vec![1.0_f64; 3900]),
    ] {
        let record = (serde_json::json!({"keep":u64::MAX,"ignored":ignored}),);
        let json = serde_json::to_vec(&record).unwrap();
        let binary = encode_admitted(&record).unwrap();
        let selected: (Selected,) = decode_json_tuple(&binary, json.len()).unwrap();
        assert_eq!(selected.0.keep, u64::MAX);
        assert!(decode_json_tuple::<(Selected,)>(&binary, json.len() - 1).is_err());
        let selected_json = serde_json::to_vec(&(serde_json::json!({"keep":u64::MAX}),)).unwrap();
        assert!(decode_json_tuple::<(Selected,)>(&binary, selected_json.len()).is_err());
    }
    let ordered = (
        std::collections::BTreeMap::from([
            ("z".to_owned(), vec![-0.0, 1.0, f64::MIN_POSITIVE]),
            ("a".to_owned(), vec![f64::MAX]),
        ]),
        i64::MIN,
        u64::MAX,
    );
    let json = serde_json::to_vec(&ordered).unwrap();
    let binary = encode_admitted(&ordered).unwrap();
    let restored: (std::collections::BTreeMap<String, Vec<f64>>, i64, u64) =
        decode_json_tuple(&binary, json.len()).unwrap();
    assert_eq!(serde_json::to_vec(&restored).unwrap(), json);
    assert_eq!(restored.0["z"][0].to_bits(), (-0.0_f64).to_bits());
}

#[test]
fn compact_tuple_reader_prescans_hostile_values_and_rejects_every_suffix() {
    thread_local! { static TYPED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
    struct Probe;
    impl<'de> Deserialize<'de> for Probe {
        fn deserialize<D: de::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
            TYPED.with(|calls| calls.set(calls.get() + 1));
            de::IgnoredAny::deserialize(decoder).map(|_| Self)
        }
    }
    let mut depth = vec![0x91; 130];
    depth.push(0xc0);
    for bytes in [
        vec![0x91, 0xdd, 0xff, 0xff, 0xff, 0xff],
        vec![0x91, 0xdf, 0xff, 0xff, 0xff, 0xff],
        vec![0x91, 0xdb, 0xff, 0xff, 0xff, 0xff],
        vec![0x91, 0xc4, 0x01, 0x41],
        vec![0x91, 0xd4, 0, 0],
        vec![0x91, 0x81, 0x01, 0xc0],
        vec![0x91, 0x81, 0xc0, 0xc0],
        vec![0x91, 0x81, 0xc4, 0x01, b'x', 0xc0],
        depth,
    ] {
        assert!(decode_json_tuple::<Probe>(&bytes, 16_384).is_err());
        assert_eq!(TYPED.with(std::cell::Cell::get), 0);
    }
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let bytes = encode_admitted(&(value,)).unwrap();
        assert!(decode_json_tuple::<Probe>(&bytes, 16_384).is_err());
        assert_eq!(TYPED.with(std::cell::Cell::get), 0);
    }
    let bytes = encode_admitted(&(u64::MAX,)).unwrap();
    for length in 0..bytes.len() {
        assert!(decode_json_tuple::<(u64,)>(&bytes[..length], 16_384).is_err());
    }
    for marker in 0..=255 {
        let mut trailing = bytes.clone();
        trailing.push(marker);
        assert!(
            decode_json_tuple::<(u64,)>(&trailing, 16_384).is_err(),
            "marker {marker}"
        );
    }
    assert_eq!(
        decode_json_tuple::<(u64,)>(&bytes, 16_384).unwrap(),
        (u64::MAX,)
    );
}
