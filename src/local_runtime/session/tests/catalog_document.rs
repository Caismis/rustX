//! Catalog document schema, Session-owned model metadata, and the visibility
//! commit protocol under injected write faults.

use super::super::SESSION_CATALOG_SCHEMA_VERSION;
use super::*;

// P15
#[test]
fn an_unsupported_catalog_schema_is_refused_without_touching_the_file() {
    for version in [12_u64, u64::from(SESSION_CATALOG_SCHEMA_VERSION) + 100] {
        let (directory, catalog, _config) = open_catalog();
        let path = catalog.path.clone();
        let mut document: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).expect("catalog bytes")).expect("catalog JSON");
        document["schema_version"] = serde_json::json!(version);
        let rewritten = serde_json::to_vec(&document).expect("rewritten catalog");
        fs::write(&path, &rewritten).expect("write rewritten catalog");
        let error = SessionCatalog::open_existing(directory.path())
            .expect_err("an unsupported schema is refused");
        let SessionError::Catalog { detail } = error else {
            panic!("a version mismatch is a catalog error");
        };
        assert!(
            detail.contains("unsupported session catalog schema"),
            "the refusal names the schema: {detail}"
        );
        assert_eq!(
            fs::read(&path).expect("catalog bytes"),
            rewritten,
            "a refused catalog is left byte-identical: no deletion, no rewrite"
        );
    }
}

#[test]
fn accepted_model_configuration_is_catalog_metadata_for_runtime_replacement() {
    let (directory, mut catalog, _config) = open_catalog();
    let mut model = config().initial_model().clone().clone();
    model.model =
        serde_json::from_value(serde_json::json!("provider/next-model")).expect("model reference");
    catalog
        .persist_model(&first_session(&catalog), model.clone())
        .expect("persist model metadata");

    let reopened = reopen_catalog(directory.path());
    let (_, _, reopened_state) = reopened
        .lineage(&first_session(&catalog), None)
        .map(|(node, state)| (first_session(&catalog), node, state))
        .expect("active lineage");
    assert_eq!(reopened_state.model, Some(model));
}

#[test]
fn resume_keeps_explicit_session_model_but_new_catalog_uses_current_default() {
    let directory = tempfile::tempdir().expect("temporary root");
    let first = config().initial_model().clone();
    let mut explicit = first.clone();
    explicit.model = serde_json::from_value(serde_json::json!("provider/explicit"))
        .expect("explicit model reference");
    let mut current = first.clone();
    current.model = serde_json::from_value(serde_json::json!("provider/current"))
        .expect("current model reference");

    let mut catalog = SessionCatalog::create(
        directory.path(),
        &SessionPersistentState {
            model: Some(first.clone()),
            ..SessionPersistentState::from_input(&crate::local_runtime::SessionConfigInput::new(
                std::path::PathBuf::from("/"),
            ))
        },
    )
    .expect("catalog");
    catalog
        .persist_model(&first_session(&catalog), explicit.clone())
        .expect("persist explicit Session model");
    let reopened = reopen_catalog(directory.path());
    let (_, _, resumed) = reopened
        .lineage(&first_session(&catalog), None)
        .map(|(node, state)| (first_session(&catalog), node, state))
        .expect("resumed lineage");
    assert_eq!(resumed.model, Some(explicit));

    let new_directory = tempfile::tempdir().expect("new Session root");
    let fresh = SessionCatalog::create(
        new_directory.path(),
        &SessionPersistentState {
            model: Some(current.clone()),
            ..SessionPersistentState::from_input(&crate::local_runtime::SessionConfigInput::new(
                std::path::PathBuf::from("/"),
            ))
        },
    )
    .expect("new catalog");
    let (_, _, fresh_state) = fresh
        .lineage(&first_session(&fresh), None)
        .map(|(node, state)| (first_session(&fresh), node, state))
        .expect("fresh lineage");
    assert_eq!(fresh_state.model, Some(current));
}

/// The lineage provenance promise moved, so the persisted version that
/// gates it moved with it.
///
/// A catalog written before the promise decodes perfectly: same fields,
/// same `Clone` origin, same destination database. What differs is only
/// what the destination's retained history *means* — the older seed
/// flattened the source Surface into one append per active message, so
/// its recorded branch points are artefacts of the copy. Nothing in the
/// record distinguishes that from a genuinely copied history, so this
/// code must refuse the document rather than read a lineage it would then
/// fork at the wrong boundaries. The published clone below is what such a
/// catalog holds; only its version is put back.
#[test]
fn a_pre_provenance_session_catalog_is_refused_rather_than_reinterpreted() {
    let (directory, mut catalog, _config) = open_catalog();
    let history = source_history();
    let (source_conversation, source_session, source_node) = append_history(&catalog, &history);
    let source_store = store_for(&catalog, &source_session, &source_conversation);
    let revision = source_store.load_head().expect("source head").revision;
    let source = lineage_at(&source_store, &source_conversation, revision);
    let clone = catalog
        .prepare_clone_session(&state(), &source)
        .expect("prepare clone");
    catalog
        .publish_session(
            &clone,
            SessionNodeOrigin::Clone {
                source_session,
                source_node,
                source_surface_revision: revision,
            },
        )
        .expect("publish clone");

    // Everything about the document stays as it is; only the version goes
    // back to the one written before copies retained their provenance.
    let path = catalog.path.clone();
    let mut document: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).expect("catalog bytes")).expect("catalog JSON");
    assert_eq!(
        document["schema_version"],
        serde_json::json!(SESSION_CATALOG_SCHEMA_VERSION),
        "the published catalog carries the current schema"
    );
    document["schema_version"] = serde_json::json!(SESSION_CATALOG_SCHEMA_VERSION - 1);
    fs::write(
        &path,
        serde_json::to_vec(&document).expect("downgraded catalog"),
    )
    .expect("write downgraded catalog");

    let error = SessionCatalog::open_existing(directory.path())
        .expect_err("a pre-provenance catalog must not open");
    let SessionError::Catalog { detail } = error else {
        panic!("a version mismatch is a catalog error");
    };
    assert!(
        detail.contains("unsupported session catalog schema"),
        "the refusal names the schema, not some downstream symptom: {detail}"
    );
}

#[test]
fn catalog_serialization_contains_no_current_runtime_configuration() {
    let directory = tempfile::tempdir().expect("temporary root");
    let state = state();
    let catalog = SessionCatalog::create(directory.path(), &state).expect("catalog");
    let bytes = fs::read(&catalog.path).expect("catalog bytes");
    let json = String::from_utf8(bytes).expect("catalog UTF-8");
    assert!(json.contains("\"state\""));
    assert!(json.contains("\"model\""));
    for forbidden in [
        "agent_id",
        "timezone",
        "context",
        "mcp_servers",
        "native_tools",
        "approval_mode",
        "approvalMode",
        "environment",
        "agent.tools",
    ] {
        assert!(
            !json.contains(forbidden),
            "durable Session state must not persist current runtime field {forbidden:?}"
        );
    }
}

#[test]
fn catalog_creation_persists_only_the_supplied_session_state() {
    let directory = tempfile::tempdir().expect("temporary root");
    let state = state();
    let catalog = SessionCatalog::create(directory.path(), &state).expect("catalog");

    let (_, _, persisted) = catalog
        .lineage(&first_session(&catalog), None)
        .map(|(node, state)| (first_session(&catalog), node, state))
        .expect("active lineage");
    assert_eq!(persisted, state);
}

#[test]
fn catalog_fault_before_rename_keeps_memory_and_file_on_old_document() {
    let (directory, mut catalog, _config) = open_catalog();
    let before = catalog
        .snapshot(&first_session(&catalog))
        .expect("initial snapshot");

    catalog.arm_write_fault_before_rename();
    let error = catalog
        .rename(&before.id, "not published")
        .expect_err("the deterministic pre-rename fault must fail");
    assert!(matches!(
        error,
        SessionError::CatalogCommit {
            error: CatalogCommitError::NotCommitted { .. }
        }
    ));
    assert_eq!(
        catalog
            .snapshot(&first_session(&catalog))
            .expect("in-memory snapshot"),
        before,
        "a pre-commit failure leaves the in-process document unchanged"
    );
    let reopened = reopen_catalog(directory.path());
    assert_eq!(
        reopened
            .snapshot(&first_session(&catalog))
            .expect("reopened snapshot"),
        before
    );

    // The failed metadata mutation did not poison the catalog: absent
    // runtime quiescence, the same attachment remains usable.
    let retried = catalog
        .rename(&before.id, "published after retry")
        .expect("ordinary metadata remains usable after pre-commit failure");
    assert_eq!(retried.name.as_deref(), Some("published after retry"));
}

#[test]
fn catalog_fault_after_rename_keeps_memory_coherent_and_reports_uncertain_durability() {
    let (directory, mut catalog, _config) = open_catalog();
    let before = catalog
        .snapshot(&first_session(&catalog))
        .expect("initial snapshot");

    catalog.arm_write_fault_after_rename();
    let error = catalog
        .rename(&before.id, "visible but uncertain")
        .expect_err("the deterministic post-rename fault must fail");
    assert!(matches!(
        error,
        SessionError::CatalogCommit {
            error: CatalogCommitError::CommittedButDurabilityUncertain { .. }
        }
    ));
    let in_memory = catalog
        .snapshot(&first_session(&catalog))
        .expect("committed in-memory snapshot");
    assert_eq!(in_memory.name.as_deref(), Some("visible but uncertain"));
    let reopened = reopen_catalog(directory.path());
    assert_eq!(
        reopened
            .snapshot(&first_session(&catalog))
            .expect("reopened snapshot")
            .name
            .as_deref(),
        Some("visible but uncertain"),
        "the file crossed the visibility commit point even though durability was uncertain"
    );
}

// Issue #386: the post-commit summary invalidation follows the Catalog
// *visibility* point, not the durability barrier. A pre-visibility failure
// announces nothing; a visible-but-durability-uncertain publication is a
// real metadata change and must not be lost merely because the API
// returned `Err`.
#[test]
fn projection_invalidation_follows_visibility_not_the_durability_barrier() {
    let (_directory, mut catalog, _config) = open_catalog();
    let id = first_session(&catalog);
    let invalidations = catalog.summary_invalidations();
    let recorded = invalidations.recorded();

    catalog.arm_write_fault_before_rename();
    assert!(
        catalog
            .publish_display_preview(&id, "never became visible")
            .is_err()
    );
    assert_eq!(
        invalidations.recorded(),
        recorded,
        "a pre-visibility failure announces no metadata change"
    );
    assert_eq!(catalog.summary(&id).expect("summary").preview, None);

    catalog.arm_write_fault_after_rename();
    let error = catalog
        .publish_display_preview(&id, "visible but uncertain")
        .expect_err("the deterministic post-rename fault must fail");
    assert!(
        error.committed(),
        "the projection crossed the visibility commit point"
    );
    assert_eq!(
        catalog.summary(&id).expect("summary").preview.as_deref(),
        Some("visible but uncertain")
    );
    assert_eq!(
        invalidations.recorded(),
        recorded + 1,
        "a visible metadata change is announced even when durability is uncertain"
    );
}

#[test]
fn issue419_creation_invalidation_tracks_visibility_even_with_durability_diagnostic() {
    let (_directory, mut catalog, _config) = open_catalog();
    let log = catalog.summary_invalidations();
    let frontier = log.frontier();
    let prepared = catalog.prepare_session(&state(), &[]).unwrap();
    catalog.arm_write_fault_before_rename();
    assert!(
        catalog
            .publish_session(&prepared, SessionNodeOrigin::New)
            .is_err()
    );
    assert_eq!(log.next_after(frontier), None);
    let prepared = catalog.prepare_session(&state(), &[]).unwrap();
    catalog.arm_write_fault_after_rename();
    assert!(
        catalog
            .publish_session(&prepared, SessionNodeOrigin::New)
            .unwrap_err()
            .committed()
    );
    assert_eq!(log.next_after(frontier).unwrap().1, prepared.session_id);
    assert!(log.next_after(frontier).unwrap().2);
}
