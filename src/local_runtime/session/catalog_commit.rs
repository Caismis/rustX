//! The Catalog document commit protocol: planned documents, validation,
//! temporary write + fsync, the rename visibility point, and the parent
//! directory durability barrier.

use super::CatalogDocument;
use super::PersistedSession;
use super::SESSION_CATALOG_SCHEMA_VERSION;
use super::SESSION_NAME_LIMIT;
use super::SessionCatalog;
use super::SessionError;
use super::SessionNode;
use super::SessionNodeOrigin;
use super::SessionPersistentState;
#[cfg(test)]
use super::create_profile;
use super::creation::next_ordinal;
use super::deletion;
use super::lineage::PreparedLineage;
use super::lineage::lineage_of;
use super::observation;
use super::projection::single_line;
use super::uploads;
use crate::runtime::identity::SessionId;
use crate::runtime::identity::SessionNodeId;
use chrono::Utc;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;

/// A call-site-local startup destination and a generation-checked publication plan.
/// The target is never serialized. A composition may fail before publication;
/// its private seed must not create visible catalog membership.
/// Opening any existing node produces an unchanged plan and writes nothing.
#[derive(Debug, Clone)]
pub(crate) struct PlannedCatalog {
    target: SessionId,
    target_node: SessionNodeId,
    /// The complete document to persist.
    document: CatalogDocument,
    /// Whether the plan differs from the catalog it was planned against.
    /// An unchanged plan commits nothing.
    changed: bool,
}

impl PlannedCatalog {
    /// The explicit destination node of this startup plan, and its Session-local state.
    ///
    /// Read from the planned document, not from the catalog on disk: this is
    /// the destination the caller must compose for.
    pub(crate) fn destination_lineage(
        &self,
    ) -> Result<(SessionId, SessionNode, SessionPersistentState), SessionError> {
        lineage_of(&self.document, &self.target, &self.target_node)
    }
}

impl SessionCatalog {
    /// Prepare real benchmark databases, then publish the validated catalog once.
    /// The caller holds the fixture catalog exclusively before starting measurement.
    /// Database initialization, identity reservations and the final durable catalog
    /// commit use their ordinary owners; no measured read path is replaced.
    #[cfg(test)]
    pub(crate) fn seed_benchmark_sessions(
        &mut self,
        state: &SessionPersistentState,
        seed: &[crate::message::types::MessageBlock],
        count: usize,
    ) -> Result<Vec<(super::SessionSnapshot, PathBuf)>, SessionError> {
        if count == 0 {
            return Ok(Vec::new());
        }
        let mut draft = self.clone();
        let mut identities = Vec::with_capacity(count);
        for index in 0..count {
            let prepared = draft.prepare_session(state, seed)?;
            draft.document = draft.build_session_document(&prepared, SessionNodeOrigin::New)?;
            identities.push((prepared.session_id, prepared.database_path));
            if (index + 1).is_multiple_of(100) {
                eprintln!("Benchmark Session preparation {}/{count}", index + 1);
            }
        }
        self.commit(draft.document)?;
        identities
            .into_iter()
            .map(|(id, path)| self.snapshot(&id).map(|snapshot| (snapshot, path)))
            .collect()
    }

    /// A planned catalog transition that changes nothing.
    ///
    /// An unpublished catalog — a first launch, see
    /// [`Self::create_unpublished`] — still has a pending first write, so
    /// its "unchanged" plan commits the document it was built with. Every
    /// other catalog commits nothing.
    #[must_use]
    pub(crate) fn plan_unchanged(&self, target: &SessionId) -> PlannedCatalog {
        PlannedCatalog {
            target: target.clone(),
            target_node: self.document.sessions[target].active_node.clone(),
            document: self.document.clone(),
            changed: !self.published,
        }
    }

    /// Plan a client routing target independently of the durable graph default.
    /// Neither an explicit historical node nor the default node causes publication.
    pub(crate) fn plan_attachment(
        &self,
        session_id: &SessionId,
        node_id: Option<&SessionNodeId>,
    ) -> Result<PlannedCatalog, SessionError> {
        let (node, _) = self.lineage(session_id, node_id)?;
        Ok(PlannedCatalog {
            target: session_id.clone(),
            target_node: node.id,
            document: self.document.clone(),
            changed: false,
        })
    }

    /// Plans the publication of a prepared independent Session without
    /// publishing it.
    pub(crate) fn plan_session(
        &self,
        prepared: &PreparedLineage,
        origin: SessionNodeOrigin,
    ) -> Result<PlannedCatalog, SessionError> {
        Ok(PlannedCatalog {
            target: prepared.session_id.clone(),
            target_node: prepared.node_id.clone(),
            document: self.build_session_document(prepared, origin)?,
            changed: true,
        })
    }

    /// Commits one planned transition as a single catalog transaction.
    ///
    /// A plan that changes nothing writes nothing: an unchanged document is
    /// not rewritten just because a launch looked at it.
    pub(crate) fn commit_planned(&mut self, planned: PlannedCatalog) -> Result<(), SessionError> {
        if !planned.changed {
            return Ok(());
        }
        self.commit(planned.document)
    }

    pub(super) fn build_current_node_document(
        &self,
        session_id: &SessionId,
        node_id: Option<&SessionNodeId>,
    ) -> Result<CatalogDocument, SessionError> {
        let (_, _) = self.lineage(session_id, node_id)?;
        let mut next = self.document.clone();
        let session =
            next.sessions
                .get_mut(session_id)
                .ok_or_else(|| SessionError::UnknownSession {
                    session_id: session_id.clone(),
                })?;
        let selected_node = node_id
            .cloned()
            .unwrap_or_else(|| session.active_node.clone());
        if !session.nodes.contains_key(&selected_node) {
            return Err(SessionError::UnknownNode {
                session_id: session_id.clone(),
                node_id: selected_node,
            });
        }
        session.active_node = selected_node;
        session.updated_at = Utc::now();
        Ok(next)
    }

    pub(super) fn build_session_document(
        &self,
        prepared: &PreparedLineage,
        origin: SessionNodeOrigin,
    ) -> Result<CatalogDocument, SessionError> {
        self.reject_pending_identity(
            &prepared.session_id,
            &prepared.node_id,
            &prepared.conversation_id,
        )?;
        self.validate_new_session_identity(
            &prepared.session_id,
            &prepared.node_id,
            &prepared.conversation_id,
        )?;
        if self.document.sessions.contains_key(&prepared.session_id) {
            return Err(SessionError::Catalog {
                detail: format!(
                    "prepared Session {} has already been published",
                    prepared.session_id
                ),
            });
        }
        if !prepared.database_path.is_file() {
            return Err(SessionError::Catalog {
                detail: "prepared conversation seed is not a database file".to_owned(),
            });
        }
        let now = Utc::now();
        let node = SessionNode {
            ordinal: self.document.next_node_ordinal,
            id: prepared.node_id.clone(),
            parent: None,
            conversation_id: prepared.conversation_id.clone(),
            origin,
        };
        let mut nodes = BTreeMap::new();
        nodes.insert(prepared.node_id.clone(), node);
        let mut next = profile_stage!(
            |times: &mut create_profile::StageTimes| &mut times.catalog_document_clone_ns,
            self.document.clone()
        );
        next.sessions.insert(
            prepared.session_id.clone(),
            PersistedSession {
                ownership_generation: 0,
                ordinal: self.document.next_session_ordinal,
                uploads: prepared.uploads.clone(),
                id: prepared.session_id.clone(),
                name: prepared.name.clone(),
                generated_title: prepared.generated_title.clone(),
                // The projection of the frozen seed, persisted at the same
                // visibility commit that publishes the lineage itself: a
                // clone/fork destination is born listing its first retained
                // user message without ever opening its store.
                display_preview: prepared.display_preview.clone(),
                created_at: now,
                last_prompt_at: None,
                updated_at: now,
                active_node: prepared.node_id.clone(),
                nodes,
                state: prepared.state.clone(),
                settings_revision: 0,
            },
        );
        next.upload_preparations.remove(&prepared.session_id);
        next.next_session_ordinal = next_ordinal(self.document.next_session_ordinal)?;
        next.next_node_ordinal = next_ordinal(self.document.next_node_ordinal)?;
        Ok(next)
    }

    pub(super) fn build_node_document(
        &self,
        session_id: &SessionId,
        prepared: &PreparedLineage,
        parent: SessionNodeId,
        origin: SessionNodeOrigin,
    ) -> Result<CatalogDocument, SessionError> {
        self.validate_new_node_identity(&prepared.node_id, &prepared.conversation_id)?;
        if prepared.session_id != *session_id {
            return Err(SessionError::Catalog {
                detail: "prepared node belongs to another Session".to_owned(),
            });
        }
        if !prepared.database_path.is_file() {
            return Err(SessionError::Catalog {
                detail: "prepared conversation seed is not a database file".to_owned(),
            });
        }
        let mut next = self.document.clone();
        let session =
            next.sessions
                .get_mut(session_id)
                .ok_or_else(|| SessionError::UnknownSession {
                    session_id: session_id.clone(),
                })?;
        if !session.nodes.contains_key(&parent) {
            return Err(SessionError::UnknownNode {
                session_id: session_id.clone(),
                node_id: parent,
            });
        }
        let node = SessionNode {
            ordinal: self.document.next_node_ordinal,
            id: prepared.node_id.clone(),
            parent: Some(parent),
            conversation_id: prepared.conversation_id.clone(),
            origin,
        };
        session.nodes.insert(prepared.node_id.clone(), node);
        session.active_node = prepared.node_id.clone();
        session.updated_at = Utc::now();
        next.next_node_ordinal = next_ordinal(self.document.next_node_ordinal)?;
        Ok(next)
    }

    pub(super) fn commit(&mut self, next: CatalogDocument) -> Result<(), SessionError> {
        let _mutation = self
            .product
            .ownership_mutation()
            .map_err(|e| SessionError::Catalog {
                detail: e.to_string(),
            })?;
        self.commit_under_ownership(next)
    }

    pub(super) fn commit_under_ownership(
        &mut self,
        mut next: CatalogDocument,
    ) -> Result<(), SessionError> {
        // Root ownership precedes publication exclusion. Lock a stable directory,
        // never the replaced catalog inode. The generation comparison and rename
        // are one serialized publication; cloned/planned views cannot lose updates.
        fs::create_dir_all(&self.root).map_err(|e| SessionError::Catalog {
            detail: e.to_string(),
        })?;
        let file = fs::File::open(&self.root).map_err(|e| SessionError::Catalog {
            detail: e.to_string(),
        })?;
        let _publication =
            nix::fcntl::Flock::lock(file, nix::fcntl::FlockArg::LockExclusiveNonblock).map_err(
                |(_, e)| SessionError::Catalog {
                    detail: e.to_string(),
                },
            )?;
        let current = Self::read_under_guard(&self.product)?;
        if next.generation != self.document.generation
            || current.as_ref().map(|c| c.document.generation)
                != self.published.then_some(self.document.generation)
        {
            return Err(SessionError::Catalog {
                detail: "stale catalog generation".into(),
            });
        }
        next.generation = next
            .generation
            .checked_add(1)
            .ok_or_else(|| SessionError::Catalog {
                detail: "catalog generation exhausted".into(),
            })?;
        // Assign the catalog commit identity only to newly admitted ownership.
        // Metadata-only commits and repeated selection retain the old generation.
        for (id, session) in &mut next.sessions {
            session.ownership_generation = self
                .document
                .sessions
                .get(id)
                .filter(|old| self.published && old.active_node == session.active_node)
                .map_or(next.generation, |old| old.ownership_generation);
        }
        validate_document(&next)?;
        // Membership evidence belongs to the visibility commit, including
        // uncertain directory durability. Cleanup/recovery commits with unchanged
        // membership emit nothing. This covers creation, copies and deletion.
        let membership: Vec<_> = self
            .document
            .sessions
            .keys()
            .filter(|id| self.published && !next.sessions.contains_key(*id))
            .chain(
                next.sessions
                    .keys()
                    .filter(|id| !self.published || !self.document.sessions.contains_key(*id)),
            )
            .cloned()
            .collect();
        let retired: Vec<_> = self
            .document
            .sessions
            .iter()
            .filter(|(id, old)| {
                next.sessions
                    .get(*id)
                    .is_none_or(|new| new.active_node != old.active_node)
            })
            .map(|(id, old)| (id.clone(), old.ownership_generation))
            .collect();
        let committed = self.persist(&next);
        if committed.is_ok() || committed.as_ref().is_err_and(SessionError::committed) {
            self.node_observation_index = Arc::new(observation::node_index(&next));
            self.document = next;
            self.published = true;
            for (id, generation) in retired {
                self.summary_invalidations.retire_ownership(&id, generation);
            }
            for id in membership {
                self.summary_invalidations.record_change(&id, true);
            }
        }
        committed
    }

    pub(super) fn persist(&self, document: &CatalogDocument) -> Result<(), SessionError> {
        let bytes = profile_stage!(
            |times: &mut create_profile::StageTimes| &mut times.catalog_serialize_ns,
            serde_json::to_vec_pretty(document)
        )
        .map_err(|error| SessionError::CatalogCommit {
            error: CatalogCommitError::NotCommitted {
                path: self.path.clone(),
                detail: format!("cannot encode catalog: {error}"),
            },
        })?;
        #[cfg(test)]
        let result = atomic_write(&self.path, &bytes, &self.write_fault);
        #[cfg(not(test))]
        let result = atomic_write(&self.path, &bytes);
        result.map_err(SessionError::from)
    }
}

#[allow(clippy::too_many_lines)] // One ordered audit of all durable catalog invariants.
pub(super) fn validate_document(document: &CatalogDocument) -> Result<(), SessionError> {
    // An unexpected version is refused, never interpreted. See
    // `SESSION_CATALOG_SCHEMA_VERSION`: the fields of an older catalog decode
    // cleanly and mean something this code does not promise.
    if document.schema_version != SESSION_CATALOG_SCHEMA_VERSION {
        return Err(SessionError::Catalog {
            detail: format!(
                "unsupported session catalog schema {}; expected {}",
                document.schema_version, SESSION_CATALOG_SCHEMA_VERSION
            ),
        });
    }
    uploads::validate_preparations(document)?;
    deletion::validate_records(document)?;
    let mut conversation_ids = BTreeSet::new();
    let mut node_ids = BTreeSet::new();
    let mut session_ordinals = BTreeSet::new();
    let mut node_ordinals = BTreeSet::new();
    for (session_id, session) in &document.sessions {
        if document.retired.sessions.contains(session_id)
            || session.ownership_generation == 0
            || session.ownership_generation > document.generation
            || session.ordinal == 0
            || session.ordinal >= document.next_session_ordinal
            || !session_ordinals.insert(session.ordinal)
        {
            return Err(SessionError::Catalog {
                detail: "invalid Session publication ordinal or retired identity".into(),
            });
        }
        session
            .uploads
            .validate()
            .map_err(|e| SessionError::Catalog {
                detail: e.to_string(),
            })?;
        if !session.state.cwd.is_absolute() {
            return Err(SessionError::Catalog {
                detail: "durable Session context paths must be absolute".into(),
            });
        }
        validate_id(session_id.as_str(), "session")?;
        if session.id != *session_id {
            return Err(SessionError::Catalog {
                detail: format!("session key {session_id} disagrees with its record identity"),
            });
        }
        if session.nodes.is_empty() {
            return Err(SessionError::Catalog {
                detail: format!("session {session_id} has no nodes"),
            });
        }
        validate_active_node(session_id, session)?;
        if let Some(title) = &session.generated_title {
            super::title::validate_title(title, session)?;
        }
        for (node_id, node) in &session.nodes {
            if document.retired.nodes.contains(node_id)
                || document
                    .retired
                    .conversations
                    .contains(&node.conversation_id)
                || node.ordinal == 0
                || node.ordinal >= document.next_node_ordinal
                || !node_ordinals.insert(node.ordinal)
            {
                return Err(SessionError::Catalog {
                    detail: "invalid node publication ordinal or retired identity".into(),
                });
            }
            validate_id(node_id.as_str(), "node")?;
            if node.id != *node_id {
                return Err(SessionError::Catalog {
                    detail: format!("session {session_id} contains an invalid node {node_id}"),
                });
            }
            if !node_ids.insert(node.id.clone()) {
                return Err(SessionError::Catalog {
                    detail: format!("SessionNode identity {node_id} is duplicated"),
                });
            }
            validate_id(node.conversation_id.as_str(), "conversation")?;
            if !conversation_ids.insert(node.conversation_id.clone()) {
                return Err(SessionError::Catalog {
                    detail: format!(
                        "conversation {} is mapped by more than one SessionNode",
                        node.conversation_id
                    ),
                });
            }
            if let Some(parent) = &node.parent
                && !session.nodes.contains_key(parent)
            {
                return Err(SessionError::Catalog {
                    detail: format!("node {node_id} points at missing parent {parent}"),
                });
            }
        }
        for node in session.nodes.values() {
            let mut seen = BTreeSet::new();
            let mut current = Some(node.id.clone());
            while let Some(node_id) = current {
                if !seen.insert(node_id.clone()) {
                    return Err(SessionError::Catalog {
                        detail: format!("Session {session_id} graph contains a parent cycle"),
                    });
                }
                current = session
                    .nodes
                    .get(&node_id)
                    .and_then(|parent| parent.parent.clone());
            }
        }
    }
    Ok(())
}

fn validate_active_node(
    session_id: &SessionId,
    session: &PersistedSession,
) -> Result<(), SessionError> {
    if !session.nodes.contains_key(&session.active_node) {
        return Err(SessionError::Catalog {
            detail: format!(
                "session {session_id} selects missing node {}",
                session.active_node
            ),
        });
    }
    Ok(())
}

pub(super) fn validate_id(value: &str, kind: &str) -> Result<(), SessionError> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains('/')
        || value.contains('\\')
        || value.chars().any(char::is_control)
    {
        return Err(SessionError::Catalog {
            detail: format!("{kind} identity is not a safe product id"),
        });
    }
    Ok(())
}

/// Normalizes one user-supplied Session name into the single bounded line a
/// selector row can show. A name that survives this is exactly what the row
/// displays, so no renderer has to defend itself against a name.
pub(super) fn normalize_name(name: &str) -> Result<String, SessionError> {
    let name = single_line(name);
    if name.is_empty() || name.chars().count() > SESSION_NAME_LIMIT {
        return Err(SessionError::InvalidName);
    }
    Ok(name)
}

fn atomic_write(
    path: &Path,
    bytes: &[u8],
    #[cfg(test)] write_fault: &Arc<Mutex<Option<CatalogWriteFault>>>,
) -> Result<(), CatalogCommitError> {
    let parent = path
        .parent()
        .ok_or_else(|| CatalogCommitError::NotCommitted {
            path: path.to_path_buf(),
            detail: "catalog has no parent".to_owned(),
        })?;
    fs::create_dir_all(parent).map_err(|error| CatalogCommitError::NotCommitted {
        path: parent.to_path_buf(),
        detail: error.to_string(),
    })?;
    let temporary = path.with_extension("json.tmp");
    crate::runtime::local_storage::logical_operations::record_catalog_temp_open();
    let mut file = profile_stage!(
        |times: &mut create_profile::StageTimes| &mut times.temp_write_ns,
        OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)
    )
    .map_err(|error| CatalogCommitError::NotCommitted {
        path: temporary.clone(),
        detail: error.to_string(),
    })?;
    // Count the actual logical payload supplied to the catalog write, not a
    // file length sampled before the write. `write_all` either writes every
    // byte or errors, so the supplied payload is the full length.
    let write_result = profile_stage!(
        |times: &mut create_profile::StageTimes| &mut times.temp_write_ns,
        file.write_all(bytes)
    );
    write_result.map_err(|error| CatalogCommitError::NotCommitted {
        path: temporary.clone(),
        detail: error.to_string(),
    })?;
    crate::runtime::local_storage::logical_operations::record_catalog_payload_write(bytes.len());
    crate::runtime::local_storage::logical_operations::record_catalog_file_sync();
    profile_stage!(
        |times: &mut create_profile::StageTimes| &mut times.file_fsync_ns,
        file.sync_all()
    )
    .map_err(|error| CatalogCommitError::NotCommitted {
        path: temporary.clone(),
        detail: error.to_string(),
    })?;
    #[cfg(test)]
    let write_fault = take_write_fault(write_fault);
    #[cfg(test)]
    if write_fault == Some(CatalogWriteFault::BeforeRename) {
        return Err(CatalogCommitError::NotCommitted {
            path: temporary,
            detail: "deterministic fault before catalog visibility rename".to_owned(),
        });
    }
    crate::runtime::local_storage::logical_operations::record_catalog_rename();
    profile_stage!(
        |times: &mut create_profile::StageTimes| &mut times.rename_ns,
        fs::rename(&temporary, path)
    )
    .map_err(|error| CatalogCommitError::NotCommitted {
        path: path.to_path_buf(),
        detail: error.to_string(),
    })?;
    // The rename above is the visibility commit point. Every error after it
    // is therefore a different outcome from a pre-commit write failure.
    #[cfg(test)]
    if write_fault == Some(CatalogWriteFault::AfterRename) {
        return Err(CatalogCommitError::CommittedButDurabilityUncertain {
            path: path.to_path_buf(),
            detail: "deterministic fault after catalog visibility rename".to_owned(),
        });
    }
    profile_stage!(
        |times: &mut create_profile::StageTimes| &mut times.dir_fsync_ns,
        sync_directory(parent)
    )
    .map_err(
        |error| CatalogCommitError::CommittedButDurabilityUncertain {
            path: parent.to_path_buf(),
            detail: error.to_string(),
        },
    )?;
    Ok(())
}

// Preserve fallible publication of the owned directory and post-rename error
// classification. Ancestor power-loss persistence is outside this contract.
pub(super) fn sync_directory(path: &Path) -> std::io::Result<()> {
    crate::runtime::local_storage::logical_operations::record_catalog_directory_sync();
    File::open(path)?.sync_all()
}

/// The result classification of the catalog visibility/durability protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogCommitError {
    /// The visibility rename did not complete. The previous catalog remains
    /// authoritative and the in-memory document is unchanged.
    NotCommitted { path: PathBuf, detail: String },
    /// The visibility rename completed, but the parent-directory durability
    /// barrier did not. The new catalog is visible; durability is uncertain.
    CommittedButDurabilityUncertain { path: PathBuf, detail: String },
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CatalogWriteFault {
    BeforeRename,
    AfterRename,
}

#[cfg(test)]
fn take_write_fault(fault: &Arc<Mutex<Option<CatalogWriteFault>>>) -> Option<CatalogWriteFault> {
    fault
        .lock()
        .expect("catalog write fault lock poisoned")
        .take()
}
