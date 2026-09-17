/// Repeatable child identities for ownership fixtures. This maps a fixture key
/// into UUID bits only; neither allocation nor semantic ordering uses this map.
pub fn child_conversation_id(key: &str) -> rustx::runtime::identity::ConversationId {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(key.as_bytes());
    let mut bytes: [u8; 16] = digest[..16].try_into().unwrap();
    bytes[6] = (bytes[6] & 0x0f) | 0x70;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    rustx::runtime::identity::ConversationId::from_uuid(uuid::Uuid::from_bytes(bytes)).unwrap()
}
