//! Native reminder rearm extends only the approved retry context lane.
use super::*;
use crate::context::contribution::ContributionStart;
use crate::context::{ContributionOpportunities, ContributorGeneration};
use crate::runtime::identity::{ContextContributorIdentity, NativeContextContributor};
use crate::scripted_suites::support::context::{checkpoint_commit_fixture, checkpoint_fixture};

fn refresh_snapshot(
    store: &SqliteConversationStore,
    previous: &RequestSnapshot,
    ordinal: u32,
    id: &str,
) -> (RequestSnapshot, MessageBlock) {
    let owner = ContextContributorIdentity::Native(NativeContextContributor::PermissionReminder);
    let message_id = MessageId::new(id);
    let message = MessageBlock::User(UserMessageBlock {
        id: message_id.clone(),
        content: crate::context::assembly::permission_reminder_content(
            &crate::runtime::permission::permission_reminder_text(previous.permission_mode),
        ),
        source: UserSource::Runtime,
        kind: InboundKind::Context(ContextKind::NativeEnvironment),
        timestamp: None,
    });
    let mut snapshot = previous.clone();
    snapshot.identity.retry_number = ordinal;
    snapshot.request_id = snapshot.identity.request_id();
    snapshot.provisional_message_id = snapshot.identity.provisional_message_id();
    snapshot.canonical_frontier =
        crate::context::resources::LedgerFrontier(store.load_head().unwrap().revision.next().get());
    snapshot.request_context_ids = vec![message_id.clone()];
    if !snapshot
        .context_generation
        .contributors
        .iter()
        .any(|entry| entry.identity == owner)
    {
        snapshot
            .context_generation
            .contributors
            .push(ContributorGeneration {
                identity: owner.clone(),
                attestation: None,
            });
    }
    snapshot.contributions.push(ContributionStart {
        message_id,
        producer: owner,
        metadata: ContextKind::NativeEnvironment,
        presentation: None,
        emissions: vec![],
        opportunities: ContributionOpportunities::default(),
        post_tool_batch_anchor: None,
    });
    freeze_test_input(store, &mut snapshot, std::slice::from_ref(&message));
    (snapshot, message)
}

fn assert_rearm_rejects_other_retry_context(
    store: &SqliteConversationStore,
    snapshot: &RequestSnapshot,
    reminder: &MessageBlock,
) {
    let mut ordinary_retry = snapshot.clone();
    ordinary_retry.contributions.last_mut().unwrap().producer =
        ContextContributorIdentity::Native(NativeContextContributor::RuntimeToolObservation);
    assert!(
        store
            .commit_model_turn_start(std::slice::from_ref(reminder), &ordinary_retry, Utc::now(),)
            .is_err(),
        "the retry exception grants no ordinary producer lane"
    );
    let mut forged = reminder.clone();
    let MessageBlock::User(user) = &mut forged else {
        unreachable!()
    };
    user.content = vec![UserContentBlock::Text(TextBlock {
        text: "forged reminder".into(),
    })];
    assert!(
        store
            .commit_model_turn_start(std::slice::from_ref(&forged), snapshot, Utc::now(),)
            .is_err(),
        "native producer identity alone does not authorize arbitrary content"
    );
}

#[test]
fn reminder_rearm_retries_preserve_receipts_and_atomic_start_reconstruction() {
    let store = store();
    store
        .initialize(&[user_message("history", "old history")])
        .unwrap();
    let ordinary = request_context_message("ordinary", "frozen ordinary context");
    let mut original = context_start_snapshot(
        store.load_head().unwrap().revision.next(),
        vec![ordinary.id().clone()],
    );
    freeze_test_input(&store, &mut original, std::slice::from_ref(&ordinary));
    store
        .commit_model_turn_start(std::slice::from_ref(&ordinary), &original, Utc::now())
        .unwrap();

    let (without_checkpoint, reminder) = refresh_snapshot(&store, &original, 1, "refresh-1");
    assert!(
        store
            .commit_model_turn_start(
                std::slice::from_ref(&reminder),
                &without_checkpoint,
                Utc::now(),
            )
            .unwrap_err()
            .to_string()
            .contains("newly activated checkpoint")
    );
    assert_eq!(store.load_canonical().unwrap().len(), 2);

    store
        .commit_projection(checkpoint_commit_fixture(
            &store,
            checkpoint_fixture(&store, "reminder-checkpoint-1", 1, "short summary"),
            None,
        ))
        .unwrap();
    let (first, reminder) = refresh_snapshot(&store, &original, 1, "refresh-1");
    assert_rearm_rejects_other_retry_context(&store, &first, &reminder);

    store.arm_request_start_fault_script([RequestStartFaultOperation::AfterContextAppend]);
    assert!(
        store
            .commit_model_turn_start(std::slice::from_ref(&reminder), &first, Utc::now(),)
            .is_err()
    );
    assert_eq!(
        store.load_canonical().unwrap().len(),
        2,
        "failed start rolls rearm back"
    );
    store
        .commit_model_turn_start(std::slice::from_ref(&reminder), &first, Utc::now())
        .unwrap();
    assert_eq!(
        store
            .commit_model_turn_start(std::slice::from_ref(&reminder), &first, Utc::now(),)
            .unwrap()
            .disposition,
        ModelTurnStartCommitDisposition::IdempotentReplay
    );
    assert_eq!(first.contributions[0], original.contributions[0]);

    let mut transient = first.clone();
    transient.identity.retry_number = 2;
    transient.request_id = transient.identity.request_id();
    transient.provisional_message_id = transient.identity.provisional_message_id();
    transient.request_context_ids.clear();
    transient.canonical_frontier =
        crate::context::resources::LedgerFrontier(store.load_head().unwrap().revision.get());
    freeze_test_input(&store, &mut transient, &[]);
    store
        .commit_model_turn_start(&[], &transient, Utc::now())
        .unwrap();

    store
        .commit_projection(checkpoint_commit_fixture(
            &store,
            checkpoint_fixture(&store, "reminder-checkpoint-2", 1, "new summary"),
            None,
        ))
        .unwrap();
    let (second, reminder2) = refresh_snapshot(&store, &transient, 3, "refresh-2");
    store
        .commit_model_turn_start(std::slice::from_ref(&reminder2), &second, Utc::now())
        .unwrap();
    assert_eq!(second.context_generation, first.context_generation);
    assert_eq!(&second.contributions[..2], &first.contributions);
    assert_eq!(second.request_context_ids, vec![reminder2.id().clone()]);
    assert_eq!(
        store
            .commit_model_turn_start(std::slice::from_ref(&reminder2), &second, Utc::now(),)
            .unwrap()
            .disposition,
        ModelTurnStartCommitDisposition::IdempotentReplay
    );
    let replay = store.load_request_snapshot(&second.request_id).unwrap();
    assert_eq!(replay, second);
}
