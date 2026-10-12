//! Deterministic, deletion-first repair of the two `rustx.toml` sources
//! (Issue #464).
//!
//! ```text
//! bound source bytes (one exact revision)
//!   -> editable toml_edit document            syntax error: unresolved, unchanged
//!   -> eligible edits, one finite pass        schema-unknown fields/tables,
//!                                             explicit schema_version,
//!                                             illegal agent.tools.builtin entries
//!   -> candidate validation                   production strict source parser,
//!                                             source restrictions, native semantics
//!   -> backup, CAS, staged atomic publication  or the source stays unchanged
//!   -> one bounded, redacted receipt           process-local ledger
//! ```
//!
//! The strict parser is never weakened: repair rewrites a small, precisely
//! recognizable set of authored constructs and then requires the candidate to
//! pass the same native validation every other consumer applies. Eligibility
//! comes from the generated source schema of [`RuntimeLayer`] — closed objects
//! (`additionalProperties: false`) versus declared open maps — and from the
//! native Tool selection domain, never from a hand-written field or Tool list.
//! Open map identities (Providers, Models, Profiles, sources, environment,
//! MCP policy identities) and the opaque `request_params` JSON string are not
//! schema fields and are never entered.
//!
//! A committed repair proves only that authored bytes changed. Runtime
//! application and Session adoption stay with the configuration coordinator.

use super::super::settings::SourceDirectory;
use super::UserConfigManager;
use super::settings::{SettingsError, SourceScope, SourceTarget};
use crate::local_runtime::authoring::RuntimeLayer;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use toml_edit::{Array, DocumentMut, Item, TableLike, Value as TomlValue};

/// Receipt entries retained per source; further changes only count.
pub const MAX_RECEIPT_CHANGES: usize = 64;
/// Characters retained per rendered key segment or Tool name.
const MAX_SEGMENT_CHARS: usize = 64;
/// Characters retained per rendered field path.
const MAX_PATH_CHARS: usize = 256;
/// Characters retained per receipt diagnostic.
const MAX_DIAGNOSTIC_CHARS: usize = 512;
/// Workspace sources the process-local ledger remembers; User is always kept.
const MAX_LEDGER_WORKSPACES: usize = 64;
/// Private directory below the bound runtime root holding original bytes.
pub(crate) const BACKUP_DIRECTORY: &str = "configuration-backups";

/// Why a Native Tool selection entry was removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NativeToolRemoval {
    /// No native Tool of the root selection plane has this name.
    Unknown,
    /// A later repeat of an ordinary selection; the first occurrence stays.
    Duplicate,
    /// An Agent control Tool composed automatically when agents are admitted.
    AgentControl,
    /// A Tool owned by a Plugin and selected through `agent.plugins`.
    PluginOwned,
}

/// One deterministic edit of an authored source. Never carries a value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConfigRepairChange {
    /// A field or table unknown to its closed schema object was deleted.
    RemovedField { field_path: String },
    /// An illegal `agent.tools.builtin` element was deleted.
    RemovedNativeTool {
        name: String,
        reason: NativeToolRemoval,
    },
    /// An explicit `schema_version` was normalized to the current version.
    UpdatedSchemaVersion { from: i64, to: u32 },
}

/// The outcome of one automatic repair observation. A healthy unchanged
/// source has no receipt at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConfigRepairStatus {
    /// A validated candidate was atomically published.
    Repaired,
    /// The source is invalid and no valid candidate exists; it is unchanged.
    Unresolved,
    /// Publication may have happened but was not durably confirmed. The
    /// observed revision is reported; the mutation is never replayed.
    CommitUncertain,
}

/// Bounded, redacted native fact about one automatic repair of one source.
///
/// It describes authored bytes only: it never claims that any configuration
/// was prepared, applied or adopted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigRepairReceipt {
    /// Content identity of this observation; equal receipts are one repair.
    pub id: String,
    pub target: SourceTarget,
    pub status: ConfigRepairStatus,
    /// The exact source revision the repair was planned from.
    pub before_revision: String,
    /// The committed revision, or the revision observed after an uncertain
    /// publication. Absent when nothing was published.
    pub after_revision: Option<String>,
    /// Every eligible change, including those beyond `changes`.
    pub changed_count: u32,
    pub changes: Vec<ConfigRepairChange>,
    /// Whether `changes` omits entries beyond [`MAX_RECEIPT_CHANGES`].
    pub truncated: bool,
    /// Whether an exact private copy of the original bytes exists.
    pub backup_available: bool,
    /// Why the source is unresolved or uncertain; never source excerpts.
    pub diagnostic: Option<String>,
}

/// What one automatic repair observation did to its source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RepairOutcome {
    /// Nothing was written. A receipt may have been recorded or cleared.
    Unchanged,
    /// Bytes were (or may have been) published; the coordinator owns them now.
    Committed,
    /// The source changed during the repair; nothing was written or recorded.
    Superseded,
}

/// The composition one repair boundary serves, admitted against the storage
/// isolation contract before any of its source transactions begins.
///
/// Process startup repairs the bound User source before any Workspace is
/// selected: its scope is the User alone ([`RepairScope::user`]), and nothing
/// can be proven about Workspaces not yet chosen. A Session or reconciliation
/// boundary knows its selected Workspace; [`UserConfigManager::admit_repair_scope`]
/// admits that Workspace's physical directory once, and every transaction of
/// the boundary — the User source as much as the Workspace source — uses that
/// one handle: the Workspace source is published through it, and every
/// backup is checked against it.
pub(crate) struct RepairScope {
    target: SourceTarget,
    workspace: Option<SourceDirectory>,
}

impl RepairScope {
    /// The User alone, at process startup.
    pub(crate) const fn user() -> Self {
        Self {
            target: SourceTarget::User,
            workspace: None,
        }
    }

    pub(crate) const fn target(&self) -> &SourceTarget {
        &self.target
    }
}

/// Whether admitted runtime storage and an admitted Workspace are physically
/// disjoint: neither directory lies within the other, followed by `..` from
/// the handles. Unknown is not disjoint.
fn isolated(root: &SourceDirectory, workspace: &SourceDirectory) -> bool {
    !root.within(workspace).unwrap_or(true) && !workspace.within(root).unwrap_or(true)
}

/// One finite repair plan of exact source bytes.
#[derive(Debug)]
pub(crate) struct RepairPlan {
    pub(crate) changes: Vec<ConfigRepairChange>,
    pub(crate) candidate: Vec<u8>,
}

/// Why no editable document exists. Never carries authored text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Uneditable {
    NotUtf8,
    Syntax {
        line: Option<usize>,
        column: Option<usize>,
    },
}

impl Uneditable {
    fn diagnostic(self) -> String {
        match self {
            Self::NotUtf8 => {
                "rustx.toml is not valid UTF-8; it cannot be repaired automatically".into()
            }
            Self::Syntax { line, column } => format!(
                "malformed TOML at line {}, column {}; syntax is never repaired automatically",
                line.unwrap_or(1),
                column.unwrap_or(1)
            ),
        }
    }
}

/// The generated source schema of [`RuntimeLayer`], computed once.
fn source_schema() -> &'static Value {
    static SCHEMA: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    SCHEMA.get_or_init(super::authoring_schema)
}

/// Plan every eligible deterministic edit of `bytes` in one finite pass.
///
/// The plan is structural only; [`UserConfigManager::repair_source`] decides
/// whether the candidate is valid enough to publish.
pub(crate) fn plan(bytes: &[u8]) -> Result<RepairPlan, Uneditable> {
    let text = std::str::from_utf8(bytes).map_err(|_| Uneditable::NotUtf8)?;
    let mut document: DocumentMut = text.parse().map_err(|error: toml_edit::TomlError| {
        let prefix = error.span().and_then(|span| text.get(..span.start));
        Uneditable::Syntax {
            line: prefix.map(|s| s.bytes().filter(|b| *b == b'\n').count() + 1),
            column: prefix.map(|s| s.rsplit('\n').next().unwrap_or_default().chars().count() + 1),
        }
    })?;
    let mut changes = Vec::new();
    normalize_schema_version(&mut document, &mut changes);
    let mut walker = Walker {
        root: source_schema(),
        path: Vec::new(),
        changes: Vec::new(),
    };
    walker.walk_table(walker.root, document.as_table_mut());
    changes.append(&mut walker.changes);
    repair_builtin_selection(&mut document, &mut changes);
    let candidate = if changes.is_empty() {
        bytes.to_vec()
    } else {
        document.to_string().into_bytes()
    };
    Ok(RepairPlan { changes, candidate })
}

/// The typed `RuntimeLayer::schema_version` field.
const SCHEMA_VERSION_FIELD: &str = "schema_version";
/// The typed root profile path of `ToolsLayer::builtin`.
const BUILTIN_SELECTION_PATH: [&str; 3] = ["agent", "tools", "builtin"];

fn normalize_schema_version(document: &mut DocumentMut, changes: &mut Vec<ConfigRepairChange>) {
    let current = super::super::config::CURRENT_RUNTIME_SCHEMA_VERSION;
    let Some(value) = document
        .as_table_mut()
        .get_mut(SCHEMA_VERSION_FIELD)
        .and_then(Item::as_value_mut)
    else {
        return;
    };
    // Only an explicit integer is version metadata; another shape is a
    // known-field type error and stays for strict validation to reject.
    let Some(from) = value.as_integer() else {
        return;
    };
    if from == i64::from(current) {
        return;
    }
    let decor = value.decor().clone();
    *value = TomlValue::from(i64::from(current));
    *value.decor_mut() = decor;
    changes.push(ConfigRepairChange::UpdatedSchemaVersion { from, to: current });
}

fn repair_builtin_selection(document: &mut DocumentMut, changes: &mut Vec<ConfigRepairChange>) {
    use crate::capabilities::selection::{BuiltinSelectionClass, classify_builtin_selection};
    let [agent, tools, builtin] = BUILTIN_SELECTION_PATH;
    let Some(array) = document
        .as_table_mut()
        .get_mut(agent)
        .and_then(Item::as_table_like_mut)
        .and_then(|table| table.get_mut(tools))
        .and_then(Item::as_table_like_mut)
        .and_then(|table| table.get_mut(builtin))
        .and_then(Item::as_array_mut)
    else {
        return;
    };
    // Only a list of strings is a selection; another element shape is a
    // known-field type error and stays for strict validation to reject.
    let Some(names) = array
        .iter()
        .map(|value| value.as_str().map(str::to_owned))
        .collect::<Option<Vec<_>>>()
    else {
        return;
    };
    let mut selected = BTreeSet::new();
    let mut removed = Vec::new();
    for (index, name) in names.iter().enumerate() {
        let reason = match classify_builtin_selection(name) {
            BuiltinSelectionClass::Ordinary if selected.insert(name.as_str()) => continue,
            BuiltinSelectionClass::Ordinary => NativeToolRemoval::Duplicate,
            BuiltinSelectionClass::AgentControl => NativeToolRemoval::AgentControl,
            BuiltinSelectionClass::PluginOwned(_) => NativeToolRemoval::PluginOwned,
            BuiltinSelectionClass::Unknown => NativeToolRemoval::Unknown,
        };
        removed.push(index);
        changes.push(ConfigRepairChange::RemovedNativeTool {
            name: bounded(name, MAX_SEGMENT_CHARS),
            reason,
        });
    }
    for index in removed.into_iter().rev() {
        remove_element(array, index);
    }
}

/// Remove one array element without disturbing its neighbours' layout.
///
/// A multi-line element's prefix starts with the previous element's trailing
/// comment, and its successor's prefix starts with the removed element's own
/// trailing comment. The successor (or the closing trailer) therefore keeps
/// the previous line's comment and its own following lines, and drops only
/// the comment of the removed element.
fn remove_element(array: &mut Array, index: usize) {
    fn raw(decor: Option<&toml_edit::RawString>) -> String {
        decor
            .and_then(toml_edit::RawString::as_str)
            .unwrap_or_default()
            .to_owned()
    }
    let removed = raw(array.remove(index).decor().prefix());
    let rejoin = |successor: &str| match (removed.split_once('\n'), successor.split_once('\n')) {
        (Some((previous, _)), Some((_, following))) => format!("{previous}\n{following}"),
        _ => removed.clone(),
    };
    if let Some(next) = array.get_mut(index) {
        let prefix = rejoin(&raw(next.decor().prefix()));
        next.decor_mut().set_prefix(prefix);
    } else if removed.contains('\n') {
        let trailing = array.trailing().as_str().unwrap_or_default().to_owned();
        if trailing.contains('\n') {
            array.set_trailing(rejoin(&trailing));
        }
    }
}

/// The TOML shape of one authored node, as the schema distinguishes it.
enum Shape<'a> {
    Table(&'a dyn TableLike),
    Array,
    String,
    Integer,
    Float,
    Boolean,
    Datetime,
}

impl Shape<'_> {
    fn json_types(&self) -> &'static [&'static str] {
        match self {
            Self::Table(_) => &["object"],
            Self::Array => &["array"],
            Self::String => &["string"],
            Self::Integer => &["integer", "number"],
            Self::Float => &["number"],
            Self::Boolean => &["boolean"],
            Self::Datetime => &[],
        }
    }
}

fn value_shape(value: &TomlValue) -> Shape<'_> {
    match value {
        TomlValue::String(_) => Shape::String,
        TomlValue::Integer(_) => Shape::Integer,
        TomlValue::Float(_) => Shape::Float,
        TomlValue::Boolean(_) => Shape::Boolean,
        TomlValue::Datetime(_) => Shape::Datetime,
        TomlValue::Array(_) => Shape::Array,
        TomlValue::InlineTable(table) => Shape::Table(table),
    }
}

/// One schema-directed traversal. It only ever deletes keys of closed
/// objects; open maps, unions it cannot select, and scalars are left alone.
struct Walker<'s> {
    root: &'s Value,
    path: Vec<String>,
    changes: Vec<ConfigRepairChange>,
}

impl<'s> Walker<'s> {
    /// Follow local `$ref`s to the definition they name.
    fn resolve(&self, mut node: &'s Value) -> &'s Value {
        for _ in 0..32 {
            let Some(name) = node
                .get("$ref")
                .and_then(Value::as_str)
                .and_then(|reference| reference.strip_prefix("#/$defs/"))
            else {
                break;
            };
            match self.root.get("$defs").and_then(|defs| defs.get(name)) {
                Some(target) => node = target,
                None => break,
            }
        }
        node
    }

    /// The single schema alternative admitting `shape`, or nothing when no
    /// alternative or more than one does.
    fn select(&self, node: &'s Value, shape: &Shape<'_>) -> Option<&'s Value> {
        let node = self.resolve(node);
        let Some(branches) = node
            .get("anyOf")
            .or_else(|| node.get("oneOf"))
            .and_then(Value::as_array)
        else {
            return self.admits(node, shape).then_some(node);
        };
        let mut matches = branches
            .iter()
            .filter_map(|branch| self.select(branch, shape));
        let selected = matches.next()?;
        matches.next().is_none().then_some(selected)
    }

    fn admits(&self, node: &'s Value, shape: &Shape<'_>) -> bool {
        let declared: Vec<&str> = match node.get("type") {
            Some(Value::String(kind)) => vec![kind.as_str()],
            Some(Value::Array(kinds)) => kinds.iter().filter_map(Value::as_str).collect(),
            _ if node.get("const").is_some() || node.get("enum").is_some() => {
                let values: Vec<&Value> = node
                    .get("const")
                    .into_iter()
                    .chain(
                        node.get("enum")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten(),
                    )
                    .collect();
                values
                    .into_iter()
                    .map(|value| match value {
                        Value::String(_) => "string",
                        Value::Bool(_) => "boolean",
                        Value::Number(_) => "number",
                        _ => "null",
                    })
                    .collect()
            }
            _ if node.get("properties").is_some() || node.get("additionalProperties").is_some() => {
                vec!["object"]
            }
            _ if node.get("items").is_some() => vec!["array"],
            _ => return true,
        };
        if !shape
            .json_types()
            .iter()
            .any(|kind| declared.contains(kind))
        {
            return false;
        }
        // A tagged object alternative admits exactly the tables naming its tag.
        let Shape::Table(table) = shape else {
            return true;
        };
        let required: Vec<&str> = node
            .get("required")
            .and_then(Value::as_array)
            .map(|names| names.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        node.get("properties")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .filter(|(name, _)| required.contains(&name.as_str()))
            .filter_map(|(name, property)| {
                self.resolve(property)
                    .get("const")
                    .and_then(Value::as_str)
                    .map(|tag| (name, tag))
            })
            .all(|(name, tag)| {
                table
                    .get(name)
                    .and_then(Item::as_str)
                    .is_some_and(|authored| authored == tag)
            })
    }

    fn walk_item(&mut self, schema: &'s Value, item: &mut Item) {
        match item {
            Item::Table(table) => self.walk_table(schema, table),
            Item::ArrayOfTables(tables) => {
                let Some(items) = self
                    .select(schema, &Shape::Array)
                    .and_then(|array| array.get("items"))
                else {
                    return;
                };
                for (index, table) in tables.iter_mut().enumerate() {
                    self.path.push(format!("[{index}]"));
                    self.walk_table(items, table);
                    self.path.pop();
                }
            }
            Item::Value(value) => self.walk_value(schema, value),
            Item::None => {}
        }
    }

    fn walk_value(&mut self, schema: &'s Value, value: &mut TomlValue) {
        match value {
            TomlValue::InlineTable(table) => self.walk_table(schema, table),
            TomlValue::Array(array) => {
                let Some(items) = self
                    .select(schema, &Shape::Array)
                    .and_then(|array| array.get("items"))
                else {
                    return;
                };
                for (index, element) in array.iter_mut().enumerate() {
                    if matches!(value_shape(element), Shape::Table(_) | Shape::Array) {
                        self.path.push(format!("[{index}]"));
                        self.walk_value(items, element);
                        self.path.pop();
                    }
                }
            }
            _ => {}
        }
    }

    fn walk_table(&mut self, schema: &'s Value, table: &mut dyn TableLike) {
        let Some(schema) = self.select(schema, &Shape::Table(table)) else {
            return;
        };
        match schema.get("additionalProperties") {
            Some(Value::Bool(false)) => {
                let properties = schema.get("properties").and_then(Value::as_object);
                let unknown: Vec<String> = table
                    .iter()
                    .map(|(key, _)| key.to_owned())
                    .filter(|key| !properties.is_some_and(|known| known.contains_key(key)))
                    .collect();
                for key in unknown {
                    table.remove(&key);
                    self.changes.push(ConfigRepairChange::RemovedField {
                        field_path: self.render(&key),
                    });
                }
                let Some(properties) = properties else {
                    return;
                };
                for (key, item) in table.iter_mut() {
                    if let Some(property) = properties.get(key.get()) {
                        self.path.push(segment(key.get()));
                        self.walk_item(property, item);
                        self.path.pop();
                    }
                }
            }
            // A declared open map: every key is an authored identity.
            Some(entry @ Value::Object(_)) => {
                for (key, item) in table.iter_mut() {
                    self.path.push(segment(key.get()));
                    self.walk_item(entry, item);
                    self.path.pop();
                }
            }
            // An unconstrained object is opaque data, never repaired.
            _ => {}
        }
    }

    fn render(&self, key: &str) -> String {
        let mut path = String::new();
        for part in self
            .path
            .iter()
            .map(String::as_str)
            .chain([segment(key).as_str()])
        {
            if !path.is_empty() && !part.starts_with('[') {
                path.push('.');
            }
            path.push_str(part);
        }
        bounded(&path, MAX_PATH_CHARS)
    }
}

/// One key as TOML spells it (quoted and escaped when not bare), bounded.
fn segment(key: &str) -> String {
    bounded(&toml_edit::Key::new(key).display_repr(), MAX_SEGMENT_CHARS)
}

/// Bound `text` to `limit` characters, escaping control characters.
fn bounded(text: &str, limit: usize) -> String {
    let escaped: String = text
        .chars()
        .flat_map(|character| {
            let escaped: Vec<char> = if character.is_control() {
                character.escape_unicode().collect()
            } else {
                vec![character]
            };
            escaped
        })
        .collect();
    let mut result: String = escaped.chars().take(limit).collect();
    if escaped.chars().count() > limit {
        result.pop();
        result.push('…');
    }
    result
}

fn bounded_diagnostic(text: &str) -> String {
    let mut result: String = text.chars().take(MAX_DIAGNOSTIC_CHARS).collect();
    if text.chars().count() > MAX_DIAGNOSTIC_CHARS {
        result.pop();
        result.push('…');
    }
    result
}

/// Process-local identity of one source in the ledger.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum SourceKey {
    User,
    Workspace(PathBuf),
}

impl From<&SourceTarget> for SourceKey {
    fn from(target: &SourceTarget) -> Self {
        match target {
            SourceTarget::User => Self::User,
            SourceTarget::Workspace { directory } => Self::Workspace(directory.clone()),
        }
    }
}

#[derive(Debug, Clone)]
struct LedgerEntry {
    sequence: u64,
    receipt: Option<ConfigRepairReceipt>,
}

#[derive(Debug, Default)]
struct LedgerState {
    sequence: u64,
    entries: BTreeMap<SourceKey, LedgerEntry>,
}

/// Finite process-local repair observations: the latest receipt per source.
///
/// Not history, not an Event Journal, and never persisted. A source repaired
/// before any client connected is visible on that client's first source read.
#[derive(Clone)]
pub(crate) struct RepairLedger {
    state: Arc<Mutex<LedgerState>>,
    changed: tokio::sync::watch::Sender<u64>,
}

impl Default for RepairLedger {
    fn default() -> Self {
        Self {
            state: Arc::default(),
            changed: tokio::sync::watch::channel(0).0,
        }
    }
}

impl std::fmt::Debug for RepairLedger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RepairLedger")
    }
}

impl RepairLedger {
    fn lock(&self) -> std::sync::MutexGuard<'_, LedgerState> {
        self.state.lock().expect("configuration repair ledger")
    }

    /// Record `receipt` as its source's latest observation. Re-observing the
    /// same repair changes nothing and announces nothing.
    fn record(&self, receipt: ConfigRepairReceipt) {
        let key = SourceKey::from(&receipt.target);
        let mut state = self.lock();
        if state
            .entries
            .get(&key)
            .and_then(|entry| entry.receipt.as_ref())
            .is_some_and(|current| current.id == receipt.id)
        {
            return;
        }
        state.sequence += 1;
        let sequence = state.sequence;
        state.entries.insert(
            key,
            LedgerEntry {
                sequence,
                receipt: Some(receipt),
            },
        );
        // Finite: the least recently observed Workspace source is forgotten.
        while state
            .entries
            .keys()
            .filter(|key| matches!(key, SourceKey::Workspace(_)))
            .count()
            > MAX_LEDGER_WORKSPACES
        {
            let oldest = state
                .entries
                .iter()
                .filter(|(key, _)| matches!(key, SourceKey::Workspace(_)))
                .min_by_key(|(_, entry)| entry.sequence)
                .map(|(key, _)| key.clone())
                .expect("a Workspace entry exceeds the bound");
            state.entries.remove(&oldest);
        }
        self.changed.send_replace(sequence);
    }

    /// Replace an unresolved observation of the same source with `receipt`,
    /// itself unresolved. A source without one gains nothing.
    fn replace_unresolved(&self, receipt: ConfigRepairReceipt) {
        let unresolved = self
            .lock()
            .entries
            .get(&SourceKey::from(&receipt.target))
            .and_then(|entry| entry.receipt.as_ref())
            .is_some_and(|current| current.status == ConfigRepairStatus::Unresolved);
        if unresolved {
            self.record(receipt);
        }
    }

    /// A source found healthy no longer reports an unresolved repair.
    fn clear_unresolved(&self, key: &SourceKey) {
        let mut state = self.lock();
        let unresolved = state
            .entries
            .get(key)
            .and_then(|entry| entry.receipt.as_ref())
            .is_some_and(|receipt| receipt.status == ConfigRepairStatus::Unresolved);
        if unresolved {
            state.sequence += 1;
            let sequence = state.sequence;
            if let Some(entry) = state.entries.get_mut(key) {
                entry.sequence = sequence;
                entry.receipt = None;
            }
            self.changed.send_replace(sequence);
        }
    }

    /// The latest receipt of one source, if it has one.
    pub(crate) fn latest(&self, target: &SourceTarget) -> Option<ConfigRepairReceipt> {
        self.lock()
            .entries
            .get(&SourceKey::from(target))
            .and_then(|entry| entry.receipt.clone())
    }

    /// The source's observation if it changed after `cursor`, with its
    /// sequence. A cleared observation carries no receipt.
    pub(crate) fn changed_after(
        &self,
        key: &SourceKey,
        cursor: u64,
    ) -> Option<(u64, Option<ConfigRepairReceipt>)> {
        self.lock()
            .entries
            .get(key)
            .filter(|entry| entry.sequence > cursor)
            .map(|entry| (entry.sequence, entry.receipt.clone()))
    }

    pub(crate) fn subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.changed.subscribe()
    }
}

/// One finished observation before it becomes a receipt.
struct Observation {
    status: ConfigRepairStatus,
    after_revision: Option<String>,
    changes: Vec<ConfigRepairChange>,
    backup_available: bool,
    diagnostic: Option<String>,
}

fn receipt(target: &SourceTarget, before: &str, observation: Observation) -> ConfigRepairReceipt {
    let changed_count = u32::try_from(observation.changes.len()).unwrap_or(u32::MAX);
    let truncated = observation.changes.len() > MAX_RECEIPT_CHANGES;
    let mut changes = observation.changes;
    changes.truncate(MAX_RECEIPT_CHANGES);
    let mut receipt = ConfigRepairReceipt {
        id: String::new(),
        target: target.clone(),
        status: observation.status,
        before_revision: before.to_owned(),
        after_revision: observation.after_revision,
        changed_count,
        changes,
        truncated,
        backup_available: observation.backup_available,
        diagnostic: observation.diagnostic.as_deref().map(bounded_diagnostic),
    };
    let canonical = serde_json::to_vec(&receipt).expect("receipt serializes");
    let mut hash = Sha256::new();
    hash.update(b"rustx-config-repair-v1");
    hash.update(&canonical);
    receipt.id = format!("{:x}", hash.finalize());
    receipt
}

impl UserConfigManager {
    /// The process-local repair observations shared by every clone.
    pub(crate) fn repairs(&self) -> &RepairLedger {
        &self.repairs
    }

    /// Observe one bound source and repair it when, and only when, a fully
    /// validated deterministic candidate exists.
    ///
    /// Healthy sources are read once without any lock or write. A changed
    /// source is published through the native writer primitive under the
    /// source writer lock and exact revision fences, after an exact private
    /// backup of the original bytes. The caller serializes this with source
    /// writes and coordinator capture.
    ///
    /// `scope` is the admitted composition this observation serves, as in
    /// [`UserConfigManager::capture_source_settings`]: the User alone (process
    /// startup) or the User with one selected Workspace. A Workspace target
    /// must be that admitted Workspace.
    pub(crate) fn repair_source(
        &self,
        target: &SourceTarget,
        scope: &RepairScope,
    ) -> RepairOutcome {
        if target.validate().is_err() {
            return RepairOutcome::Unchanged;
        }
        let key = SourceKey::from(target);
        let path = self.config_path(target);
        let Some(name) = path.file_name() else {
            return RepairOutcome::Unchanged;
        };
        // The physical source this transaction observes, and may publish to:
        // the bound User directory, or the admitted Workspace itself.
        let opened;
        let source = match (target, &scope.workspace) {
            (SourceTarget::User, _) => match SourceDirectory::open(&path) {
                Ok(Some(source)) => {
                    opened = source;
                    &opened
                }
                Ok(None) => {
                    self.repairs.clear_unresolved(&key);
                    return RepairOutcome::Unchanged;
                }
                Err(_) => return RepairOutcome::Unchanged,
            },
            (SourceTarget::Workspace { .. }, Some(workspace)) if *target == scope.target => {
                workspace
            }
            (SourceTarget::Workspace { .. }, _) => return RepairOutcome::Unchanged,
        };
        let original = match source.read(name) {
            Ok(Some(original)) => original,
            // An absent document is healthy: there is nothing to repair.
            Ok(None) => {
                self.repairs.clear_unresolved(&key);
                return RepairOutcome::Unchanged;
            }
            // A symlinked or unreadable document is not this owner's to
            // rewrite, and says nothing about health; resolution reports it.
            Err(_) => return RepairOutcome::Unchanged,
        };
        let before = super::super::settings::revision(Some(&original));
        let unresolved = |changes, diagnostic: String| {
            self.repairs.record(receipt(
                target,
                &before,
                Observation {
                    status: ConfigRepairStatus::Unresolved,
                    after_revision: None,
                    changes,
                    backup_available: false,
                    diagnostic: Some(diagnostic),
                },
            ));
            RepairOutcome::Unchanged
        };
        let plan = match plan(&original) {
            Ok(plan) => plan,
            Err(uneditable) => return unresolved(Vec::new(), uneditable.diagnostic()),
        };
        if plan.changes.is_empty() {
            // Nothing is eligible, which proves nothing: the source is healthy
            // only under the source-intrinsic contract. Composition facts
            // another scope may supply are never this source's failure.
            let workspace = target.scope() == SourceScope::Workspace;
            return match intrinsic_health(&path, &original, workspace) {
                Ok(_) => {
                    self.repairs.clear_unresolved(&key);
                    RepairOutcome::Unchanged
                }
                Err(diagnostic) => unresolved(Vec::new(), diagnostic),
            };
        }
        if let Err(diagnostic) =
            self.validate_candidate(target, &scope.target, &path, &plan.candidate)
        {
            return unresolved(plan.changes, diagnostic);
        }
        let workspace = scope.workspace.as_ref();
        self.commit_repair(target, source, workspace, name, &before, &original, plan)
    }

    /// Admit the composition of one repair boundary before any of its source
    /// transactions (ISO-01..04).
    ///
    /// The User alone needs no admission. A selected Workspace must be a
    /// canonical directory lexically disjoint from the bound runtime root
    /// under the rule Session resolution applies — checked before anything is
    /// created — and then physically disjoint from it: both are observed as
    /// no-follow directory handles and neither may lie within the other.
    /// Admission creates and syncs nothing, so a healthy boundary costs no
    /// durability I/O; runtime storage that does not exist yet is checked
    /// against the Workspace handle by the backup that creates it. The
    /// returned scope carries the admitted Workspace handle to every
    /// transaction of the boundary. Any failure admits nothing: the boundary
    /// repairs neither source, and the fresh resolution that follows reports
    /// the scope.
    pub(crate) fn admit_repair_scope(&self, scope: &SourceTarget) -> Result<RepairScope, String> {
        fn refused<E>(_: E) -> String {
            String::from("the selected Workspace could not be admitted for repair")
        }
        let Some(directory) = scope.workspace() else {
            return Ok(RepairScope::user());
        };
        scope.validate().map_err(refused)?;
        super::storage_disjoint(self.runtime_root(), directory)?;
        let workspace = SourceDirectory::open(&self.config_path(scope))
            .map_err(refused)?
            .ok_or_else(|| refused(()))?;
        // Observed, never created: a healthy boundary costs no durability
        // I/O. Runtime storage that does not exist yet is checked against
        // this Workspace handle by the backup that creates it.
        let root = SourceDirectory::observe(self.runtime_root()).map_err(refused)?;
        if root.is_some_and(|root| !isolated(&root, &workspace)) {
            return Err("runtime_root must be disjoint from the workspace".into());
        }
        #[cfg(test)]
        self.test_hooks.reach("repair_scope_admitted");
        Ok(RepairScope {
            target: scope.clone(),
            workspace: Some(workspace),
        })
    }

    /// After a committed source write, observe the source again under the
    /// same source-intrinsic contract without repairing it. An unresolved
    /// receipt is withdrawn only when the source is now intrinsically
    /// healthy, and replaced by the current unresolved observation while it
    /// is not. A source without an unresolved receipt gains none here, and a
    /// source with eligible repairs is left to the next repair observation.
    pub(crate) fn refresh_repair_observation(&self, target: &SourceTarget) {
        let path = self.config_path(target);
        let workspace = target.scope() == SourceScope::Workspace;
        let Ok(bytes) = super::super::settings::read_document(&path) else {
            return;
        };
        let Some(bytes) = bytes else {
            self.repairs.clear_unresolved(&SourceKey::from(target));
            return;
        };
        let diagnostic = match plan(&bytes) {
            Err(uneditable) => uneditable.diagnostic(),
            Ok(plan) if !plan.changes.is_empty() => return,
            Ok(_) => match intrinsic_health(&path, &bytes, workspace) {
                Ok(_) => {
                    self.repairs.clear_unresolved(&SourceKey::from(target));
                    return;
                }
                Err(diagnostic) => diagnostic,
            },
        };
        self.repairs.replace_unresolved(receipt(
            target,
            &super::super::settings::revision(Some(&bytes)),
            Observation {
                status: ConfigRepairStatus::Unresolved,
                after_revision: None,
                changes: Vec::new(),
                backup_available: false,
                diagnostic: Some(diagnostic),
            },
        ));
    }

    /// Validate a complete candidate under its applicable contract.
    ///
    /// The serialized candidate must fit the authored file bound its reader
    /// enforces, checked here — before any backup — although publication
    /// fences it again. The production strict parser, the source restrictions
    /// and every source-intrinsic native constraint always apply.
    /// Composition-dependent constraints apply to the candidate composed with
    /// the other source of `scope`, exactly as Session resolution composes
    /// them, once that source is known: never at startup for the User, which
    /// may leave its default model or Model definitions to a Workspace not
    /// yet selected.
    fn validate_candidate(
        &self,
        target: &SourceTarget,
        scope: &SourceTarget,
        path: &Path,
        candidate: &[u8],
    ) -> Result<(), String> {
        let workspace = target.scope() == SourceScope::Workspace;
        if !crate::bounded_file::within_bound(candidate) {
            return Err(
                "the repaired document would exceed the authored file size limit; the source is unchanged"
                    .into(),
            );
        }
        if plan(candidate).map_or(true, |again| !again.changes.is_empty()) {
            return Err("automatic repair did not converge; the source is unchanged".into());
        }
        let layer = intrinsic_health(path, candidate, workspace)?;
        match self.composition(target, scope, layer) {
            Some(merged) => super::composition_semantics(&merged)
                .map(drop)
                .map_err(|failure| semantic_diagnostic(&failure)),
            None => Ok(()),
        }
    }

    /// The candidate layer composed with the other source of `scope` (User,
    /// then Workspace). `None` when that source is not known — the User alone
    /// at startup — or is unavailable or invalid on its own terms: those are
    /// missing external facts, never failures of this candidate. An absent
    /// document is known: it contributes nothing.
    fn composition(
        &self,
        target: &SourceTarget,
        scope: &SourceTarget,
        candidate: RuntimeLayer,
    ) -> Option<RuntimeLayer> {
        let other = match target {
            SourceTarget::User => scope.workspace().map(|directory| SourceTarget::Workspace {
                directory: directory.to_path_buf(),
            })?,
            SourceTarget::Workspace { .. } => SourceTarget::User,
        };
        let path = self.config_path(&other);
        let other_layer = match super::super::settings::read_document(&path).ok()? {
            None => RuntimeLayer::default(),
            Some(bytes) => {
                intrinsic_health(&path, &bytes, other.scope() == SourceScope::Workspace).ok()?
            }
        };
        let (user, workspace) = match target {
            SourceTarget::User => (candidate, other_layer),
            SourceTarget::Workspace { .. } => (other_layer, candidate),
        };
        let mut merged = RuntimeLayer {
            app_server: user.app_server.clone(),
            ..RuntimeLayer::default()
        };
        let mut origins = BTreeMap::new();
        merged.overlay(user, &super::Origin::Builtin, &mut origins);
        merged.overlay(workspace, &super::Origin::Builtin, &mut origins);
        Some(merged)
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "one transaction's admitted handles and its planned revision"
    )]
    fn commit_repair(
        &self,
        target: &SourceTarget,
        source: &SourceDirectory,
        workspace: Option<&SourceDirectory>,
        name: &std::ffi::OsStr,
        before: &str,
        original: &[u8],
        plan: RepairPlan,
    ) -> RepairOutcome {
        let scope = target.scope();
        let RepairPlan { changes, candidate } = plan;
        let committed = super::super::settings::revision(Some(&candidate));
        let record = |observation| {
            self.repairs.record(receipt(target, before, observation));
        };
        #[cfg(test)]
        self.test_hooks.reach("repair_before_lock");
        let Ok(_lock) = source.lock(name) else {
            record(Observation {
                status: ConfigRepairStatus::Unresolved,
                after_revision: None,
                changes,
                backup_available: false,
                diagnostic: Some(
                    "the source writer lock is unavailable; the source is unchanged".into(),
                ),
            });
            return RepairOutcome::Unchanged;
        };
        // The candidate was planned from `before` in this physical source; it
        // is publishable only while the target still names that source and
        // the locked document still has exactly those bytes.
        if !source.holds() {
            return RepairOutcome::Superseded;
        }
        match source.read(name) {
            Ok(bytes) if super::super::settings::revision(bytes.as_deref()) == before => {}
            _ => return RepairOutcome::Superseded,
        }
        if let Err(diagnostic) = self.backup_original(target, workspace, before, original) {
            record(Observation {
                status: ConfigRepairStatus::Unresolved,
                after_revision: None,
                changes,
                backup_available: false,
                diagnostic: Some(diagnostic),
            });
            return RepairOutcome::Unchanged;
        }
        let outcome = self.publish_locked(source, name, scope, before, Some(&candidate), true);
        let (status, after_revision, diagnostic, result) = match outcome {
            Ok(()) => (
                ConfigRepairStatus::Repaired,
                Some(committed),
                None,
                RepairOutcome::Committed,
            ),
            // Changed bytes, or a target that no longer names this physical
            // source: nothing was published and nothing is reported. (An
            // oversized candidate never gets here: validation refused it.)
            Err(SettingsError::Conflict { .. } | SettingsError::Invalid) => {
                return RepairOutcome::Superseded;
            }
            Err(SettingsError::Committed) => {
                // Possibly published: report what the target's source holds
                // now, unknown once the target names another directory; never
                // replay.
                let observed = source
                    .holds()
                    .then(|| source.read(name).ok())
                    .flatten()
                    .map(|bytes| super::super::settings::revision(bytes.as_deref()));
                (
                    ConfigRepairStatus::CommitUncertain,
                    observed,
                    Some(
                        "publication was not durably confirmed; reread the source before editing"
                            .into(),
                    ),
                    RepairOutcome::Committed,
                )
            }
            Err(SettingsError::Io) => (
                ConfigRepairStatus::Unresolved,
                None,
                Some("the staged candidate could not be written; the source is unchanged".into()),
                RepairOutcome::Unchanged,
            ),
        };
        record(Observation {
            status,
            after_revision,
            changes,
            backup_available: true,
            diagnostic,
        });
        result
    }

    /// The backup of one source revision: its scope, a digest of the source
    /// path (never the path itself) and the original revision.
    fn backup_name(&self, target: &SourceTarget, before: &str) -> String {
        let source = format!(
            "{:x}",
            Sha256::digest(self.config_path(target).as_os_str().as_bytes())
        );
        format!(
            "{}-{}-{}.toml",
            match target.scope() {
                SourceScope::User => "user",
                SourceScope::Workspace => "workspace",
            },
            &source[..16],
            before.trim_start_matches("sha256:")
        )
    }

    /// Keep one exact private copy of the original bytes outside the selected
    /// Workspace, named by source identity and original revision so repeated
    /// repairs of the same bytes keep exactly one copy.
    ///
    /// The bound runtime root is admitted for durable writes: walked from `/`
    /// one component at a time without following any symlink, missing
    /// components created relative to their verified parent. With a selected
    /// Workspace — `workspace`, the handle its scope admitted, whichever
    /// source is being repaired — the runtime root must be physically
    /// disjoint from it. The backup directory is then
    /// created or opened relative to that handle and verified as this user's
    /// `0700` directory.
    /// The staged copy, the no-clobber link that publishes it, the comparison
    /// with an existing copy and the durability sync all go through that one
    /// handle, so a symlink in any ancestor, or any replacement after
    /// admission, can never redirect the bytes.
    ///
    /// The backup is durable only once every name leading to it is, as this
    /// transaction itself confirmed them: every directory on the way has its
    /// entry confirmed by syncing its parent before use, whether this
    /// transaction created it, another writer just did, or it was merely
    /// visible (see [`SourceDirectory`]); the backup directory is synced after
    /// the link. Any failed sync is a backup failure.
    ///
    /// Physical safety does not depend on the pathname afterwards; whether
    /// the backup is confirmed does. The copy counts only if, once durable,
    /// the bound pathname still names the directory holding it. Otherwise the
    /// copy stays in the admitted (moved) directory and the source is left
    /// unchanged.
    fn backup_original(
        &self,
        target: &SourceTarget,
        workspace: Option<&SourceDirectory>,
        before: &str,
        original: &[u8],
    ) -> Result<(), String> {
        fn failed<E>(_: E) -> String {
            String::from("the original could not be backed up; the source is unchanged")
        }
        let outside = || {
            String::from(
                "backup storage would not be private rustX storage outside the Workspace; the source is unchanged",
            )
        };
        #[cfg(test)]
        if self.test_hooks.fault("backup") {
            return Err(failed(()));
        }
        #[cfg(test)]
        self.test_hooks.reach("before_backup");
        let root = SourceDirectory::admit_directory(self.runtime_root()).map_err(failed)?;
        if workspace.is_some_and(|workspace| !isolated(&root, workspace)) {
            return Err(outside());
        }
        let directory = root.private_child(BACKUP_DIRECTORY).map_err(failed)?;
        drop(root);
        #[cfg(test)]
        self.test_hooks.reach("backup_directory_opened");
        let name = self.backup_name(target, before);
        let staged = directory.stage(original).map_err(failed)?;
        let name = std::ffi::OsStr::new(&name);
        match directory.link(&staged, name) {
            Ok(()) => {}
            // The same original bytes were backed up before: keep that copy,
            // but only if it really is those bytes.
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if directory.read(name).map_err(failed)?.as_deref() != Some(original) {
                    return Err(failed(()));
                }
            }
            Err(error) => return Err(failed(error)),
        }
        drop(staged);
        directory.sync().map_err(failed)?;
        if !directory.holds() {
            return Err(
                "backup storage moved while the original was backed up; the source is unchanged"
                    .into(),
            );
        }
        Ok(())
    }
}

/// The source-intrinsic health contract, the one predicate for "healthy":
/// the strict parser, the source restrictions and every native constraint a
/// source decides on its own ([`super::source_semantics`]). Facts another
/// scope may supply — Model definitions, the default selection — are not
/// part of it. Failures are safe diagnostics.
fn intrinsic_health(path: &Path, bytes: &[u8], workspace: bool) -> Result<RuntimeLayer, String> {
    let layer = super::parse_layer(path, bytes, workspace)
        .map_err(|failure| source_contract_diagnostic(&failure))?;
    super::source_semantics(&layer).map_err(|failure| semantic_diagnostic(&failure))?;
    Ok(layer)
}

/// A source-contract failure, safe to show: the parser's structured location
/// and classification, never its message (which may quote authored values).
fn source_contract_diagnostic(failure: &super::super::diagnostics::LaunchFailure) -> String {
    use std::fmt::Write;
    let diagnostic = &failure.diagnostic;
    let mut text = diagnostic.reason.clone();
    if let Some(path) = schema_path(&diagnostic.path) {
        let _ = write!(text, " at {}", bounded(&path, MAX_PATH_CHARS));
    }
    if let (Some(line), Some(column)) = (diagnostic.line, diagnostic.column) {
        let _ = write!(text, " (line {line}, column {column})");
    }
    text.push_str("; it is not an automatically repairable construct");
    text
}

/// A candidate that parses but fails a native semantic constraint: the
/// native category and schema location only, never the domain message.
fn semantic_diagnostic(failure: &super::SemanticFailure) -> String {
    format!(
        "{}; automatic repair never corrects an invalid value, so the source is unchanged",
        failure.summary()
    )
}

/// A parser path projected onto the source schema: each segment is either a
/// schema property name or `*` for an open-map identity. A path the schema
/// does not explain segment by segment is dropped, never echoed, so the result
/// holds native names only.
fn schema_path(path: &str) -> Option<String> {
    if path == "$" || path.is_empty() {
        return None;
    }
    let walker = Walker {
        root: source_schema(),
        path: Vec::new(),
        changes: Vec::new(),
    };
    let mut node = walker.root;
    let mut projected = Vec::new();
    for segment in path.split('.') {
        let resolved = walker.resolve(node);
        let candidates: Vec<&Value> = match resolved
            .get("anyOf")
            .or_else(|| resolved.get("oneOf"))
            .and_then(Value::as_array)
        {
            Some(branches) => branches
                .iter()
                .map(|branch| walker.resolve(branch))
                .collect(),
            None => vec![resolved],
        };
        let property = candidates.iter().find_map(|candidate| {
            candidate
                .get("properties")
                .and_then(Value::as_object)
                .and_then(|properties| properties.get_key_value(segment))
        });
        if let Some((name, next)) = property {
            projected.push(name.as_str());
            node = next;
        } else {
            node = candidates.iter().find_map(|candidate| {
                candidate
                    .get("additionalProperties")
                    .filter(|entry| entry.is_object())
            })?;
            projected.push("*");
        }
    }
    Some(projected.join("."))
}

#[cfg(test)]
#[path = "repair_tests.rs"]
mod tests;
