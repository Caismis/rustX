use super::super::scalar_cache::Owner;
use super::*;

#[derive(serde::Serialize, schemars::JsonSchema)]
struct MemoryScalar {
    tools: Vec<serde_json::Value>,
    messages: Vec<u8>,
}
pub(super) fn source(marker: &str) -> SeekableObservationSource {
    // A valid nested JSON Schema gives long repeated locator paths without
    // exceeding the 64 KiB Tools bound or 2,048 fact rows.
    let mut schema = serde_json::json!({"type":"object","properties":(0..28).map(|n| (
        format!("leaf-{n:03}"), serde_json::json!({"type":"string"})
    )).collect::<serde_json::Map<_,_>>()});
    for ordinal in 0..12 {
        let key = format!("level-{ordinal:02}-{}", "k".repeat(160));
        schema = serde_json::json!({"type":"object","properties":serde_json::Value::Object(
            [(key, schema)].into_iter().collect()
        )});
    }
    SeekableObservationSource::new(&MemoryScalar {
        tools: vec![serde_json::json!({
            "tool_id":"memory-probe-tool", "name":marker,
            "description":{"text":"bounded valid nested schema","truncated":false},
            "input_schema":{"value":schema,"truncated":false}
        })],
        messages: Vec::new(),
    })
    .unwrap()
}
fn connection() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    super::super::create_schema(&connection).unwrap();
    connection
}
fn pages(connection: &Connection) -> Vec<(Vec<u8>, Vec<u8>)> {
    connection
        .prepare("SELECT hash,bytes FROM request_source_pages ORDER BY hash")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}
fn assert_pending(owner: &Owner, session: &super::super::scalar_cache::Session) -> (usize, usize) {
    let retained = owner.retained_work().map_or(0, |work| work.1);
    let (reused, rows, pending) = session.work();
    assert_eq!(reused, 0, "alternating different catalogs must miss");
    assert!(rows > 0);
    assert!((1_000_000..=1_048_576).contains(&pending));
    assert!(
        retained + pending <= 2 * 1_048_576,
        "retained and pending entry buffers have separate 1 MiB budgets"
    );
    (retained, pending)
}
#[test]
fn alternating_near_budget_catalogs_keep_exact_pages_and_separate_pending_bound() {
    let sources = [source("catalog-A"), source("catalog-B")];
    let baseline = connection();
    let cached = connection();
    let owner = Owner::default();
    let scope = crate::runtime::identity::ConversationId::generate();
    for cycle in 0..16 {
        let current = &sources[cycle % 2];
        let expected = scalar(&baseline, current, None).unwrap();
        let mut session = owner.session(&scope);
        let actual = scalar(&cached, current, Some((&scope, &mut session))).unwrap();
        let (retained, pending) = assert_pending(&owner, &session);
        if cycle > 0 {
            assert!(retained >= 1_000_000);
        }
        assert_eq!(
            serde_json::to_vec(&actual.0).unwrap(),
            serde_json::to_vec(&expected.0).unwrap()
        );
        assert_eq!((actual.1, actual.2), (expected.1, expected.2));
        assert_eq!(pages(&cached), pages(&baseline));
        session.commit(&owner);
        assert_eq!(owner.retained_work().unwrap().1, pending);
    }
}
