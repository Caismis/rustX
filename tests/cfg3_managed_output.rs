use rustx::runtime::identity::{ConversationId, ToolExecutionId, UuidV7Generator};
use rustx::tools::managed_output::{ManagedOutputError, ManagedToolOutput};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

#[derive(Debug)]
struct Identities(Mutex<VecDeque<uuid::Uuid>>);
impl UuidV7Generator for Identities {
    fn next_uuid(&self) -> uuid::Uuid {
        let mut values = self.0.lock().unwrap();
        if values.len() > 1 {
            values.pop_front().unwrap()
        } else {
            *values.front().unwrap()
        }
    }
}
fn ids(values: &[&str]) -> Arc<dyn UuidV7Generator> {
    Arc::new(Identities(Mutex::new(
        values
            .iter()
            .map(|value| uuid::Uuid::parse_str(value).unwrap())
            .collect(),
    )))
}
const FIRST: &str = "01900000-0000-7000-8000-000000000001";
const SECOND: &str = "01900000-0000-7000-8000-000000000002";
fn store(root: &std::path::Path) -> ManagedToolOutput {
    ManagedToolOutput::new(
        ConversationId::new("conv_01900000-0000-7000-8000-000000000001"),
        root,
    )
    .unwrap()
}
#[test]
fn spill_collision_retries_without_overwrite_and_reconstruction_retains_content() {
    let directory = tempfile::tempdir().unwrap();
    let original = store(directory.path()).with_identity_generator(ids(&[FIRST]));
    let mut spill = original.open_spill().unwrap();
    spill.write_all("retained original").unwrap();
    let original_path = spill.path().to_owned();
    drop(spill);
    drop(original);
    let reconstructed = store(directory.path()).with_identity_generator(ids(&[FIRST, SECOND]));
    let second = reconstructed.open_spill().unwrap();
    assert_eq!(
        second.path().file_name().unwrap(),
        format!("result_{SECOND}.txt").as_str()
    );
    assert_eq!(
        std::fs::read_to_string(original_path).unwrap(),
        "retained original"
    );
}
#[test]
fn collision_budget_refuses_instead_of_overwriting_or_looping() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(directory.path()).with_identity_generator(ids(&[FIRST]));
    let first = store.open_spill().unwrap();
    assert!(matches!(
        store.open_spill(),
        Err(ManagedOutputError::IdentityCollision)
    ));
    assert!(first.path().exists());
}
#[test]
fn background_output_locator_is_the_execution_identity_and_collision_preserves_it() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(directory.path()).with_identity_generator(ids(&[FIRST]));
    let execution = store.execution_identity().unwrap();
    assert_eq!(execution.as_str(), format!("exec_{FIRST}"));
    let path = store.allocate_background_output(&execution).unwrap();
    std::fs::write(&path, "existing output").unwrap();
    assert!(store.allocate_background_output(&execution).is_err());
    assert_eq!(store.background_output_path(&execution), path);
    assert_eq!(std::fs::read_to_string(path).unwrap(), "existing output");
}
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
