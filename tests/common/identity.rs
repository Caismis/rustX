/// Repeatable child identities for ownership fixtures. This maps a fixture key
/// into UUID bits only; neither allocation nor semantic ordering uses this map.
pub fn child_conversation_id(key: &str) -> rustx::runtime::identity::ConversationId {
    rustx::durable::sqlite::test_support::child_conversation_id(key)
}
