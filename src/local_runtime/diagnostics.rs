//! Bounded prospective diagnostics. This module owns no runtime or executor.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Serialize;
use serde_json::{Value, json};

use super::launch::{HostEnvironment, LaunchRequest, Origin, ProspectiveLaunch, PythonLocalStatus};
use crate::capabilities::activation::SourceActivation;

pub(super) const OUTPUT_LIMIT: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Validity {
    Valid,
    Invalid,
    Incomplete,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Readiness {
    // CFG-04 has no provider verification operation and cannot establish ready.
    Unresolved,
}

#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    pub classification: &'static str,
    pub category: &'static str,
    pub file: Option<PathBuf>,
    pub path: String,
    pub reason: String,
    pub correction: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

/// Structured context retained at the launch owner; raw domain error text is
/// private and is never used in static projections or Debug output.
pub struct LaunchFailure {
    pub diagnostic: Box<Diagnostic>,
    pub incomplete: bool,
    pub partial: Option<Box<PartialProjection>>,
    detail: String,
}

impl LaunchFailure {
    pub(super) fn resource(error: crate::runtime::resources::RuntimeResourceLoadError) -> Self {
        Self::at(
            error.source_file,
            error.field_path.as_deref().unwrap_or("resources"),
            error
                .diagnostic_reason
                .unwrap_or("local resource loading or static compilation failed"),
            "correct the referenced file and its native resource contract",
            error.message,
        )
    }
    pub(super) fn at(
        file: Option<PathBuf>,
        path: &str,
        reason: &str,
        correction: &str,
        detail: String,
    ) -> Self {
        Self {
            diagnostic: Box::new(Diagnostic {
                classification: "error",
                category: "invalid",
                file,
                path: path.chars().take(256).collect(),
                reason: reason.into(),
                correction: correction.into(),
                line: None,
                column: None,
            }),
            incomplete: false,
            partial: None,
            detail,
        }
    }
    pub(super) fn parse(
        file: &std::path::Path,
        failure: crate::config_format::ParseFailure,
    ) -> Self {
        let mut result = Self::at(
            Some(file.into()),
            "$",
            if failure.syntax {
                "malformed JSONC"
            } else {
                "invalid document shape or unknown field"
            },
            "correct this document using its authoring schema",
            String::new(),
        );
        result.diagnostic.line = failure.line;
        result.diagnostic.column = failure.column;
        result.detail = format!("{}: {}", file.display(), failure.into_detail());
        result
    }
}
impl From<String> for LaunchFailure {
    fn from(detail: String) -> Self {
        Self::at(
            None,
            "$",
            "launch semantic validation failed",
            "check local references, model selection, paths, and field authority",
            detail,
        )
    }
}
impl From<&str> for LaunchFailure {
    fn from(detail: &str) -> Self {
        detail.to_owned().into()
    }
}
impl From<LaunchFailure> for String {
    fn from(failure: LaunchFailure) -> Self {
        failure.detail
    }
}
impl std::fmt::Display for LaunchFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}
impl std::fmt::Debug for LaunchFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.diagnostic.fmt(f)
    }
}
impl std::error::Error for LaunchFailure {}

#[derive(Debug, Serialize)]
pub struct SourceProjection {
    pub activation: SourceActivation,
    /// None means package contents were not inspected (or this is not Python).
    pub local_status: Option<super::launch::PythonLocalStatus>,
    pub readiness: &'static str,
    pub reason: &'static str,
}

#[derive(Debug, Serialize)]
pub struct LaunchProjection {
    pub roles:
        BTreeMap<crate::runtime::subagent::SubagentName, super::subagent_resources::RoleSource>,
    pub workspace: PathBuf,
    pub runtime_root: PathBuf,
    pub trusted: bool,
    pub selected_model: String,
    pub configuration: Value,
    pub provenance: BTreeMap<String, ProvenanceProjection>,
    pub sources: BTreeMap<String, SourceProjection>,
    pub tool_selection: Value,
    pub registered_workflows: Vec<String>,
    pub local_skills: Vec<String>,
    pub provider: Value,
}

#[derive(Debug, Serialize)]
pub struct ProvenanceProjection {
    pub origin: Origin,
    pub authority: &'static str,
    pub reason: &'static str,
}

/// Only facts already established by the shared resolver before it stopped.
#[derive(Debug, Serialize)]
pub struct PartialProjection {
    pub workspace: PathBuf,
    pub runtime_root: PathBuf,
    pub trusted: bool,
    pub configuration: Value,
}

impl PartialProjection {
    pub(super) fn new(
        locations: &super::launch::LaunchLocations,
        trusted: bool,
        mut configuration: Value,
    ) -> Self {
        redact(&mut configuration);
        Self {
            workspace: locations.workspace.clone(),
            runtime_root: locations.runtime_root.clone(),
            trusted,
            configuration,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub version: u32,
    pub operation: &'static str,
    pub scope: &'static str,
    pub validity: Validity,
    pub readiness: Option<Readiness>,
    pub diagnostics: Vec<Diagnostic>,
    pub launch: Option<LaunchProjection>,
    pub partial: Option<Box<PartialProjection>>,
    pub initialization: Option<super::initialization::InitializationResult>,
    pub projection_omitted: bool,
}

impl Report {
    pub(super) fn new(operation: &'static str) -> Self {
        Self {
            version: 1,
            operation,
            scope: "prospective_next_launch",
            validity: Validity::Valid,
            readiness: if operation == "init" {
                None
            } else {
                Some(Readiness::Unresolved)
            },
            diagnostics: Vec::new(),
            launch: None,
            partial: None,
            initialization: None,
            projection_omitted: false,
        }
    }

    pub(super) fn failure(
        operation: &'static str,
        file: Option<PathBuf>,
        path: &str,
        reason: &str,
        correction: &str,
    ) -> Self {
        let mut report = Self::new(operation);
        report.validity = Validity::Invalid;
        report.diagnostics.push(Diagnostic {
            classification: "error",
            category: "invalid",
            file,
            path: path.into(),
            reason: reason.into(),
            correction: correction.into(),
            line: None,
            column: None,
        });
        report
    }

    pub(super) fn exit_code(&self) -> i32 {
        match (self.validity, self.readiness) {
            (Validity::Invalid, _) => 2,
            (Validity::Incomplete, _) | (_, Some(Readiness::Unresolved)) => 3,
            (Validity::Valid, None) => 0,
        }
    }

    pub(super) fn render(&self, json_output: bool) -> String {
        let header = format!(
            "{}: {:?}; {:?} (prospective next launch; partial if incomplete or projection_omitted)\n",
            self.operation, self.validity, self.readiness,
        );
        let value = self.bounded_value(header.len());
        if json_output {
            serde_json::to_string(&value).expect("report serializes")
        } else {
            format!(
                "{header}{}",
                serde_json::to_string_pretty(&value).expect("report serializes")
            )
        }
    }

    fn bounded_value(&self, header_bytes: usize) -> Value {
        let mut value = serde_json::to_value(self).expect("report serializes");
        // One structured value for both renderers. Reserve the human header and
        // the command's trailing newline, and measure escaped, pretty JSON.
        let fits = |value: &Value| {
            serde_json::to_vec_pretty(value)
                .expect("report serializes")
                .len()
                + header_bytes
                + 1
                < OUTPUT_LIMIT
        };
        if fits(&value) {
            return value;
        }
        value["launch"] = Value::Null;
        value["partial"] = Value::Null;
        value["projection_omitted"] = json!(true);
        let projection_warning = json!(Diagnostic {
            classification: "warning", category: "projection_limit", file: None, path: "$".into(),
            reason: "projection omitted because it exceeds the 256 KiB output bound; validity/readiness are unchanged".into(),
            correction: "inspect smaller authoring documents; this output is partial".into(), line: None, column: None,
        });
        value["diagnostics"]
            .as_array_mut()
            .expect("diagnostics array")
            .push(projection_warning.clone());
        if fits(&value) {
            return value;
        }

        // Classification is supplied by the authoritative owner, not inferred
        // from error prose. Prioritize the first cause, then retain a prefix of
        // the remaining diagnostics in their original order.
        let causal = self
            .diagnostics
            .iter()
            .position(|diagnostic| match self.validity {
                Validity::Invalid => {
                    diagnostic.category == "invalid" && diagnostic.classification == "error"
                }
                Validity::Incomplete => diagnostic.category == "incomplete",
                Validity::Valid => false,
            });
        let Value::Array(mut original) = value["diagnostics"].take() else {
            unreachable!("diagnostics array")
        };
        original.pop(); // The omission warning is reserved below, not truncated.
        value["diagnostics"] = json!([projection_warning, Diagnostic {
            classification: "warning", category: "diagnostics_truncated", file: None, path: "diagnostics".into(),
            reason: "diagnostic count or text was truncated to preserve the first causal diagnostic within the output bound".into(),
            correction: "correct the retained cause and check again for subsequent diagnostics".into(), line: None, column: None,
        }]);
        let mut retained = 0;
        if let Some(index) = causal {
            value["diagnostics"]
                .as_array_mut()
                .unwrap()
                .insert(0, original[index].clone());
            if !fits(&value) {
                // A single pathological record must not consume the report.
                // Summarize only already-redacted text, preserving field names,
                // classification, location and UTF-8/JSON record integrity.
                summarize_diagnostic(&mut value["diagnostics"][0]);
            }
            retained = 1;
        }
        for (index, diagnostic) in original.into_iter().enumerate() {
            if Some(index) == causal {
                continue;
            }
            value["diagnostics"]
                .as_array_mut()
                .unwrap()
                .insert(retained, diagnostic);
            if !fits(&value) {
                value["diagnostics"]
                    .as_array_mut()
                    .unwrap()
                    .remove(retained);
                break;
            }
            retained += 1;
        }
        value
    }
}

fn summarize_diagnostic(diagnostic: &mut Value) {
    for field in ["file", "path", "reason", "correction"] {
        if let Some(text) = diagnostic[field].as_str()
            && text.len() > 1024
        {
            let mut end = 1024;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            diagnostic[field] = json!(format!("{}… [truncated]", &text[..end]));
        }
    }
}

pub(super) fn inspect(
    operation: &'static str,
    request: &LaunchRequest,
    host: &HostEnvironment,
) -> (Report, Option<ProspectiveLaunch>) {
    match super::launch::analyze(request, host) {
        Ok(launch) => (project(operation, &launch), Some(launch)),
        Err(mut failure) => {
            let mut report = Report::new(operation);
            report.validity = if failure.incomplete {
                Validity::Incomplete
            } else {
                Validity::Invalid
            };
            if failure.incomplete {
                report.readiness = Some(Readiness::Unresolved);
                failure.diagnostic.category = "incomplete";
                failure.diagnostic.classification = "warning";
            }
            report.partial = failure.partial;
            report.diagnostics.push(*failure.diagnostic);
            (report, None)
        }
    }
}

#[allow(clippy::too_many_lines)] // One redacted projection, no alternate resolution.
fn project(operation: &'static str, launch: &ProspectiveLaunch) -> Report {
    let mut report = Report::new(operation);
    report.diagnostics.push(Diagnostic {
        classification: "warning",
        category: "unresolved",
        file: None,
        path: format!("providers.{}", launch.config.model.model.provider()),
        reason: "provider credential availability, endpoint connectivity and model compatibility were not verified".into(),
        correction: "supply credentials at runtime; static validation does not verify provider execution".into(),
        line: None,
        column: None,
    });
    if !launch.trusted {
        report.readiness = Some(Readiness::Unresolved);
        report.diagnostics.push(Diagnostic {
            classification: "warning",
            category: "unresolved",
            file: None,
            path: "workspace.trust".into(),
            reason: "workspace is untrusted; project resources remain inert".into(),
            correction: "review the project and explicitly run rustx --trust grant".into(),
            line: None,
            column: None,
        });
    }
    let mut sources = BTreeMap::new();
    for (name, activation) in &launch.source_activations {
        let activation = *activation;
        let local_status = launch.python_local_status.get(name).copied();
        let (mut readiness, mut reason) = match activation {
            SourceActivation::Enabled => {
                report.readiness = Some(Readiness::Unresolved);
                (
                    "unresolved",
                    "capabilities require explicit online discovery",
                )
            }
            SourceActivation::Disabled => ("inert", "explicitly disabled; not loaded"),
            SourceActivation::Unconfigured => ("inert", "no explicit activation grant; not loaded"),
            SourceActivation::Untrusted => ("inert", "host trust has not admitted this source"),
        };
        let local_failure = matches!(
            local_status,
            Some(PythonLocalStatus::Missing | PythonLocalStatus::Invalid)
        );
        if local_failure {
            report.validity = Validity::Invalid;
            readiness = "unavailable";
            reason = if local_status == Some(PythonLocalStatus::Missing) {
                "enabled managed Python package is not present locally"
            } else {
                "enabled managed Python package violates the local package contract"
            };
        }
        let path = if launch.config.mcp_servers.contains_key(name) {
            format!("mcpServers.{name}")
        } else {
            format!("pythonSources.{name}")
        };
        let file = launch
            .provenance
            .get(&path)
            .and_then(|origin| match origin {
                Origin::User { document, .. } | Origin::Project { document, .. } => {
                    Some(document.clone())
                }
                Origin::Builtin | Origin::Cli { .. } => None,
            });
        report.diagnostics.push(Diagnostic {
            classification: if local_failure {
                "error"
            } else if readiness == "unresolved" {
                "warning"
            } else {
                "info"
            },
            category: if local_failure { "invalid" } else { readiness },
            file,
            path,
            reason: reason.into(),
            correction: if local_failure {
                "create or repair .agents/tools/<package>: use a valid package name, regular server.py and requirements.txt, and bounded symlink-free package files; otherwise disable this source"
            } else { match activation {
                SourceActivation::Enabled => {
                    "use doctor --probe for explicitly authorized readiness checks"
                }
                SourceActivation::Untrusted => {
                    "review the workspace and grant trust explicitly before activation"
                }
                SourceActivation::Disabled | SourceActivation::Unconfigured => {
                    "leave inert, or explicitly configure enablement after reviewing the source"
                }
            } }
            .into(),
            line: None,
            column: None,
        });
        sources.insert(
            name.to_string(),
            SourceProjection {
                activation,
                local_status,
                readiness,
                reason,
            },
        );
    }
    let mut configuration =
        serde_json::to_value(&*launch.config).expect("configuration serializes");
    redact(&mut configuration);
    report.launch = Some(LaunchProjection {
        roles: launch.role_sources.clone(),
        local_skills: launch.skill_names.clone(),
        provider: launch.models.providers().find(|provider| &provider.id == launch.config.model.model.provider()).map_or(Value::Null, |provider| json!({"id":provider.id, "endpoint":provider.base_url, "credential":provider.api_key.view(), "verification":"deferred; no credential value or connectivity was checked"})),
        workspace: launch.workspace.clone(),
        runtime_root: launch.runtime_root.clone(),
        trusted: launch.trusted,
        selected_model: launch.config.model.model.to_string(),
        configuration,
        provenance: launch.provenance.iter().map(|(field, origin)| {
            let (authority, reason) = match origin {
                Origin::Builtin => ("builtin", "no higher-precedence declaration; domain default applies"),
                Origin::User { .. } => ("user", "user declaration overrides builtin; no admitted higher-precedence declaration"),
                Origin::Project { .. } if launch.trusted => ("trusted_project", "project declaration overrides user/builtin within project-owned fields"),
                Origin::Project { .. } => ("untrusted_project", "prospective project declaration; runtime admission is withheld"),
                Origin::Cli { .. } => ("cli", "explicit launch intent has highest precedence"),
            };
            (field.clone(), ProvenanceProjection { origin: origin.clone(), authority, reason })
        }).collect(),
        sources,
        tool_selection: json!({"selected": launch.selected_tools, "noTools":launch.no_tools, "noBuiltinTools":launch.no_builtin_tools,
            "allowlist":launch.tools, "exclusions":launch.exclude_tools, "defaults":launch.config.default_tools,
            "exclusionReason": if launch.no_tools { "--no-tools removes every ordinary main-model Tool" } else { "exact allowlist/default selection followed by final exclusions; no mandatory Read insertion" },
            "onlineIdentities":"unresolved until source discovery"}),
        registered_workflows: launch
            .config
            .workflows
            .definitions
            .iter()
            .map(ToString::to_string)
            .collect(),
    });
    report
}

fn redact(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if matches!(
                    key.as_str(),
                    "environment"
                        | "env"
                        | "headers"
                        | "args"
                        | "requestParams"
                        | "apiKey"
                        | "command"
                        | "url"
                ) {
                    *value = json!("<redacted>");
                } else {
                    redact(value);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                redact(value);
            }
        }
        _ => {}
    }
}
