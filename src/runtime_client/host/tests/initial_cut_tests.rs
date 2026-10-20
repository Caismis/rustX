// Genuine pre-activation checkpoint evidence and immutable capability reads.
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t08_t14_t20_initial_checkpoint_pages_survive_unrelated_publication() {
    let (_adapter, fixture) = host_fixture_before_activation(
        Vec::new(),
        ToolRegistry::new(),
        status_engine(),
        true,
        None,
    )
    .await;
    let initial = fixture.host.checkpoint().unwrap();
    assert_eq!(initial.cursor.get(), 0);
    let inner = fixture.host.weak_inner().upgrade().unwrap();
    let domain = "capabilities.available_tools";
    let before = inner
        .checkpoint_page(&initial.read_cut, domain, 0, 32)
        .unwrap();
    assert!(before.total > 0);

    fixture.runtime.activate();
    await_fixture_activation(&fixture.host).await;
    inner.park_projection_worker();
    let published = fixture.host.checkpoint().unwrap();
    assert!(published.cursor.get() > 0);
    assert_eq!(
        published.read_cut.domain_revisions.get("capabilities"),
        initial.read_cut.domain_revisions.get("capabilities")
    );
    assert_eq!(
        inner
            .checkpoint_page(&initial.read_cut, domain, 0, 32)
            .unwrap(),
        before
    );
    assert_eq!(fixture.host.checkpoint().unwrap(), published);

    let mut wrong_scope = initial.read_cut.clone();
    wrong_scope.activation_id = Some(crate::runtime::identity::SubagentId::for_conversation(
        &initial.read_cut.conversation_id,
        1,
    ));
    assert!(matches!(
        inner.checkpoint_page(&wrong_scope, domain, 0, 32),
        Err(RuntimeClientError::ReadCutExpired)
    ));

    fixture
        .host
        .model_set(marked_model_config(
            &fixture.runtime,
            "changed-capability-owner",
        ))
        .unwrap();
    assert_ne!(
        fixture
            .host
            .checkpoint()
            .unwrap()
            .read_cut
            .domain_revisions
            .get("capabilities"),
        initial.read_cut.domain_revisions.get("capabilities")
    );
    assert!(matches!(
        inner.checkpoint_page(&initial.read_cut, domain, 0, 32),
        Err(RuntimeClientError::ReadCutExpired)
    ));
    inner.unpark_projection_worker();
    fixture.runtime.shutdown().await.unwrap();
}
