//! Session creation and identity allocation: the private conversation
//! database is prepared and validated before the Catalog visibility commit
//! can name it.

use super::CatalogDocument;
use super::PersistedSession;
use super::RetiredIdentities;
use super::SESSION_CATALOG_SCHEMA_VERSION;
use super::SessionCatalog;
use super::SessionError;
use super::SessionNode;
use super::SessionNodeOrigin;
use super::SessionPersistentState;
use super::artifacts;
#[cfg(test)]
use super::catalog_commit::CatalogWriteFault;
#[cfg(test)]
use super::create_profile;
use super::observation;
use super::uploads;
use crate::durable::CanonicalStore;
use crate::durable::LineageSeed;
use crate::durable::SqliteConversationStore;
use crate::runtime::identity::ConversationId;
use crate::runtime::identity::SessionId;
use crate::runtime::identity::SessionNodeId;
use chrono::Utc;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;

impl SessionCatalog {
    /// Creates and publishes the first root Session with an already-validated
    /// Session-local model.
    ///
    /// Model catalog resolution deliberately does not happen here. The
    /// composition layer owns that authority and must complete it before
    /// calling this mutating Session-domain operation.
    ///
    /// Startup does not take this path: it composes against
    /// [`Self::create_unpublished`] and commits the first catalog write in
    /// the same transaction as every other startup decision. Nothing in
    /// production creates and publishes a first Session in one step, so this
    /// is the test-only shorthand for "a runtime root that has already
    /// launched once".
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] when the private conversation seed or catalog
    /// publication cannot be completed.
    #[cfg(test)]
    pub(crate) fn create(
        runtime_root: &Path,
        state: &SessionPersistentState,
    ) -> Result<Self, SessionError> {
        Self::create_with_identities(
            runtime_root,
            state,
            Arc::new(crate::runtime::identity::SystemUuidV7Generator),
        )
    }

    #[cfg(test)]
    pub(crate) fn create_with_identities(
        runtime_root: &Path,
        state: &SessionPersistentState,
        identities: Arc<dyn crate::runtime::identity::UuidV7Generator>,
    ) -> Result<Self, SessionError> {
        let mut catalog =
            Self::create_unpublished_with_identities(runtime_root, state, identities)?;
        let planned = catalog.plan_unchanged(&catalog.persisted_session_ids()[0]);
        catalog.commit_planned(planned)?;
        Ok(catalog)
    }

    /// Builds the first root Session **without** publishing it.
    ///
    /// The returned catalog names a root Session that `catalog.json` does
    /// not yet mention. That is deliberate: a first launch still has to
    /// compose a runtime for that Session — workspace, capabilities,
    /// recovery, host binding — and every one of those steps can fail. A
    /// catalog written before them leaves a visible, resumable Session
    /// behind a launch that never started, and the next launch resumes into
    /// it.
    ///
    /// The seeded conversation database is not a published fact: a
    /// conversation the catalog does not name is neither selectable nor
    /// resumable, so an abandoned first launch leaves an inert file and
    /// nothing else.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] when the private conversation seed cannot be
    /// created.
    pub(crate) fn create_unpublished(
        runtime_root: &Path,
        state: &SessionPersistentState,
    ) -> Result<Self, SessionError> {
        Self::create_unpublished_with_identities(
            runtime_root,
            state,
            Arc::new(crate::runtime::identity::SystemUuidV7Generator),
        )
    }

    #[allow(clippy::too_many_lines)] // One first-Session reservation and seed transaction.
    pub(super) fn create_unpublished_with_identities(
        runtime_root: &Path,
        state: &SessionPersistentState,
        identities: Arc<dyn crate::runtime::identity::UuidV7Generator>,
    ) -> Result<Self, SessionError> {
        #[cfg(test)]
        {
            super::super::static_effects::observe(super::super::static_effects::Effect::Session);
            super::super::static_effects::observe(super::super::static_effects::Effect::State);
        }
        let product =
            crate::runtime::local_storage::ProductRoot::create(runtime_root).map_err(|error| {
                SessionError::Io {
                    path: runtime_root.to_path_buf(),
                    detail: error.to_string(),
                }
            })?;
        let root = product
            .confined(&product.root().join("sessions"))
            .map_err(|error| SessionError::Io {
                path: product.root().to_path_buf(),
                detail: error.to_string(),
            })?;
        fs::create_dir_all(&root).map_err(|error| SessionError::Io {
            path: root.clone(),
            detail: error.to_string(),
        })?;
        let path = root.join("catalog.json");
        if path.exists() {
            return Err(SessionError::Catalog {
                detail: format!(
                    "cannot create first Session: {} already exists",
                    path.display()
                ),
            });
        }

        let session_id = SessionId::from_uuid(identities.next_uuid())
            .map_err(|detail| SessionError::Catalog { detail })?;
        let node_id = SessionNodeId::from_uuid(identities.next_uuid())
            .map_err(|detail| SessionError::Catalog { detail })?;
        let conversation_id = ConversationId::from_uuid(identities.next_uuid())
            .map_err(|detail| SessionError::Catalog { detail })?;
        // Reserve the identity before any Session or Conversation directory
        // exists, so a consumed identity can never leave a half-prepared
        // allocation behind. A collision is a hard refusal here: this is the
        // first-Session path and the caller supplied the identity.
        product
            .reserve_conversation(&conversation_id)
            .map_err(|error| SessionError::Io {
                path: product.root().join("conversation-reservations"),
                detail: error.to_string(),
            })?;
        crate::runtime::local_storage::logical_operations::record_session_allocation_mkdir();
        fs::create_dir(root.join(session_id.as_str())).map_err(|error| SessionError::Catalog {
            detail: format!("cannot reserve Session identity: {error}"),
        })?;
        let database_path = conversation_database_path(&root, &session_id, &conversation_id);
        initialize_database(
            &product,
            &database_path,
            &conversation_id,
            &LineageSeed::history(Vec::new()),
            None,
        )?;
        let now = Utc::now();
        let node = SessionNode {
            ordinal: 1,
            id: node_id.clone(),
            parent: None,
            conversation_id,
            origin: SessionNodeOrigin::New,
        };
        let mut nodes = BTreeMap::new();
        nodes.insert(node_id.clone(), node);
        let mut sessions = BTreeMap::new();
        sessions.insert(
            session_id.clone(),
            PersistedSession {
                ownership_generation: 0,
                uploads: uploads::UploadRegistry::default(),
                ordinal: 1,
                id: session_id.clone(),
                name: None,
                display_preview: None,
                created_at: now,
                last_prompt_at: None,
                updated_at: now,
                active_node: node_id,
                nodes,
                state: state.clone(),
                settings_revision: 0,
            },
        );
        let document = CatalogDocument {
            retired: RetiredIdentities::default(),
            upload_preparations: BTreeMap::new(),
            deletions: BTreeMap::new(),
            schema_version: SESSION_CATALOG_SCHEMA_VERSION,
            generation: 0,
            next_session_ordinal: 2,
            next_node_ordinal: 2,
            sessions,
        };
        Ok(Self {
            identities,
            product,
            root,
            path,
            node_observation_index: Arc::new(observation::node_index(&document)),
            document,
            published: false,
            lifecycle: None,
            #[cfg(test)]
            write_fault: Arc::new(Mutex::new(None)),
            #[cfg(test)]
            preparation_cleanup_fault: Arc::default(),
            #[cfg(test)]
            classification_gate: None,
            summary_invalidations: Arc::default(),
        })
    }

    /// Arms one deterministic catalog-write fault for the next mutation.
    #[cfg(test)]
    pub(crate) fn arm_write_fault_before_rename(&self) {
        *self
            .write_fault
            .lock()
            .expect("catalog write fault lock poisoned") = Some(CatalogWriteFault::BeforeRename);
    }

    /// Arms one deterministic post-rename durability fault for the next
    /// mutation.
    #[cfg(test)]
    pub(crate) fn arm_write_fault_after_rename(&self) {
        *self
            .write_fault
            .lock()
            .expect("catalog write fault lock poisoned") = Some(CatalogWriteFault::AfterRename);
    }

    /// The catalog generation counter, observed by write-level no-op proofs:
    /// a declined mutation must leave it untouched.
    #[cfg(test)]
    pub(crate) fn document_generation(&self) -> u64 {
        self.document.generation
    }

    pub(super) fn validate_new_node_identity(
        &self,
        node: &SessionNodeId,
        conversation: &ConversationId,
    ) -> Result<(), SessionError> {
        if self.document.retired.nodes.contains(node)
            || self.document.retired.conversations.contains(conversation)
            || self.document.sessions.values().any(|session| {
                session.nodes.contains_key(node)
                    || session
                        .nodes
                        .values()
                        .any(|existing| &existing.conversation_id == conversation)
            })
        {
            return Err(SessionError::Catalog {
                detail: "node or conversation identity already exists".into(),
            });
        }
        Ok(())
    }

    pub(super) fn validate_new_session_identity(
        &self,
        session: &SessionId,
        node: &SessionNodeId,
        conversation: &ConversationId,
    ) -> Result<(), SessionError> {
        if self.document.retired.sessions.contains(session)
            || self.document.sessions.contains_key(session)
        {
            return Err(SessionError::Catalog {
                detail: "Session identity already exists".into(),
            });
        }
        self.validate_new_node_identity(node, conversation)
    }

    /// Inject allocation without making identity an ordering source.
    #[must_use]
    pub fn with_identity_generator(
        mut self,
        identities: Arc<dyn crate::runtime::identity::UuidV7Generator>,
    ) -> Self {
        self.identities = identities;
        self
    }

    /// Create the private Conversation allocation directory for an identity
    /// that the storage owner has already reserved exclusively through
    /// [`crate::runtime::local_storage::ProductRoot::reserve_conversation`].
    ///
    /// This is preparation, not identity allocation: the reservation marker is
    /// the one linearization point and it was written before this call. The
    /// directory itself is created with an exclusive filesystem operation, so a
    /// stale directory left by an earlier failed preparation is never
    /// overwritten and becomes visible as `AlreadyExists`. The caller under an
    /// already-admitted ownership mutation must not reacquire ownership.
    pub(crate) fn create_conversation_allocation(
        product: &crate::runtime::local_storage::ProductRoot,
        allocation: &Path,
    ) -> std::io::Result<()> {
        product.confined(allocation)?;
        Self::check_allocation_live(product, allocation)?;
        crate::runtime::local_storage::logical_operations::record_conversation_allocation_mkdir();
        fs::create_dir_all(
            allocation
                .parent()
                .ok_or_else(|| std::io::Error::other("Conversation directory has no parent"))?,
        )?;
        crate::runtime::local_storage::logical_operations::record_conversation_allocation_mkdir();
        fs::create_dir(allocation)
    }

    pub(super) fn allocate_ids(
        &self,
    ) -> Result<(SessionId, SessionNodeId, ConversationId), SessionError> {
        for _ in 0..16 {
            let session_id = SessionId::from_uuid(self.identities.next_uuid())
                .map_err(|detail| SessionError::Catalog { detail })?;
            let node_id = SessionNodeId::from_uuid(self.identities.next_uuid())
                .map_err(|detail| SessionError::Catalog { detail })?;
            let uuid = self.identities.next_uuid();
            if !crate::runtime::identity::is_uuid_v7(uuid) {
                return Err(SessionError::Catalog {
                    detail: "expected UUIDv7".into(),
                });
            }
            let conversation_id = ConversationId::from_uuid(uuid)
                .map_err(|detail| SessionError::Catalog { detail })?;
            if self
                .reject_pending_identity(&session_id, &node_id, &conversation_id)
                .is_ok()
                && self
                    .validate_new_session_identity(&session_id, &node_id, &conversation_id)
                    .is_ok()
                && !self.document.upload_preparations.contains_key(&session_id)
                && !self.root.join(session_id.as_str()).exists()
            {
                // Identity consumption is owned by local product storage, not
                // by this in-memory graph check. The exclusive reservation is
                // the linearization point; a consumed identity simply selects
                // the next candidate rather than overwriting the prior
                // reservation.
                let reservation = profile_stage!(
                    |times: &mut create_profile::StageTimes| &mut times.reserve_ns,
                    self.product.reserve_conversation(&conversation_id)
                );
                match reservation {
                    Ok(_reservation) => return Ok((session_id, node_id, conversation_id)),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => {
                        return Err(SessionError::Io {
                            path: self.product.root().join("conversation-reservations"),
                            detail: error.to_string(),
                        });
                    }
                }
            }
        }
        Err(SessionError::Catalog {
            detail: "identity reservation exhausted".into(),
        })
    }
}

pub(super) fn next_ordinal(current: u64) -> Result<u64, SessionError> {
    current.checked_add(1).ok_or_else(|| SessionError::Catalog {
        detail: "publication ordinal exhausted".into(),
    })
}

pub(super) fn conversation_database_path(
    root: &Path,
    session_id: &SessionId,
    conversation_id: &ConversationId,
) -> PathBuf {
    root.join(session_id.as_str())
        .join("conversations")
        .join(conversation_id.as_str())
        .join("conversation.sqlite")
}

// One narrow test-only seam: fail the next `count` Conversation
// initialization attempts after the identity reservation and allocation
// directory exist, so R12 can prove an initialization failure leaves no
// visible Session and retains the consumed identity. This is not a generic
// fault framework.
#[cfg(test)]
thread_local! {
    static INITIALIZE_DATABASE_FAILURES: std::cell::Cell<u32> =
        const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn fail_next_database_initializations(count: u32) {
    INITIALIZE_DATABASE_FAILURES.with(|failures| failures.set(count));
}

#[cfg(test)]
fn take_database_initialization_fault() -> bool {
    INITIALIZE_DATABASE_FAILURES.with(|failures| {
        let remaining = failures.get();
        if remaining == 0 {
            false
        } else {
            failures.set(remaining - 1);
            true
        }
    })
}

pub(super) fn initialize_database(
    product: &crate::runtime::local_storage::ProductRoot,
    path: &Path,
    conversation_id: &ConversationId,
    seed: &LineageSeed,
    source_artifacts: Option<&crate::tools::artifacts::ArtifactStore>,
) -> Result<(), SessionError> {
    product.confined(path).map_err(|error| SessionError::Io {
        path: path.to_path_buf(),
        detail: error.to_string(),
    })?;
    let parent = path.parent().ok_or_else(|| SessionError::Io {
        path: path.to_path_buf(),
        detail: "conversation database has no parent".to_owned(),
    })?;
    let _ownership = product
        .ownership_mutation()
        .map_err(|error| SessionError::Io {
            path: parent.to_path_buf(),
            detail: error.to_string(),
        })?;
    // The identity was already reserved by the allocation owner; this only
    // materializes its private allocation directory.
    profile_stage!(
        |times: &mut create_profile::StageTimes| &mut times.allocation_dir_ns,
        SessionCatalog::create_conversation_allocation(product, parent)
    )
    .map_err(|error| SessionError::Io {
        path: parent.to_path_buf(),
        detail: error.to_string(),
    })?;
    #[cfg(test)]
    if take_database_initialization_fault() {
        return Err(SessionError::Io {
            path: path.to_path_buf(),
            detail: "deterministic Conversation initialization fault".to_owned(),
        });
    }
    let access = crate::runtime::local_storage::ConversationAccess::existing(product, parent)
        .map_err(|error| SessionError::Io {
            path: parent.to_path_buf(),
            detail: error.to_string(),
        })?;
    let access = Arc::new(access);
    let materialized = source_artifacts
        .map(|source| artifacts::materialize(source, conversation_id, parent, access.clone(), seed))
        .transpose()?;
    let seed = materialized.as_ref().unwrap_or(seed);
    crate::runtime::local_storage::logical_operations::record_sqlite_store_open_request();
    let store = profile_stage!(
        |times: &mut create_profile::StageTimes| &mut times.sqlite_bootstrap_ns,
        SqliteConversationStore::open(conversation_id.clone(), path)
    )
    .map_err(SessionError::Store)?;
    // LineageSeed contains canonical meaning, Surface history and immutable response provenance.
    // Execution-recovery residue, including a pending unresolved-output
    // carryover source, belongs exclusively to the source conversation and is
    // initialized as NULL in this new destination store.
    profile_stage!(
        |times: &mut create_profile::StageTimes| &mut times.lineage_initialize_ns,
        store.with_lifecycle(access).initialize_lineage(seed)
    )
    .map_err(SessionError::Store)
}
