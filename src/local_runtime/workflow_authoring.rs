//! Native file-backed Workflow authoring (Issue #491).
//!
//! One Workflow is one canonical YAML file physically owned by exactly one
//! source scope: `~/rustx/.agents/workflows/<id>.yaml` for the User scope
//! (presented as Global) and `<workspace>/.agents/workflows/<id>.yaml` for a
//! Workspace. This owner lists and reads exactly those files — never the
//! merged effective catalog — checks candidate documents offline, and builds
//! the canonical bytes of each Workflow source mutation. Every commit goes
//! through the one native source publication primitive
//! (`UserConfigManager::write_source_settings`), so a Workflow write has the
//! same revision fence, writer lock, staging, atomic replacement, durability
//! and uncertainty rules as every other authored source.
//!
//! Nothing here creates a Session, prepares a source, connects, contacts a
//! provider, reads a credential or runs a node.
use super::configuration::UserConfigManager;
use super::configuration::settings::{SettingsError, SourceTarget};
use super::settings::SourceDirectory;
use crate::runtime::resources::RuntimeResourceLoadError;
use crate::runtime::workflow::{MAX_WORKFLOW_DIAGNOSTIC_BYTES, WorkflowId};
use crate::workflow_file::WorkflowFileDocument;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

mod analysis;
mod candidate;
pub use analysis::WorkflowAnalysisReport;
mod capacity;
mod check;
mod model_choices;
#[cfg(test)]
mod tests;

pub use candidate::WorkflowEdit;
pub(super) use candidate::candidate;
pub(super) use capacity::Capacity;
pub use check::{WorkflowCheckReport, WorkflowDependencies, WorkflowStructure};
pub use model_choices::{WorkflowModelChoices, WorkflowModelChoicesQuery};

/// The byte bound of a description in the bounded library listing.
pub const MAX_LISTED_DESCRIPTION_BYTES: usize = 512;
/// The UTF-8 byte bound of a diagnostic's authored path.
pub const MAX_DIAGNOSTIC_PATH_BYTES: usize = 512;
/// The serialized byte share of one library entry. A scope holds at most
/// 128 Workflow files, so a whole listing stays within 896 KiB of the 1 MiB
/// App Server message bound however its entries are authored.
const MAX_LISTED_ENTRY_BYTES: usize = 7 * 1024;
/// The largest integer magnitude a browser JSON number preserves exactly.
const MAX_EXACT_BROWSER_INTEGER: u64 = (1 << 53) - 1;
/// The magnitude from which a browser prints an integral binary64 in
/// exponent form, which still reads back as a native float.
const MIN_EXPONENT_FORM: f64 = 1e21;

/// One bounded native diagnostic of a Workflow file or candidate.
///
/// `path` is the native authored field path (`block.nodes.<id>.…`, `$` for the
/// document) so a client can select the owning block, node and field. Reasons
/// and corrections are static native text; `detail` is the bounded compiler
/// rejection. Parser messages, unknown field names and authored values are
/// never copied here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowDiagnostic {
    pub path: String,
    pub category: String,
    pub reason: String,
    pub correction: Option<String>,
    pub detail: Option<String>,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

impl WorkflowDiagnostic {
    fn new(path: impl Into<String>, category: &str, reason: &str) -> Self {
        Self {
            path: path.into(),
            category: category.into(),
            reason: reason.into(),
            correction: None,
            detail: None,
            line: None,
            column: None,
        }
    }

    pub(super) fn from_resource(error: &RuntimeResourceLoadError) -> Self {
        let path = error
            .field_path
            .as_deref()
            .filter(|path| !path.starts_with("workflows") && !path.starts_with("resources"))
            .unwrap_or("$");
        Self {
            path: bounded_path(path),
            category: error.inspection.category.unwrap_or("resource").into(),
            reason: error
                .diagnostic_reason
                .unwrap_or("Workflow file cannot be read as one bounded regular YAML file")
                .into(),
            correction: error.inspection.correction.map(Into::into),
            detail: error
                .inspection
                .detail
                .as_deref()
                .map(|detail| bounded(detail, MAX_WORKFLOW_DIAGNOSTIC_BYTES)),
            line: error.inspection.line,
            column: error.inspection.column,
        }
    }
}

/// An authored path within `MAX_DIAGNOSTIC_PATH_BYTES`. A longer one (an
/// oversized invalid identity, say) keeps its longest whole-component
/// prefix, so it still locates the owning block; `$` when none fits.
fn bounded_path(path: &str) -> String {
    if path.len() <= MAX_DIAGNOSTIC_PATH_BYTES {
        return path.into();
    }
    let mut end = 0;
    for (index, _) in path.match_indices('.') {
        if index > MAX_DIAGNOSTIC_PATH_BYTES {
            break;
        }
        end = index;
    }
    if end == 0 {
        "$".into()
    } else {
        path[..end].into()
    }
}

/// One file physically owned by the listed scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowFileSummary {
    /// The file's basename, lossily decoded for presentation.
    pub file_name: String,
    /// The native identity; absent when the filename is not a Workflow id.
    pub id: Option<WorkflowId>,
    /// The location relative to the scope's owning directory, e.g.
    /// `.agents/workflows/<id>.yaml`.
    pub relative_path: String,
    /// Exact byte revision, the CAS token of edits, copies and deletion.
    pub revision: String,
    pub byte_length: u64,
    /// Last modification in milliseconds since the Unix epoch, when known.
    pub modified_ms: Option<i64>,
    /// The bounded authored description of a parseable document.
    pub description: Option<String>,
    /// How people see a parseable document, its description bounded.
    pub display: Option<crate::workflow_file::WorkflowDisplay>,
    /// Root-block node count of a parseable document.
    pub nodes: Option<usize>,
    /// Why this file is not a valid Workflow program.
    pub diagnostic: Option<WorkflowDiagnostic>,
    /// Informational: a Workspace file whose id also names a User (Global)
    /// file, which it shadows in this Workspace. Edits never redirect.
    pub shadows_global: bool,
}

/// The Workflow files physically owned by one source scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowLibrary {
    pub target: SourceTarget,
    /// The canonical directory owning this scope's Workflow files.
    pub directory: PathBuf,
    /// The CAS token of an absent file: the expected revision of a create or
    /// copy destination.
    pub absent_revision: String,
    /// Files in deterministic byte order of their basenames.
    pub files: Vec<WorkflowFileSummary>,
    /// A directory-level failure; `files` is then empty, never partial.
    pub diagnostic: Option<WorkflowDiagnostic>,
}

/// The native names an author can choose from in this scope: discovered
/// named Agent profiles and the builtin Tools a Tool node can select.
/// Choices, never admission: the native check decides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowVocabulary {
    pub profiles: Vec<crate::runtime::subagent::SubagentName>,
    pub builtin_tools: Vec<String>,
}

/// The complete authorized source of one exact Workflow file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowSource {
    pub target: SourceTarget,
    pub id: WorkflowId,
    pub path: PathBuf,
    pub relative_path: String,
    /// The exact byte revision; the absent revision when the file is missing.
    pub revision: String,
    pub present: bool,
    pub modified_ms: Option<i64>,
    /// The parsed native definition. Absent for a missing, unparseable or
    /// browser-unrepresentable file, whose bytes stay untouched on disk.
    pub document: Option<Box<WorkflowFileDocument>>,
    /// The first structural (parse or compile) or representability failure.
    pub diagnostic: Option<WorkflowDiagnostic>,
    pub vocabulary: WorkflowVocabulary,
}

/// The fixed native locations of one source scope.
struct Scope {
    /// The authorization boundary of resource reads.
    boundary: PathBuf,
    /// The directory owning the scope's Workflow files.
    directory: PathBuf,
    /// The base of scope-relative presentation paths.
    base: PathBuf,
}

impl Scope {
    fn of(manager: &UserConfigManager, target: &SourceTarget) -> Self {
        let resources = manager.resource_root(target);
        let base = match target {
            SourceTarget::User => resources
                .parent()
                .map_or_else(|| resources.clone(), Path::to_path_buf),
            SourceTarget::Workspace { directory } => directory.clone(),
        };
        let boundary = match target {
            SourceTarget::User => resources.clone(),
            SourceTarget::Workspace { directory } => directory.clone(),
        };
        Self {
            boundary,
            directory: resources.join("workflows"),
            base,
        }
    }

    fn relative(&self, path: &Path) -> String {
        path.strip_prefix(&self.base)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned()
    }
}

/// The canonical file of one Workflow identity in one source scope.
pub(super) fn file_path(
    manager: &UserConfigManager,
    target: &SourceTarget,
    id: &WorkflowId,
) -> PathBuf {
    Scope::of(manager, target).directory.join(file_name(id))
}

fn file_name(id: &WorkflowId) -> String {
    format!("{id}.yaml")
}

fn modified_ms(path: &Path) -> Option<i64> {
    let modified = std::fs::symlink_metadata(path).ok()?.modified().ok()?;
    let duration = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    i64::try_from(duration.as_millis()).ok()
}

fn bounded(text: &str, limit: usize) -> String {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

/// The number contract of Workflow authoring across the browser boundary.
///
/// Workflow documents cross the App Server as JSON, and the browser reads
/// every JSON number as one binary64 and writes it back as the shortest
/// decimal of that binary64. A native number is editable exactly when that
/// round trip returns the same native number:
///
/// - an integer survives within ±(2^53 − 1);
/// - a float survives unless it is integral below 1e21 (or −0.0): the
///   browser would write `1.0` back as the integer `1`, which native types
///   differently.
///
/// The first other number makes a file read-only in the editor, and a
/// candidate carrying one is rejected, so a browser can neither receive a
/// number it would change nor commit one it already changed.
pub(super) fn unrepresentable_number(
    definition: &WorkflowFileDocument,
) -> Option<WorkflowDiagnostic> {
    fn exact(number: &serde_json::Number) -> bool {
        if let Some(n) = number.as_u64() {
            return n <= MAX_EXACT_BROWSER_INTEGER;
        }
        if let Some(n) = number.as_i64() {
            return n.unsigned_abs() <= MAX_EXACT_BROWSER_INTEGER;
        }
        number.as_f64().is_some_and(|n| {
            n.is_finite()
                && (n.fract() != 0.0 || n.abs() >= MIN_EXPONENT_FORM)
                && !(n == 0.0 && n.is_sign_negative())
        })
    }
    fn find(value: &Value, path: &mut Vec<String>) -> bool {
        match value {
            Value::Number(number) => !exact(number),
            Value::Array(values) => values.iter().enumerate().any(|(index, value)| {
                path.push(index.to_string());
                let found = find(value, path);
                if !found {
                    path.pop();
                }
                found
            }),
            Value::Object(fields) => fields.iter().any(|(key, value)| {
                path.push(key.clone());
                let found = find(value, path);
                if !found {
                    path.pop();
                }
                found
            }),
            _ => false,
        }
    }
    let value = serde_json::to_value(&definition.definition).ok()?;
    let mut path = Vec::new();
    find(&value, &mut path).then(|| WorkflowDiagnostic {
        correction: Some(
            "use an integer within ±9007199254740991 or a non-integral number, or edit this file outside the browser editor"
                .into(),
        ),
        ..WorkflowDiagnostic::new(
            bounded_path(&path.join(".")),
            "unsupported_number",
            "the browser editor cannot preserve this number exactly",
        )
    })
}

/// Parse a present file into an editable document, or explain why not.
fn parse(
    id: &WorkflowId,
    path: &Path,
    bytes: &[u8],
) -> (Option<WorkflowFileDocument>, Option<WorkflowDiagnostic>) {
    let definition = match super::workflow_resources::parse_source(id, path, bytes) {
        Ok(definition) => definition,
        Err(error) => return (None, Some(WorkflowDiagnostic::from_resource(&error))),
    };
    if serde_json::to_vec(&definition).map_or(true, |json| {
        json.len() > crate::workflow_file::MAX_WORKFLOW_DOCUMENT_BYTES
    }) {
        return (
            None,
            Some(WorkflowDiagnostic::new(
                "$",
                "size",
                "the expanded Workflow document exceeds 768 KiB",
            )),
        );
    }
    if let Some(diagnostic) = unrepresentable_number(&definition) {
        return (None, Some(diagnostic));
    }
    let diagnostic =
        super::workflow_resources::compile_definition(id, path, definition.definition.clone())
            .err()
            .map(|error| WorkflowDiagnostic::from_resource(&error));
    let diagnostic = diagnostic.or_else(|| {
        definition
            .presentation_diagnostics()
            .first()
            .map(presentation_diagnostic)
    });
    (Some(definition), diagnostic)
}

/// The bounded facts of one listed file, derived from exactly the bytes its
/// revision names; `None` when it was removed after enumeration.
fn summarize(
    scope: &Scope,
    directory: &SourceDirectory,
    path: &Path,
    shadows_global: bool,
) -> Option<WorkflowFileSummary> {
    let name = path.file_name()?;
    let file_name = name.to_string_lossy().into_owned();
    let id = path
        .file_stem()
        .and_then(OsStr::to_str)
        .and_then(|stem| WorkflowId::parse(stem).ok());
    let mut summary = WorkflowFileSummary {
        relative_path: scope.relative(path),
        file_name,
        id,
        revision: String::new(),
        byte_length: 0,
        modified_ms: modified_ms(path),
        description: None,
        display: None,
        nodes: None,
        diagnostic: None,
        shadows_global,
    };
    let bytes = match directory.read(name) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return None,
        Err(_) => {
            summary.diagnostic = Some(WorkflowDiagnostic::new(
                "$",
                "resource",
                "Workflow file cannot be read as one bounded regular file",
            ));
            return Some(summary);
        }
    };
    let (definition, diagnostic) = match &summary.id {
        Some(id) => parse(id, path, &bytes),
        None => (
            None,
            Some(WorkflowDiagnostic {
                correction: Some(
                    "rename the file to <id>.yaml with a lowercase [a-z0-9_-] id".into(),
                ),
                ..WorkflowDiagnostic::new(
                    "$",
                    "identity",
                    "the filename is not a valid Workflow identity",
                )
            }),
        ),
    };
    summary.revision = super::settings::revision(Some(&bytes));
    summary.byte_length = bytes.len() as u64;
    summary.description = definition.as_ref().map(|definition| {
        bounded(
            &definition.definition.description,
            MAX_LISTED_DESCRIPTION_BYTES,
        )
    });
    summary.display = definition
        .as_ref()
        .and_then(|definition| definition.editor.as_ref()?.display.clone())
        .map(|mut display| {
            display.description = display
                .description
                .map(|text| bounded(&text, MAX_LISTED_DESCRIPTION_BYTES));
            display
        });
    summary.nodes = definition
        .as_ref()
        .map(|definition| definition.definition.block.nodes.len());
    summary.diagnostic = diagnostic;
    Some(fit_listing_share(summary))
}

/// Keep one entry within its fixed listing share. Only an entry authored
/// to the extreme is reduced, from the least to the most locating fact: the
/// compiler detail, then the diagnostic path, then the descriptions. Its
/// identity, revision and diagnostic category always remain, so an invalid
/// file stays discoverable and never displaces another entry.
fn fit_listing_share(mut summary: WorkflowFileSummary) -> WorkflowFileSummary {
    let fits = |summary: &WorkflowFileSummary| {
        serde_json::to_vec(summary).is_ok_and(|json| json.len() <= MAX_LISTED_ENTRY_BYTES)
    };
    if fits(&summary) {
        return summary;
    }
    if let Some(diagnostic) = &mut summary.diagnostic {
        diagnostic.detail = None;
    }
    if fits(&summary) {
        return summary;
    }
    if let Some(diagnostic) = &mut summary.diagnostic {
        diagnostic.path = "$".into();
    }
    if fits(&summary) {
        return summary;
    }
    summary.description = None;
    if fits(&summary) {
        return summary;
    }
    if let Some(display) = &mut summary.display {
        display.description = None;
    }
    summary
}

impl UserConfigManager {
    /// List exactly the Workflow files physically owned by `target`.
    ///
    /// A Workspace listing never includes User files; it only marks Workspace
    /// files whose identity also names a User file. Each entry's revision is
    /// the revision of exactly the bytes its facts were derived from.
    /// # Errors
    /// Rejects a relative, removed or redirected Workspace target.
    pub fn workflow_library(
        &self,
        target: &SourceTarget,
    ) -> Result<WorkflowLibrary, SettingsError> {
        target.validate()?;
        let scope = Scope::of(self, target);
        let mut library = WorkflowLibrary {
            target: target.clone(),
            directory: scope.directory.clone(),
            absent_revision: super::settings::revision(None),
            files: Vec::new(),
            diagnostic: None,
        };
        let paths =
            match super::resource_directory::files(&scope.boundary, &scope.directory, "yaml") {
                Ok(paths) => paths,
                Err(error) => {
                    library.diagnostic = Some(WorkflowDiagnostic::from_resource(&error));
                    return Ok(library);
                }
            };
        if paths.is_empty() {
            return Ok(library);
        }
        // Reads are relative to the directory handle the listing admitted;
        // a redirected directory or symlinked file is never followed.
        let directory = match SourceDirectory::observe(&scope.directory) {
            Ok(Some(directory)) => directory,
            Ok(None) => return Ok(library),
            Err(_) => {
                target.validate()?;
                library.diagnostic = Some(WorkflowDiagnostic::new(
                    "$",
                    "resource",
                    "the Workflow directory is not a canonical directory of this scope",
                ));
                return Ok(library);
            }
        };
        let global = match target {
            SourceTarget::User => BTreeSet::new(),
            SourceTarget::Workspace { .. } => {
                let user = Scope::of(self, &SourceTarget::User);
                super::resource_directory::files(&user.boundary, &user.directory, "yaml")
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|path| path.file_name().map(OsStr::to_os_string))
                    .collect()
            }
        };
        library.files.extend(paths.into_iter().filter_map(|path| {
            let shadows_global = path.file_name().is_some_and(|name| global.contains(name));
            summarize(&scope, &directory, &path, shadows_global)
        }));
        library
            .files
            .sort_by(|left, right| left.file_name.as_bytes().cmp(right.file_name.as_bytes()));
        Ok(library)
    }

    /// Read the complete authored source of one exact Workflow file.
    ///
    /// An unparseable file is reported with its diagnostic and never
    /// replaced by an empty program: its document is absent and its bytes
    /// keep their revision.
    /// # Errors
    /// Rejects a redirected target or directory, and an unreadable file.
    pub fn workflow_source(
        &self,
        target: &SourceTarget,
        id: &WorkflowId,
    ) -> Result<WorkflowSource, SettingsError> {
        target.validate()?;
        let scope = Scope::of(self, target);
        let path = scope.directory.join(file_name(id));
        super::resource_directory::entries(&scope.boundary, &scope.directory)
            .map_err(|_| target.validate().err().unwrap_or(SettingsError::Invalid))?;
        let bytes = match SourceDirectory::observe(&scope.directory) {
            Ok(Some(directory)) => directory
                .read(OsStr::new(&file_name(id)))
                .map_err(|_| SettingsError::Io)?,
            Ok(None) => None,
            Err(_) => return Err(target.validate().err().unwrap_or(SettingsError::Invalid)),
        };
        let (document, diagnostic) = bytes
            .as_deref()
            .map_or((None, None), |bytes| parse(id, &path, bytes));
        Ok(WorkflowSource {
            target: target.clone(),
            id: id.clone(),
            relative_path: scope.relative(&path),
            revision: super::settings::revision(bytes.as_deref()),
            present: bytes.is_some(),
            modified_ms: bytes.as_ref().and_then(|_| modified_ms(&path)),
            document: document.map(Box::new),
            diagnostic,
            vocabulary: self.workflow_vocabulary(target),
            path,
        })
    }

    /// Valid named Agent profiles visible to a file of `target` (User files:
    /// User profiles; Workspace files: User and Workspace profiles).
    fn workflow_vocabulary(&self, target: &SourceTarget) -> WorkflowVocabulary {
        let user = self.resource_root(&SourceTarget::User).join("agents");
        let profiles = super::agent_resources::load_authorized(target.workspace(), &user)
            .map(|(catalog, _)| catalog.names().into_iter().cloned().collect())
            .unwrap_or_default();
        WorkflowVocabulary {
            profiles,
            builtin_tools: crate::runtime::workflow::selectable_builtin_tools(),
        }
    }
}

/// Presentation errors have their own diagnostic category, never executable types.
fn presentation_diagnostic(
    error: &crate::workflow_file::WorkflowPresentationDiagnostic,
) -> WorkflowDiagnostic {
    WorkflowDiagnostic::new(&error.path, &error.category, &error.reason)
}
