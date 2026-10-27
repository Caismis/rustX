//! Native request configuration sharing. The request manifest binds the exact
//! configuration bytes by digest; scalar validators authenticate those bytes
//! using the same typed decoder as full historical reconstruction.
use super::{Connection, ConversationStoreError, RequestSnapshot, Sha256, storage};
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

/// Serializes borrowed data, stores a new configuration only on an exact-byte
/// miss, and returns the small per-request metadata. The enclosing transaction
/// owns rollback of both rows. No selected history or heavy configuration clone.
pub(super) fn persist(
    connection: &Connection,
    snapshot: &RequestSnapshot,
) -> Result<String, ConversationStoreError> {
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
    let existing: Option<Option<String>> = connection.prepare_cached(
        "SELECT CASE WHEN typeof(configuration_json)='text' AND octet_length(configuration_json)<=67108864 THEN configuration_json END FROM request_configurations WHERE digest=?1"
    ).map_err(|error| storage(error.to_string()))?
        .query_row([&digest], |row| row.get(0)).optional().map_err(|error| storage(error.to_string()))?;
    match existing {
        Some(Some(existing)) if existing == configuration => {}
        Some(_) => return Err(invalid("frozen request configuration integrity mismatch")),
        None => {
            connection
                .prepare_cached(
                    "INSERT INTO request_configurations(digest,configuration_json) VALUES(?1,?2)",
                )
                .map_err(|error| storage(error.to_string()))?
                .execute(rusqlite::params![digest, configuration])
                .map_err(|error| storage(error.to_string()))?;
        }
    }
    Ok(json)
}

/// Authenticates the reference and exact shared bytes on every native read.
/// This is deliberately not an unchecked process-local cache: direct SQLite
/// changes and trigger effects remain visible in the caller's transaction.
fn read(
    connection: &Connection,
    json: &str,
    max_bytes: usize,
) -> Result<(RequestSnapshot, String, usize), ConversationStoreError> {
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
    if digest(&configuration) != configuration_digest {
        return Err(invalid("frozen request configuration integrity mismatch"));
    }
    Ok((snapshot, configuration, bytes))
}

/// A fresh start already owns the exact admitted typed values. Its immutable
/// byte digest proves those values without decoding the shared payload again.
pub(super) fn authenticate(
    connection: &Connection,
    json: &str,
) -> Result<(), ConversationStoreError> {
    read(connection, json, MAX_BYTES).map(|_| ())
}

pub(super) fn metadata(
    connection: &Connection,
    json: &str,
) -> Result<RequestSnapshot, ConversationStoreError> {
    full(connection, json, MAX_BYTES).map(|(snapshot, _)| snapshot)
}
pub(super) fn full(
    connection: &Connection,
    json: &str,
    max_bytes: usize,
) -> Result<(RequestSnapshot, usize), ConversationStoreError> {
    let (mut snapshot, json, bytes) = read(connection, json, max_bytes)?;
    #[cfg(test)]
    CONFIGURATION_DECODES.with(|count| count.set(count.get() + 1));
    let configuration: Configuration = super::decode(&json, "native request configuration")?;
    snapshot.effective_system_prompt = configuration.effective_system_prompt;
    snapshot.system_sections = configuration.system_sections;
    snapshot.tool_definitions = configuration.tool_definitions;
    Ok((snapshot, bytes))
}

#[cfg(test)]
thread_local! {
    pub(super) static CONFIGURATION_DECODES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
