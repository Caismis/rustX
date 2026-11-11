//! The single conversation root is part of the event identity proof.
//!
//! It also carries the two fixed-size open frontiers: the recovery fold
//! through one Journal sequence, and the Ledger identity validation through
//! one position. Both are advanced in the transaction that appends what they
//! cover, so they share the root row's page instead of dirtying another.
macro_rules! table {
    () => {
        "CREATE TABLE IF NOT EXISTS rustx_store (
                id INTEGER PRIMARY KEY CHECK(id=1),
                schema_version INTEGER NOT NULL,
                conversation_id TEXT NOT NULL,
                next_inbound_sequence INTEGER NOT NULL CHECK(next_inbound_sequence >= 0),
                next_event_sequence INTEGER NOT NULL CHECK(next_event_sequence >= 0),
                next_transcript_position INTEGER NOT NULL CHECK(next_transcript_position >= 0),
                retained_transcript_count INTEGER NOT NULL DEFAULT 0 CHECK(retained_transcript_count >= 0),
                pending_observation_count INTEGER NOT NULL DEFAULT 0 CHECK(pending_observation_count >= 0),
                observation_statistics BLOB CHECK(observation_statistics IS NULL OR length(observation_statistics)<=49152),
                reading_mutation_revision INTEGER NOT NULL DEFAULT 0 CHECK(reading_mutation_revision >= 0),
                logical_step_sequence INTEGER NOT NULL CHECK(logical_step_sequence >= 0),
                pending_unresolved_output_stream_id TEXT,
                bootstrap_initialized INTEGER NOT NULL DEFAULT 0 CHECK(bootstrap_initialized IN (0,1)),
                recovery_frontier BLOB CHECK(recovery_frontier IS NULL OR typeof(recovery_frontier)='blob'),
                canonical_identity_through INTEGER NOT NULL DEFAULT 0 CHECK(canonical_identity_through >= 0),
                canonical_identity_count INTEGER NOT NULL DEFAULT 0 CHECK(canonical_identity_count BETWEEN 0 AND canonical_identity_through),
                surface_revision INTEGER,
                surface_compaction_generation INTEGER,
                CHECK((surface_revision IS NULL) = (surface_compaction_generation IS NULL))
            )"
    };
}
pub(in crate::durable::sqlite) use table;
