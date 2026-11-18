use super::super::session::{
    LineageSide,
    observation::{
        MessageVersionCut, SessionMessageVersion, SessionMessageVersionFamily,
        SessionMessageVersionsPage,
    },
};
use super::{SessionController, SessionError, SessionId, SessionNodeId, SessionReadAccess};
use crate::durable::{SqliteConversationStore, sqlite::canonical::OrdinaryHumanBoundary};
use crate::observation::content::CanonicalContentLocator;
use std::collections::BTreeMap;

type VersionKey = (SessionNodeId, u64);
type Cuts = BTreeMap<SessionNodeId, MessageVersionCut>;
type Families = BTreeMap<VersionKey, Vec<(VersionKey, SessionMessageVersion)>>;

impl SessionController {
    pub(crate) async fn message_boundary(
        &self,
        session: &SessionId,
        node: SessionNodeId,
        locator: CanonicalContentLocator,
    ) -> Result<Option<SessionMessageVersion>, SessionError> {
        let access = self.acquire_read(session, Some(&node)).await?;
        tokio::task::spawn_blocking(move || {
            let store = open(&access)?;
            Ok(store
                .ordinary_human_boundary(&locator)
                .map_err(SessionError::Store)?
                .map(|boundary| version(&access, &node, boundary)))
        })
        .await
        .map_err(|error| join_error(&error))?
    }

    pub(crate) async fn message_versions(
        &self,
        session: SessionId,
        node: SessionNodeId,
        offset: usize,
        limit: usize,
    ) -> Result<SessionMessageVersionsPage, SessionError> {
        let (access, cuts) = {
            let catalog = self.catalog.lock().await;
            let access = catalog.acquire_read(&session, Some(&node))?;
            (access, catalog.message_version_cuts(&session))
        };
        let catalog = self.catalog.clone();
        tokio::task::spawn_blocking(move || {
            let read =
                |node: &SessionNodeId| catalog.blocking_lock().acquire_read(&session, Some(node));
            let families = families(&cuts, read)?;
            let mut visible = Vec::new();
            for family in families.values() {
                for (index, (key, _)) in family.iter().enumerate() {
                    if owner(&cuts, &node, key.1) == &key.0 {
                        visible.push((key.1, index, family));
                        break;
                    }
                }
            }
            visible.sort_unstable_by_key(|(position, _, _)| *position);
            if offset >= visible.len() {
                return Ok(SessionMessageVersionsPage {
                    versions: Vec::new(),
                    next_offset: None,
                });
            }
            let has_more = visible.len() > offset.saturating_add(limit);
            let store = open_version_store(&access, cuts.get(&node))?;
            let mut versions = Vec::new();
            for (position, index, family) in visible.into_iter().skip(offset).take(limit) {
                let Some(current) = store
                    .ordinary_human_boundary(&CanonicalContentLocator::ByLedgerPosition {
                        position,
                    })
                    .map_err(SessionError::Store)?
                else {
                    return Err(SessionError::Catalog {
                        detail: "Versioned prompt missing from its retained native prefix".into(),
                    });
                };
                versions.push(SessionMessageVersionFamily {
                    current: version(&access, &node, current),
                    current_index: index,
                    total: family.len(),
                    previous: index
                        .checked_sub(1)
                        .map(|previous| family[previous].1.clone()),
                    next: family.get(index + 1).map(|(_, next)| next.clone()),
                });
            }
            Ok(SessionMessageVersionsPage {
                next_offset: has_more.then(|| offset + versions.len()),
                versions,
            })
        })
        .await
        .map_err(|error| join_error(&error))?
    }
}

fn families(
    cuts: &Cuts,
    read: impl Fn(&SessionNodeId) -> Result<SessionReadAccess, SessionError>,
) -> Result<Families, SessionError> {
    let mut before: Vec<_> = cuts
        .iter()
        .filter(|(_, cut)| cut.side == LineageSide::Before)
        .collect();
    before.sort_unstable_by_key(|(_, cut)| cut.ordinal);

    let mut roots = BTreeMap::new();
    let mut families = BTreeMap::<_, Vec<_>>::new();
    for (node, cut) in before {
        let access = read(node)?;
        let Some(replacement) = open_version_store(&access, Some(cut))?
            .first_local_human_boundary()
            .map_err(SessionError::Store)?
        else {
            continue;
        };
        let source_access = read(&cut.source_node)?;
        let Some(boundary) = open_version_store(&source_access, cuts.get(&cut.source_node))?
            .ordinary_human_boundary(&CanonicalContentLocator::ByLedgerPosition {
                position: cut.source_position,
            })
            .map_err(SessionError::Store)?
        else {
            // The existing tree also supports branching at ordinary Agent input.
            continue;
        };
        if boundary.message_id != cut.source_message {
            return Err(SessionError::Catalog {
                detail: "Version source disagrees with its immutable fork origin".into(),
            });
        }
        let source = (
            owner(cuts, &cut.source_node, cut.source_position).clone(),
            cut.source_position,
        );
        let root = roots.get(&source).unwrap_or(&source).clone();
        let family = families
            .entry(root.clone())
            .or_insert_with(|| vec![(source, version(&source_access, &cut.source_node, boundary))]);
        let replacement_key = (node.clone(), replacement.position);
        roots.insert(replacement_key.clone(), root);
        family.push((replacement_key, version(&access, node, replacement)));
    }
    Ok(families)
}

/// Same-Session forks remap message IDs while preserving the canonical prefix's
/// positions. A locally appended prompt keeps the identity of its original node
/// and position even when a later fork inherits it with a new `MessageId`.
fn owner<'a>(cuts: &'a Cuts, node: &'a SessionNodeId, position: u64) -> &'a SessionNodeId {
    let mut owner = node;
    while let Some(cut) = cuts.get(owner) {
        if position > cut.retained_through() {
            break;
        }
        owner = &cut.source_node;
    }
    owner
}

fn open(access: &SessionReadAccess) -> Result<SqliteConversationStore, SessionError> {
    SqliteConversationStore::open_existing(access.conversation_id.clone(), &access.database_path)
        .map(|store| store.with_lifecycle(access.allocation.clone()))
        .map_err(SessionError::Store)
}

fn open_version_store(
    access: &SessionReadAccess,
    cut: Option<&MessageVersionCut>,
) -> Result<SqliteConversationStore, SessionError> {
    let store = open(access)?;
    if let Some(cut) = cut {
        store
            .verify_bootstrap_prefix(cut.retained_through())
            .map_err(SessionError::Store)?;
    }
    Ok(store)
}

fn version(
    access: &SessionReadAccess,
    node: &SessionNodeId,
    boundary: OrdinaryHumanBoundary,
) -> SessionMessageVersion {
    SessionMessageVersion {
        node_id: node.clone(),
        conversation_id: access.conversation_id.clone(),
        message_id: (boundary.message_id.as_str().len() <= 256).then_some(boundary.message_id),
        surface_revision: boundary.surface_revision,
        boundary: CanonicalContentLocator::ByLedgerPosition {
            position: boundary.position,
        },
    }
}

fn join_error(error: &tokio::task::JoinError) -> SessionError {
    SessionError::Catalog {
        detail: error.to_string(),
    }
}
