//! Bounded task-catalog projection over exact native conversation readers.
use super::{RuntimeManagerError, SessionRuntimeManager, error};
use crate::local_runtime::session::{SessionId, SessionNodeId, SessionSummary};
use crate::scheduler::{ScheduleBinding, ScheduleHeader, ScheduleSnapshot, ScheduleTarget};
use serde::{Deserialize, Serialize};

/// A keyset position. It confers no allocation or execution authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduleCatalogCursor {
    /// Original Session of the last inspected node.
    pub session_id: SessionId,
    /// Last inspected node, ordered by its stable native identity.
    pub node_id: SessionNodeId,
    /// Last inspected task ordinal; the maximum value skips the exhausted node.
    pub after_ordinal: u64,
}
/// One task and its independently owned Session display metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ScheduleCatalogEntry {
    /// Header only; task instructions and pinned descriptors are deferred.
    pub schedule: ScheduleHeader,
    /// Saved display metadata; reading it never activates the Session.
    pub session: SessionSummary,
}
/// A finite catalog page. Failed sources remain explicit instead of becoming empty.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ScheduleCatalogPage {
    /// Matching native task headers in keyset order.
    pub entries: Vec<ScheduleCatalogEntry>,
    /// Continue the same query here, including pages containing no matches.
    pub next: Option<ScheduleCatalogCursor>,
    /// Sources which could not be read; their prior display data is not absence.
    pub unavailable: Vec<ScheduleBinding>,
}
fn position(binding: &ScheduleBinding, after_ordinal: u64) -> ScheduleCatalogCursor {
    ScheduleCatalogCursor {
        session_id: binding.session_id.clone(),
        node_id: binding.node_id.clone(),
        after_ordinal,
    }
}
fn key(session: &SessionId, node: &SessionNodeId) -> (String, String) {
    (session.to_string(), node.to_string())
}
fn matches(value: &ScheduleSnapshot, query: &str) -> bool {
    if query.is_empty()
        || value.title.to_lowercase().contains(query)
        || value
            .binding
            .session_id
            .to_string()
            .to_lowercase()
            .contains(query)
    {
        return true;
    }
    match &value.target {
        ScheduleTarget::ConversationInbound { text } => text.to_lowercase().contains(query),
        ScheduleTarget::Workflow { pinned, arguments } => {
            pinned
                .definition
                .workflow_id
                .to_string()
                .to_lowercase()
                .contains(query)
                || arguments.to_string().to_lowercase().contains(query)
        }
    }
}
impl SessionRuntimeManager {
    /// Read tasks across exact nodes without attaching or composing any runtime.
    /// Each page opens at most eight bounded readers and inspects at most 128 rows.
    /// # Errors
    /// Invalid bounds and unavailable catalog metadata are returned explicitly.
    #[allow(clippy::too_many_lines)] // One finite traversal keeps failures and its keyset cursor together.
    pub async fn schedule_catalog(
        &self,
        session: Option<&SessionId>,
        after: Option<&ScheduleCatalogCursor>,
        limit: u32,
        query: Option<&str>,
    ) -> Result<ScheduleCatalogPage, RuntimeManagerError> {
        if !(1..=100).contains(&limit)
            || query.is_some_and(|value| value.len() > 1024)
            || session
                .zip(after)
                .is_some_and(|(id, cursor)| id != &cursor.session_id)
        {
            return Err(error("invalid task catalog bounds"));
        }
        let catalog = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            self.sessions.catalog.lock().await.clone()
        })
        .await
        .map_err(error)?;
        let mut bindings = catalog.schedule_bindings();
        bindings.retain(|binding| session.is_none_or(|id| id == &binding.session_id));
        bindings.sort_by_key(|binding| key(&binding.session_id, &binding.node_id));
        let after_key = after.map(|value| key(&value.session_id, &value.node_id));
        let query = query.unwrap_or("").trim().to_lowercase();
        let mut entries = Vec::new();
        let mut unavailable = Vec::new();
        let mut inspected = 0_u32;
        let mut reads = 0_u32;
        let mut cursor = after.cloned();
        for binding in bindings {
            let binding_key = key(&binding.session_id, &binding.node_id);
            if after_key.as_ref().is_some_and(|held| &binding_key < held) {
                continue;
            }
            let mut ordinal = if after_key.as_ref() == Some(&binding_key) {
                after.map_or(0, |value| value.after_ordinal)
            } else {
                0
            };
            if ordinal == u64::MAX {
                continue;
            }
            'source: loop {
                if reads >= 8 || inspected >= 128 || entries.len() >= limit as usize {
                    return Ok(ScheduleCatalogPage {
                        entries,
                        next: cursor,
                        unavailable,
                    });
                }
                reads += 1;
                let Ok(summary) = catalog.summary(&binding.session_id) else {
                    unavailable.push(binding.clone());
                    cursor = Some(position(&binding, u64::MAX));
                    break;
                };
                let page_limit = 32.min(128 - inspected);
                let Ok(page) = self
                    .schedule_list(&binding.session_id, &binding.node_id, ordinal, page_limit)
                    .await
                else {
                    unavailable.push(binding.clone());
                    cursor = Some(position(&binding, u64::MAX));
                    break;
                };
                let full = page.len() == page_limit as usize;
                for value in page {
                    inspected += 1;
                    ordinal = value.id.ordinal;
                    cursor = Some(position(&binding, ordinal));
                    if value.binding != binding {
                        unavailable.push(binding.clone());
                        cursor = Some(position(&binding, u64::MAX));
                        break 'source;
                    }
                    if matches(&value, &query) {
                        entries.push(ScheduleCatalogEntry {
                            schedule: value.into(),
                            session: summary.clone(),
                        });
                        if entries.len() == limit as usize {
                            return Ok(ScheduleCatalogPage {
                                entries,
                                next: cursor,
                                unavailable,
                            });
                        }
                    }
                }
                if !full {
                    cursor = Some(position(&binding, u64::MAX));
                    break;
                }
            }
        }
        Ok(ScheduleCatalogPage {
            entries,
            next: None,
            unavailable,
        })
    }
    pub(crate) fn schedule_catalog_changes(&self) -> tokio::sync::watch::Receiver<()> {
        self.schedule_catalog_changed.subscribe()
    }
}
