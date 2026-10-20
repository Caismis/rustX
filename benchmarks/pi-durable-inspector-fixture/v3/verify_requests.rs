fn verify_requests(
    store: Arc<SqliteConversationStore>,
    case: &str,
    workspace: &std::path::Path,
    requests: &[serde_json::Value],
) -> Vec<serde_json::Value> {
    use rustx::durable::presentation::{FactQuery, FactScope};
    let starts = store
        .read_presentation_events(&FactQuery {
            scope: FactScope::All,
            kinds: vec!["model_request_started"],
            before: None,
            after: 0,
            ascending: true,
            through: store.presentation_frontier().unwrap(),
            limit: 128,
        })
        .unwrap();
    let conversation = store.conversation_id().clone();
    let history = rustx::runtime::RequestHistory::new(store);
    let snapshots = crate::common::request_snapshots(&history);
    assert_eq!(snapshots.len(), requests.len());
    assert_eq!(starts.len(), requests.len());
    let mut identities = std::collections::BTreeSet::new();
    snapshots
        .into_iter()
        .zip(&starts)
        .zip(requests)
        .enumerate()
        .map(|(index, ((snapshot, start), wire))| {
            assert!(identities.insert(snapshot.request_id.clone()));
            assert_eq!(snapshot.request_id, snapshot.identity.request_id());
            assert_eq!(
                snapshot.provisional_message_id,
                snapshot.identity.provisional_message_id()
            );
            assert_eq!(
                history.get(&snapshot.identity).unwrap().as_ref(),
                Some(&snapshot)
            );
            assert_eq!(wire["index"], index);
            assert_eq!(wire["method"], "POST");
            assert_eq!(wire["path"], "/v1/chat/completions");
            assert_eq!(wire["protocol"], "openai_chat_completions");
            assert_eq!(wire["model"], "chat-model");
            let reconstructed = history.reconstruct(&snapshot.identity).unwrap();
            assert_eq!(
                reconstructed.semantic_input_fingerprint(),
                snapshot.semantic_input_fingerprint
            );
            let request = serde_json::to_value(reconstructed).unwrap();
            let snapshot = serde_json::to_value(snapshot).unwrap();
            request_oracle::assert_identity(
                &snapshot,
                &request,
                &serde_json::to_value(start).unwrap(),
                conversation.as_str(),
            );
            request_oracle::assert_bodies(
                case,
                index,
                workspace.to_str().unwrap(),
                &wire["body"],
                &request_oracle::reconstructed_body(&request),
            );
            // Retain plain values, never a Source/store owner across final close.
            json!({"snapshot":snapshot,"request":request})
        })
        .collect()
}

