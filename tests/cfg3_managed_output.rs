use rustx::runtime::identity::{ConversationId, ToolExecutionId};
const FIRST: &str = "01900000-0000-7000-8000-000000000001";
#[test]
fn execution_identity_rejects_old_ordinals_non_v7_and_noncanonical_forms() {
    for bad in [
        "exec_1",
        "exec_01900000-0000-4000-8000-000000000001",
        "exec_01900000000070008000000000000001",
    ] {
        assert!(ToolExecutionId::parse(bad).is_err());
        assert!(serde_json::from_value::<ToolExecutionId>(serde_json::json!(bad)).is_err());
    }
    let good = ToolExecutionId::parse(format!("exec_{FIRST}")).unwrap();
    assert_eq!(
        serde_json::from_str::<ToolExecutionId>(&serde_json::to_string(&good).unwrap()).unwrap(),
        good
    );
}

#[test]
fn durable_identity_domains_validate_prefix_and_uuid_version() {
    use rustx::local_runtime::session::{SessionId, SessionNodeId};
    let uuid = uuid::Uuid::parse_str(FIRST).unwrap();
    assert_eq!(
        SessionId::from_uuid(uuid).unwrap().as_str(),
        format!("ses_{FIRST}")
    );
    assert_eq!(
        SessionNodeId::from_uuid(uuid).unwrap().as_str(),
        format!("node_{FIRST}")
    );
    assert_eq!(
        ConversationId::from_uuid(uuid).unwrap().as_str(),
        format!("conv_{FIRST}")
    );
    assert!(SessionId::parse(format!("conv_{FIRST}")).is_err());
    assert!(SessionNodeId::parse("node-1").is_err());
    assert!(ConversationId::parse("conversation-1").is_err());
    assert!(serde_json::from_str::<SessionId>("\"session-1\"").is_err());
}
