//! Native request configuration sharing. The request manifest binds the exact
//! configuration bytes by digest; a transaction's first scalar validation uses
//! the same typed decoder as full historical reconstruction. Later validations
//! reuse that owner only after comparing the current stored bytes exactly.
use super::{
    Arc, Connection, ConversationStoreError, RequestSnapshot, Sha256, Transaction, storage,
};
use crate::context::assembly::AcceptedSystemSection;
use crate::tools::types::ModelToolDefinition;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize, ser::SerializeStruct};
use sha2::Digest;

const MAX_BYTES: usize = 64 * 1024 * 1024;
const REFERENCE_BYTES: usize = b"\"configuration_digest\":\"\",".len() + 64;
const EMPTY_CONFIGURATION_BYTES: usize =
    br#"{"effective_system_prompt":"","system_sections":[],"tool_definitions":[]}"#.len();

// ?3 supplies NORMALIZATION_OVERHEAD. Oversized/invalid metadata never enters
// JSON extraction, and SQLite checks the reconstructed size before Rust owns it.
// A normalized record may add the 90-byte digest reference even for empty config.
pub(super) const NORMALIZATION_OVERHEAD: usize = REFERENCE_BYTES + EMPTY_CONFIGURATION_BYTES;
pub(super) const MATERIALIZED_BYTES_SQL: &str = "CASE WHEN octet_length(snapshot_json)>67108954 THEN 67108865 WHEN json_valid(snapshot_json) THEN octet_length(snapshot_json)+coalesce((SELECT octet_length(configuration_json) FROM request_configurations WHERE digest=json_extract(snapshot_json,'$.configuration_digest')),0)-?3 ELSE octet_length(snapshot_json) END";

#[derive(Serialize)]
struct ConfigurationRef<'a> {
    effective_system_prompt: &'a str,
    system_sections: &'a [AcceptedSystemSection],
    tool_definitions: &'a [ModelToolDefinition],
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Configuration {
    effective_system_prompt: String,
    system_sections: Vec<AcceptedSystemSection>,
    tool_definitions: Vec<ModelToolDefinition>,
}
#[derive(Deserialize)]
struct Metadata {
    configuration_digest: String,
    #[serde(flatten)]
    snapshot: RequestSnapshot,
}

struct MetadataRef<'a> {
    snapshot: &'a RequestSnapshot,
    configuration_digest: &'a str,
}
impl Serialize for MetadataRef<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let s = self.snapshot;
        let fields = 23
            + usize::from(s.continuation.is_some())
            + usize::from(s.unresolved_output_carryover_source.is_some())
            + usize::from(s.unresolved_output_carryover.is_some())
            + usize::from(s.unresolved_output_carryover_anchor.is_some());
        let mut state = serializer.serialize_struct("NativeRequestMetadata", fields)?;
        state.serialize_field("configuration_digest", self.configuration_digest)?;
        macro_rules! field {
            ($field:ident) => {
                state.serialize_field(stringify!($field), &s.$field)?
            };
        }
        field!(tool_associations);
        field!(history_cut);
        state.serialize_field("input", &[] as &[u8])?;
        field!(images);
        field!(semantic_input_fingerprint);
        field!(request_id);
        field!(identity);
        field!(provisional_message_id);
        field!(canonical_frontier);
        state.serialize_field("effective_system_prompt", "")?;
        state.serialize_field("system_sections", &[] as &[u8])?;
        field!(runtime_resource_revision);
        field!(invocation);
        field!(context_window_tokens);
        field!(profile);
        field!(reasoning_enabled);
        state.serialize_field("tool_definitions", &[] as &[u8])?;
        field!(capability_revision);
        field!(context_generation);
        if s.continuation.is_some() {
            field!(continuation);
        }
        field!(continuation_source);
        field!(request_context_ids);
        if s.unresolved_output_carryover_source.is_some() {
            field!(unresolved_output_carryover_source);
        }
        if s.unresolved_output_carryover.is_some() {
            field!(unresolved_output_carryover);
        }
        if s.unresolved_output_carryover_anchor.is_some() {
            field!(unresolved_output_carryover_anchor);
        }
        field!(contributions);
        state.end()
    }
}

struct BoundedBytes {
    bytes: Vec<u8>,
    limit: usize,
}
impl std::io::Write for BoundedBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self
            .bytes
            .len()
            .checked_add(bytes.len())
            .is_none_or(|n| n > self.limit)
        {
            return Err(std::io::Error::other(
                "frozen request exceeds storage byte budget",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn encode(value: &impl Serialize, limit: usize) -> Result<String, ConversationStoreError> {
    let mut output = BoundedBytes {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer(&mut output, value)
        .map_err(|error| storage(format!("native request encoding: {error}")))?;
    String::from_utf8(output.bytes).map_err(|error| storage(error.to_string()))
}
fn digest(bytes: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"rustx-native-request-configuration-v1\0");
    digest.update(bytes.as_bytes());
    format!("{:x}", digest.finalize())
}
fn invalid(detail: &str) -> ConversationStoreError {
    ConversationStoreError::InvalidReference(detail.into())
}
fn materialized_bytes(
    metadata: usize,
    configuration: usize,
) -> Result<usize, ConversationStoreError> {
    metadata
        .checked_sub(REFERENCE_BYTES)
        .and_then(|n| n.checked_add(configuration))
        .and_then(|n| n.checked_sub(EMPTY_CONFIGURATION_BYTES))
        .ok_or_else(|| invalid("frozen request configuration byte geometry"))
}

/// Exact serialized bytes produced from admitted typed values or a successful
/// typed read. This is never accepted by digest alone: each use compares the
/// actual SQLite cell, in the caller's snapshot, as a binary byte string.
pub(super) struct FreshConfiguration {
    digest: String,
    json: String,
}
impl FreshConfiguration {
    fn matches(&self, connection: &Connection) -> Result<Option<bool>, ConversationStoreError> {
        #[cfg(test)]
        CONFIGURATION_BYTE_PROBES.with(|count| count.set(count.get() + 1));
        connection
            .prepare_cached(
                "SELECT typeof(configuration_json)='text' AND octet_length(configuration_json)=?2 AND CAST(configuration_json AS BLOB)=?3 FROM request_configurations WHERE digest=?1",
            )
            .map_err(|error| storage(error.to_string()))?
            .query_row(rusqlite::params![self.digest, self.json.len(), self.json.as_bytes()], |row| row.get(0))
            .optional()
            .map_err(|error| storage(error.to_string()))
    }

    pub(super) fn authenticate(
        &self,
        connection: &Connection,
    ) -> Result<(), ConversationStoreError> {
        if self.matches(connection)? != Some(true) {
            return Err(invalid("frozen request configuration integrity mismatch"));
        }
        Ok(())
    }
}

/// Only the exact transaction borrowed here may reuse this decoded request.
/// Metadata/root/sequence checks remain with each caller; the shared payload
/// must still equal the previously typed-decoded bytes on every reuse. No
/// result or authority survives COMMIT, rollback, or a different transaction.
pub(super) struct Validation<'transaction, 'connection> {
    transaction: &'transaction Transaction<'connection>,
    request: Option<ValidatedRequest>,
}
struct ValidatedRequest {
    metadata: String,
    configuration: FreshConfiguration,
    snapshot: Arc<RequestSnapshot>,
}
impl<'transaction, 'connection> Validation<'transaction, 'connection> {
    pub(super) fn new(transaction: &'transaction Transaction<'connection>) -> Self {
        Self {
            transaction,
            request: None,
        }
    }

    pub(super) fn transaction(&self) -> &'transaction Transaction<'connection> {
        self.transaction
    }

    pub(super) fn metadata(
        &mut self,
        json: &str,
    ) -> Result<Arc<RequestSnapshot>, ConversationStoreError> {
        if let Some(request) = &self.request
            && request.metadata == json
            && request.configuration.matches(self.transaction)? == Some(true)
        {
            return Ok(Arc::clone(&request.snapshot));
        }
        // A changed cell gets the original bounded, digest-authenticated typed
        // read. In particular, a coherently rehashed invalid JSON/type cannot
        // inherit validation from the prior request or configuration digest.
        self.request = None;
        let (mut snapshot, configuration, _) = read(self.transaction, json, MAX_BYTES)?;
        decode_configuration(&mut snapshot, &configuration.json)?;
        let snapshot = Arc::new(snapshot);
        self.request = Some(ValidatedRequest {
            metadata: json.to_owned(),
            configuration,
            snapshot: Arc::clone(&snapshot),
        });
        Ok(snapshot)
    }
}

/// Serializes borrowed data, stores a new configuration only on an exact-byte
/// miss, and returns the small per-request metadata. The enclosing transaction
/// owns rollback of both rows. No selected history or heavy configuration clone.
pub(super) fn persist(
    connection: &Connection,
    snapshot: &RequestSnapshot,
) -> Result<(String, FreshConfiguration), ConversationStoreError> {
    let configuration = encode(
        &ConfigurationRef {
            effective_system_prompt: &snapshot.effective_system_prompt,
            system_sections: &snapshot.system_sections,
            tool_definitions: &snapshot.tool_definitions,
        },
        MAX_BYTES,
    )?;
    let digest = digest(&configuration);
    let json = encode(
        &MetadataRef {
            snapshot,
            configuration_digest: &digest,
        },
        MAX_BYTES + REFERENCE_BYTES,
    )?;
    if materialized_bytes(json.len(), configuration.len())? > MAX_BYTES {
        return Err(invalid("frozen request exceeds storage byte budget"));
    }
    let configuration = FreshConfiguration {
        digest,
        json: configuration,
    };
    match configuration.matches(connection)? {
        Some(true) => {}
        Some(false) => return Err(invalid("frozen request configuration integrity mismatch")),
        None => {
            connection
                .prepare_cached(
                    "INSERT INTO request_configurations(digest,configuration_json) VALUES(?1,?2)",
                )
                .map_err(|error| storage(error.to_string()))?
                .execute(rusqlite::params![configuration.digest, configuration.json])
                .map_err(|error| storage(error.to_string()))?;
        }
    }
    Ok((json, configuration))
}

/// Authenticates the reference and exact shared bytes on every native read.
/// This is deliberately not an unchecked process-local cache: direct SQLite
/// changes and trigger effects remain visible in the caller's transaction.
fn read(
    connection: &Connection,
    json: &str,
    max_bytes: usize,
) -> Result<(RequestSnapshot, FreshConfiguration, usize), ConversationStoreError> {
    let Metadata {
        configuration_digest,
        snapshot,
    } = super::decode(json, "native request metadata")?;
    if configuration_digest.len() != 64
        || !configuration_digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || !snapshot.effective_system_prompt.is_empty()
        || !snapshot.system_sections.is_empty()
        || !snapshot.tool_definitions.is_empty()
        || !snapshot.input.is_empty()
    {
        return Err(invalid(
            "normalized frozen request configuration has an invalid shape",
        ));
    }
    let configuration_limit = max_bytes
        .min(MAX_BYTES)
        .checked_add(NORMALIZATION_OVERHEAD)
        .and_then(|n| n.checked_sub(json.len()))
        .unwrap_or(0);
    let (configuration, length): (Option<String>, usize) = connection.prepare_cached(
        "SELECT CASE WHEN typeof(configuration_json)='text' AND octet_length(configuration_json)<=?2 THEN configuration_json END,octet_length(configuration_json) FROM request_configurations WHERE digest=?1"
    ).map_err(|error| storage(error.to_string()))?
        .query_row(rusqlite::params![configuration_digest, configuration_limit], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional().map_err(|error| storage(error.to_string()))?
        .ok_or_else(|| invalid("frozen request configuration is unavailable"))?;
    let bytes = materialized_bytes(json.len(), length)?;
    if bytes > MAX_BYTES {
        return Err(invalid("frozen request record exceeds the byte budget"));
    }
    if bytes > max_bytes {
        return Err(crate::context::resources::bound(
            "historical frozen request exceeds materialization bound",
        ));
    }
    let configuration = configuration
        .ok_or_else(|| invalid("frozen request configuration has an invalid storage shape"))?;
    #[cfg(test)]
    CONFIGURATION_READ_WORK.with(|work| {
        let (reads, bytes) = work.get();
        work.set((reads + 1, bytes + configuration.len()));
    });
    if digest(&configuration) != configuration_digest {
        return Err(invalid("frozen request configuration integrity mismatch"));
    }
    Ok((
        snapshot,
        FreshConfiguration {
            digest: configuration_digest,
            json: configuration,
        },
        bytes,
    ))
}

pub(super) fn full(
    connection: &Connection,
    json: &str,
    max_bytes: usize,
) -> Result<(RequestSnapshot, usize), ConversationStoreError> {
    let (mut snapshot, configuration, bytes) = read(connection, json, max_bytes)?;
    decode_configuration(&mut snapshot, &configuration.json)?;
    Ok((snapshot, bytes))
}

fn decode_configuration(
    snapshot: &mut RequestSnapshot,
    json: &str,
) -> Result<(), ConversationStoreError> {
    #[cfg(test)]
    CONFIGURATION_DECODES.with(|count| count.set(count.get() + 1));
    let configuration: Configuration = super::decode(json, "native request configuration")?;
    snapshot.effective_system_prompt = configuration.effective_system_prompt;
    snapshot.system_sections = configuration.system_sections;
    snapshot.tool_definitions = configuration.tool_definitions;
    Ok(())
}

#[cfg(test)]
thread_local! {
    pub(super) static CONFIGURATION_READ_WORK: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
    pub(super) static CONFIGURATION_BYTE_PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    pub(super) static CONFIGURATION_DECODES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
