//! Process-death fixture logs original Journal pages, independently of display.
use super::{ApprovalDecision, ConversationRuntime, InteractionResponse, Log, Seen};
use crate::scripted_suites::support::native_journal::OriginalJournal;
use std::sync::Arc;

pub(super) struct ObserverTask(tokio::task::JoinHandle<()>);
impl Drop for ObserverTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(super) fn bind(
    core: &crate::local_runtime::composition::LocalConversationCore,
    log: Arc<Log>,
    approve: bool,
) -> Result<ObserverTask, String> {
    let observations = Arc::new(super::PendingObservations::new());
    core.runtime()
        .install_observation_bridge(observations.clone())
        .map_err(|error| format!("{error:?}"))?;
    core.runtime().set_interaction_provider_available(true);
    let journal = OriginalJournal::install(
        core.runtime(),
        &core.tool_runtime().durable_store(),
        &observations,
    );
    Ok(spawn(core.runtime().clone(), journal, log, approve))
}
fn spawn(
    runtime: ConversationRuntime,
    mut journal: OriginalJournal,
    log: Arc<Log>,
    approve: bool,
) -> ObserverTask {
    let mut pending_read = journal.pending();
    ObserverTask(tokio::spawn(async move {
        let mut seen_pending = Vec::new();
        loop {
            tokio::select! {
                events=journal.next_page()=> {
                    for envelope in events {log.push(Seen::Event(Box::new(envelope.event)));}
                }
                changed=pending_read.changed()=> {
                    if changed.is_err() {return;}
                    let pending=pending_read.borrow_and_update().clone();
                    for interaction in pending {
                        if seen_pending.contains(&interaction) {continue;}
                        seen_pending.push(interaction.clone());
                        log.push(Seen::InteractionPending);
                        if approve {
                            runtime.respond_interaction(&interaction,InteractionResponse::Approval {decision:ApprovalDecision::Allow}).await.expect("answer the original pending approval");
                        }
                    }
                }
            }
        }
    }))
}
