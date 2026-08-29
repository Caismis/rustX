//! The immutable named subagent definitions admitted into one runtime
//! resource generation (Issue #144).
//!
//! # Ownership
//!
//! ```text
//! SubagentCatalog (this module)
//!   owns: the canonical SubagentName keyspace of one generation, the
//!         immutable SubagentDefinition of each name, and the deterministic
//!         SubagentDefinitionDigest of each definition
//!   never owns: capability resolution, model resolution, live child
//!               lifecycle, capacity, or any mutable runtime-current state
//! ```
//!
//! A catalog is **configuration/resource-generation state**, not live
//! execution state: it is built off-side while a resource generation is
//! prepared, validated against that generation's capability authority, and
//! frozen into [`RuntimeResourceSnapshot`] at the same atomic commit that
//! publishes the generation's capabilities, project instructions, and
//! Skills. A definition therefore never changes underneath an attempt that
//! already owns a generation.
//!
//! [`RuntimeResourceSnapshot`]: crate::runtime::resources::RuntimeResourceSnapshot

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::model::catalog::ModelRef;
use crate::runtime::identity::McpServerId;
use crate::runtime::resources::ProjectContextFile;

/// The maximum number of named agents one catalog may admit.
///
/// The catalog is rendered into the model-facing `subagent` Tool
/// description, so its size is a bounded model-input contract rather than an
/// arbitrary configuration limit.
pub const MAX_SUBAGENT_DEFINITIONS: usize = 32;

/// The maximum byte length of one agent routing description.
pub const MAX_SUBAGENT_DESCRIPTION_BYTES: usize = 512;

/// The maximum byte length of one agent instruction document.
pub const MAX_SUBAGENT_INSTRUCTIONS_BYTES: usize = 64 * 1024;

/// The maximum number of explicit project-instruction files one definition
/// may name.
pub const MAX_SUBAGENT_PROJECT_FILES: usize = 8;

/// The canonical typed name of one admitted subagent definition.
///
/// The keyspace is deliberately narrow: lowercase ASCII letters, digits,
/// `-`, and `_`, starting with a letter. A name is the model-facing routing
/// token, the durable ownership identity, and the Runtime Client projection
/// identity, so an ambiguous or shell-shaped spelling is rejected at the
/// configuration boundary rather than normalized later.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct SubagentName(String);

impl SubagentName {
    /// The maximum byte length of a canonical name.
    pub const MAX_BYTES: usize = 64;

    /// Parses one canonical subagent name.
    ///
    /// # Errors
    ///
    /// Returns the deterministic rejection detail of the first violated
    /// rule.
    pub fn parse(value: &str) -> Result<Self, SubagentNameError> {
        if value.is_empty() {
            return Err(SubagentNameError::Empty);
        }
        if value.len() > Self::MAX_BYTES {
            return Err(SubagentNameError::TooLong { bytes: value.len() });
        }
        match value.chars().next() {
            None => return Err(SubagentNameError::Empty),
            Some(first) if !first.is_ascii_lowercase() => {
                return Err(SubagentNameError::InvalidStart { found: first });
            }
            Some(_) => {}
        }
        if let Some(found) = value
            .chars()
            .find(|character| !matches!(character, 'a'..='z' | '0'..='9' | '-' | '_'))
        {
            return Err(SubagentNameError::InvalidCharacter { found });
        }
        Ok(Self(value.to_owned()))
    }

    /// The canonical name text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl core::fmt::Display for SubagentName {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for SubagentName {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// A canonical subagent-name violation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubagentNameError {
    /// The name is empty.
    Empty,
    /// The name exceeds [`SubagentName::MAX_BYTES`].
    TooLong {
        /// The offending byte length.
        bytes: usize,
    },
    /// The name does not start with a lowercase ASCII letter.
    InvalidStart {
        /// The offending first character.
        found: char,
    },
    /// The name contains a character outside `[a-z0-9_-]`.
    InvalidCharacter {
        /// The offending character.
        found: char,
    },
}

impl core::fmt::Display for SubagentNameError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Empty => formatter.write_str("a subagent name must not be empty"),
            Self::TooLong { bytes } => write!(
                formatter,
                "a subagent name exceeds the {}-byte bound ({bytes} bytes)",
                SubagentName::MAX_BYTES
            ),
            Self::InvalidStart { found } => write!(
                formatter,
                "a subagent name must start with a lowercase ASCII letter, found {found:?}"
            ),
            Self::InvalidCharacter { found } => write!(
                formatter,
                "a subagent name accepts only [a-z0-9_-], found {found:?}"
            ),
        }
    }
}

impl std::error::Error for SubagentNameError {}

/// One source-qualified capability selection of a named definition.
///
/// The three origins share **one** selection vocabulary and **one**
/// resolution core: the selector names the origin explicitly so resolution
/// can never confuse a Builtin `read` with an MCP server's `read`, and the
/// frozen resolution keeps the exact canonical identity of the origin it
/// came from.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "origin", rename_all = "snake_case", deny_unknown_fields)]
pub enum SubagentToolSelector {
    /// A runtime built-in/native capability, selected by its canonical
    /// model-facing name.
    Builtin {
        /// The canonical model-facing name.
        name: String,
    },
    /// One tool of one configured MCP server.
    Mcp {
        /// The authoritative MCP server identity.
        server_id: McpServerId,
        /// The canonical tool name as the server publishes it.
        name: String,
    },
    /// One custom Python tool of the Workspace tool plane.
    Python {
        /// The canonical model-facing name.
        name: String,
    },
}

impl SubagentToolSelector {
    /// The canonical selection text used by digests and diagnostics.
    #[must_use]
    pub fn canonical(&self) -> String {
        match self {
            Self::Builtin { name } => format!("builtin:{name}"),
            Self::Mcp { server_id, name } => format!("mcp:{server_id}/{name}"),
            Self::Python { name } => format!("python:{name}"),
        }
    }
}

impl core::fmt::Display for SubagentToolSelector {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(&self.canonical())
    }
}

/// The project-instruction policy of one named definition.
///
/// Parent-side resource composition owns discovery; this policy decides only
/// how the parent generation's already-discovered chain composes with the
/// definition's own explicit files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentProjectInstructionPolicy {
    /// Whether the invoking generation's normal project instruction chain is
    /// prepended to the explicit files.
    pub inherit: bool,
    /// The explicit definition-owned project instruction resources, already
    /// loaded by parent-side resource composition, in configured order.
    pub files: Vec<ProjectContextFile>,
}

/// The deterministic semantic identity of one named subagent definition.
///
/// The digest covers exactly the normalized semantics that change child
/// behavior. It is computed over a rustX-owned versioned canonical framing —
/// never over raw JSONC bytes — so comments, whitespace, and JSON object
/// insertion order cannot change it, while every semantic change does.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SubagentDefinitionDigest(String);

/// The canonical framing version of [`SubagentDefinitionDigest`].
///
/// It is part of the hashed preimage: a later milestone that admits a new
/// behavior-affecting field bumps this constant, so two framings can never
/// collide into the same digest.
pub const SUBAGENT_DEFINITION_DIGEST_VERSION: &str = "rustx-subagent-definition-v2";

impl SubagentDefinitionDigest {
    /// The stable textual form `sha256:<64 lowercase hex characters>`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl core::fmt::Display for SubagentDefinitionDigest {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// One immutable named subagent definition of a runtime resource generation.
///
/// Everything a child's behavior depends on is already resolved here except
/// the invoking generation's capability/Skill/model authority, which the
/// [`SubagentResolver`](super::resolver::SubagentResolver) applies at
/// invocation time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentDefinition {
    name: SubagentName,
    description: String,
    instructions: String,
    instructions_source: PathBuf,
    model: Option<ModelRef>,
    tools: Vec<SubagentToolSelector>,
    skills: Vec<String>,
    project_instructions: SubagentProjectInstructionPolicy,
    workspace_policy: super::workspace::SubagentWorkspacePolicy,
    digest: SubagentDefinitionDigest,
}

impl SubagentDefinition {
    /// Builds one immutable definition from already-loaded resources and
    /// computes its deterministic digest.
    ///
    /// Tool selectors and Skill selectors are canonically ordered and
    /// deduplicated here, so two configurations that differ only in listing
    /// order produce the same definition and the same digest.
    ///
    /// # Errors
    ///
    /// Returns the first bounded validation failure of the definition
    /// itself. Capability, Skill, and model *authority* validation belongs
    /// to catalog admission against a prepared generation, not here.
    #[allow(clippy::too_many_arguments)] // one definition, one construction boundary
    pub fn new(
        name: SubagentName,
        description: String,
        instructions: String,
        instructions_source: PathBuf,
        model: Option<ModelRef>,
        tools: Vec<SubagentToolSelector>,
        skills: Vec<String>,
        project_instructions: SubagentProjectInstructionPolicy,
        workspace_policy: super::workspace::SubagentWorkspacePolicy,
    ) -> Result<Self, SubagentDefinitionError> {
        if description.trim().is_empty() {
            return Err(SubagentDefinitionError::EmptyDescription { agent: name });
        }
        if description.len() > MAX_SUBAGENT_DESCRIPTION_BYTES {
            return Err(SubagentDefinitionError::DescriptionOversized {
                agent: name,
                bytes: description.len(),
            });
        }
        if instructions.trim().is_empty() {
            return Err(SubagentDefinitionError::EmptyInstructions { agent: name });
        }
        if instructions.len() > MAX_SUBAGENT_INSTRUCTIONS_BYTES {
            return Err(SubagentDefinitionError::InstructionsOversized {
                agent: name,
                bytes: instructions.len(),
            });
        }
        if project_instructions.files.len() > MAX_SUBAGENT_PROJECT_FILES {
            return Err(SubagentDefinitionError::TooManyProjectFiles {
                agent: name,
                count: project_instructions.files.len(),
            });
        }
        // Nested delegation is rejected structurally, at the definition
        // boundary: no admitted definition can name the `subagent`
        // intrinsic, so no resolution path has to defend against it later.
        if let Some(selector) = tools.iter().find(|selector| {
            matches!(
                selector,
                SubagentToolSelector::Builtin { name }
                    if name == crate::tools::native::SUBAGENT_TOOL_NAME
            )
        }) {
            return Err(SubagentDefinitionError::RecursiveSelector {
                agent: name,
                selector: selector.canonical(),
            });
        }
        if let Some(selector) = tools.iter().find(|selector| {
            matches!(
                selector,
                SubagentToolSelector::Builtin { name }
                    if CHILD_UNSAFE_BUILTIN_TOOLS.contains(&name.as_str())
            )
        }) {
            return Err(SubagentDefinitionError::ChildUnsafeSelector {
                agent: name,
                selector: selector.canonical(),
            });
        }
        let mut tools = tools;
        tools.sort();
        tools.dedup();
        let mut skills = skills;
        skills.sort();
        skills.dedup();
        if let Some(empty) = skills.iter().find(|skill| skill.trim().is_empty()) {
            let _ = empty;
            return Err(SubagentDefinitionError::EmptySkillSelector { agent: name });
        }
        let digest = compute_digest(
            &name,
            &description,
            &instructions,
            model.as_ref(),
            &tools,
            &skills,
            &project_instructions,
            workspace_policy,
        );
        Ok(Self {
            name,
            description,
            instructions,
            instructions_source,
            model,
            tools,
            skills,
            project_instructions,
            workspace_policy,
            digest,
        })
    }

    /// The canonical agent name.
    #[must_use]
    pub const fn name(&self) -> &SubagentName {
        &self.name
    }

    /// The bounded model-facing routing description.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// The exact child instruction document loaded for this generation.
    #[must_use]
    pub fn instructions(&self) -> &str {
        &self.instructions
    }

    /// The canonical source identity of the instruction document.
    #[must_use]
    pub fn instructions_source(&self) -> &std::path::Path {
        &self.instructions_source
    }

    /// The explicit model selection, when the definition names one.
    #[must_use]
    pub const fn model(&self) -> Option<&ModelRef> {
        self.model.as_ref()
    }

    /// The canonically ordered typed Tool selectors.
    #[must_use]
    pub fn tools(&self) -> &[SubagentToolSelector] {
        &self.tools
    }

    /// The canonically ordered exact Skill allowlist.
    #[must_use]
    pub fn skills(&self) -> &[String] {
        &self.skills
    }

    /// The project-instruction inheritance policy and explicit resources.
    #[must_use]
    pub const fn project_instructions(&self) -> &SubagentProjectInstructionPolicy {
        &self.project_instructions
    }

    /// The resolved project-workspace policy of this definition.
    #[must_use]
    pub const fn workspace_policy(&self) -> super::workspace::SubagentWorkspacePolicy {
        self.workspace_policy
    }

    /// The deterministic semantic identity of this definition.
    #[must_use]
    pub const fn digest(&self) -> &SubagentDefinitionDigest {
        &self.digest
    }
}

/// Native capabilities whose lifecycle owner cannot exist in a headless
/// child, and which are therefore not selectable by a named definition.
///
/// `ask_user` is the exact case: the native Questionnaire capability needs a
/// Runtime Client questionnaire authority, and a subagent child is composed
/// without any Runtime Client host at all. `background_task` is the second:
/// its lifecycle owner is the *conversation* that outlives the attempt, and
/// a one-shot child conversation terminates with its single answer, so a
/// detached execution in a child has no owner to settle it.
///
/// Naming either is a configuration error rather than a silently dropped
/// capability. This is a short explicit list of known lifecycle owners
/// reviewed against their actual owners, not a generic deny-policy
/// framework.
pub const CHILD_UNSAFE_BUILTIN_TOOLS: [&str; 2] = [
    crate::tools::executor::ASK_USER_TOOL_NAME,
    "background_task",
];

/// A definition-level validation failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubagentDefinitionError {
    /// The routing description is empty.
    EmptyDescription {
        /// The offending agent.
        agent: SubagentName,
    },
    /// The routing description exceeds its bound.
    DescriptionOversized {
        /// The offending agent.
        agent: SubagentName,
        /// The offending byte length.
        bytes: usize,
    },
    /// The instruction document is empty.
    EmptyInstructions {
        /// The offending agent.
        agent: SubagentName,
    },
    /// The instruction document exceeds its bound.
    InstructionsOversized {
        /// The offending agent.
        agent: SubagentName,
        /// The offending byte length.
        bytes: usize,
    },
    /// The definition names more explicit project instruction files than the
    /// bound allows.
    TooManyProjectFiles {
        /// The offending agent.
        agent: SubagentName,
        /// The offending count.
        count: usize,
    },
    /// The definition selects the `subagent` intrinsic. Nested delegation is
    /// unsupported and is rejected structurally.
    RecursiveSelector {
        /// The offending agent.
        agent: SubagentName,
        /// The offending selector.
        selector: String,
    },
    /// The definition selects a capability whose lifecycle owner cannot
    /// exist in a headless child.
    ChildUnsafeSelector {
        /// The offending agent.
        agent: SubagentName,
        /// The offending selector.
        selector: String,
    },
    /// A Skill selector is empty.
    EmptySkillSelector {
        /// The offending agent.
        agent: SubagentName,
    },
    /// The catalog exceeds [`MAX_SUBAGENT_DEFINITIONS`].
    TooManyDefinitions {
        /// The offending count.
        count: usize,
    },
}

impl core::fmt::Display for SubagentDefinitionError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::EmptyDescription { agent } => {
                write!(formatter, "subagent {agent:?} has an empty description")
            }
            Self::DescriptionOversized { agent, bytes } => write!(
                formatter,
                "subagent {agent:?} description exceeds the \
                 {MAX_SUBAGENT_DESCRIPTION_BYTES}-byte bound ({bytes} bytes)"
            ),
            Self::EmptyInstructions { agent } => write!(
                formatter,
                "subagent {agent:?} instructions document is empty"
            ),
            Self::InstructionsOversized { agent, bytes } => write!(
                formatter,
                "subagent {agent:?} instructions exceed the \
                 {MAX_SUBAGENT_INSTRUCTIONS_BYTES}-byte bound ({bytes} bytes)"
            ),
            Self::TooManyProjectFiles { agent, count } => write!(
                formatter,
                "subagent {agent:?} names {count} project instruction files; at most \
                 {MAX_SUBAGENT_PROJECT_FILES} are admitted"
            ),
            Self::RecursiveSelector { agent, selector } => write!(
                formatter,
                "subagent {agent:?} selects {selector}: nested subagent delegation is \
                 unsupported"
            ),
            Self::ChildUnsafeSelector { agent, selector } => write!(
                formatter,
                "subagent {agent:?} selects {selector}, whose lifecycle owner does not \
                 exist in a headless child runtime"
            ),
            Self::EmptySkillSelector { agent } => {
                write!(formatter, "subagent {agent:?} names an empty Skill")
            }
            Self::TooManyDefinitions { count } => write!(
                formatter,
                "{count} subagent definitions exceed the {MAX_SUBAGENT_DEFINITIONS} bound"
            ),
        }
    }
}

impl std::error::Error for SubagentDefinitionError {}

/// The immutable named-definition catalog of one runtime resource
/// generation.
///
/// The catalog is keyed by canonical [`SubagentName`], so a name is unique by
/// construction and iteration order is deterministic.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubagentCatalog {
    agents: BTreeMap<SubagentName, Arc<SubagentDefinition>>,
}

impl SubagentCatalog {
    /// The empty catalog: a runtime generation that admits no named agent.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Builds one catalog from complete definitions.
    ///
    /// # Errors
    ///
    /// Returns [`SubagentDefinitionError::TooManyDefinitions`] when the set
    /// exceeds [`MAX_SUBAGENT_DEFINITIONS`].
    pub fn new(
        definitions: impl IntoIterator<Item = SubagentDefinition>,
    ) -> Result<Self, SubagentDefinitionError> {
        let agents: BTreeMap<SubagentName, Arc<SubagentDefinition>> = definitions
            .into_iter()
            .map(|definition| (definition.name.clone(), Arc::new(definition)))
            .collect();
        if agents.len() > MAX_SUBAGENT_DEFINITIONS {
            return Err(SubagentDefinitionError::TooManyDefinitions {
                count: agents.len(),
            });
        }
        Ok(Self { agents })
    }

    /// Whether the catalog admits no named agent.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }

    /// The number of admitted definitions.
    #[must_use]
    pub fn len(&self) -> usize {
        self.agents.len()
    }

    /// Looks one admitted definition up by canonical name.
    #[must_use]
    pub fn get(&self, name: &SubagentName) -> Option<&Arc<SubagentDefinition>> {
        self.agents.get(name)
    }

    /// Every admitted definition in canonical name order.
    pub fn definitions(&self) -> impl Iterator<Item = &Arc<SubagentDefinition>> {
        self.agents.values()
    }

    /// The canonical admitted names, in order.
    #[must_use]
    pub fn names(&self) -> Vec<&SubagentName> {
        self.agents.keys().collect()
    }
}

/// Computes the deterministic digest over the versioned canonical framing.
///
/// The preimage is a line-oriented, length-prefixed encoding of exactly the
/// semantics that change child behavior. Map/set-like inputs are already
/// canonically ordered by [`SubagentDefinition::new`], and every variable
/// length value is length-prefixed, so no two distinct definitions can frame
/// to the same preimage by concatenation.
#[allow(clippy::too_many_arguments)] // the digest framing is the semantic input boundary
fn compute_digest(
    name: &SubagentName,
    description: &str,
    instructions: &str,
    model: Option<&ModelRef>,
    tools: &[SubagentToolSelector],
    skills: &[String],
    project_instructions: &SubagentProjectInstructionPolicy,
    workspace_policy: super::workspace::SubagentWorkspacePolicy,
) -> SubagentDefinitionDigest {
    let mut hasher = Sha256::new();
    hasher.update(SUBAGENT_DEFINITION_DIGEST_VERSION.as_bytes());
    hasher.update(b"\n");
    field(&mut hasher, "name", name.as_str());
    field(&mut hasher, "description", description);
    field(&mut hasher, "instructions", instructions);
    match model {
        // The two model semantics are distinct facts, not a present/absent
        // string: an agent that explicitly names the model the attempt
        // happens to use is not the same definition as one that inherits.
        None => field(&mut hasher, "model", "\u{0}inherit"),
        Some(model) => field(&mut hasher, "model", &format!("explicit:{model}")),
    }
    count(&mut hasher, "tools", tools.len());
    for selector in tools {
        field(&mut hasher, "tool", &selector.canonical());
    }
    count(&mut hasher, "skills", skills.len());
    for skill in skills {
        field(&mut hasher, "skill", skill);
    }
    field(
        &mut hasher,
        "project_instructions_inherit",
        if project_instructions.inherit {
            "true"
        } else {
            "false"
        },
    );
    count(
        &mut hasher,
        "project_instruction_files",
        project_instructions.files.len(),
    );
    for file in &project_instructions.files {
        // Content, not merely the path: a definition whose explicit
        // instruction file changed is semantically a different definition,
        // and an already-committed child must keep the digest it started
        // with.
        field(
            &mut hasher,
            "project_instruction_path",
            &file.path.display().to_string(),
        );
        field(&mut hasher, "project_instruction_content", &file.content);
    }
    let workspace = match workspace_policy {
        super::workspace::SubagentWorkspacePolicy::SharedWorkspace => "shared".to_owned(),
        super::workspace::SubagentWorkspacePolicy::GitWorktree {
            require_clean_parent,
        } => format!("git_worktree:require_clean_parent={require_clean_parent}"),
    };
    field(&mut hasher, "workspace_policy", &workspace);
    SubagentDefinitionDigest(format!("sha256:{:x}", hasher.finalize()))
}

fn field(hasher: &mut Sha256, key: &str, value: &str) {
    hasher.update(format!("{key}={}\n", value.len()).as_bytes());
    hasher.update(value.as_bytes());
    hasher.update(b"\n");
}

fn count(hasher: &mut Sha256, key: &str, value: usize) {
    hasher.update(format!("{key}#{value}\n").as_bytes());
}

#[cfg(test)]
mod tests {
    use super::{
        SubagentCatalog, SubagentDefinition, SubagentDefinitionError, SubagentName,
        SubagentNameError, SubagentProjectInstructionPolicy, SubagentToolSelector,
    };
    use crate::runtime::identity::McpServerId;
    use crate::runtime::resources::ProjectContextFile;
    use crate::runtime::subagent::SubagentWorkspacePolicy;

    fn policy() -> SubagentProjectInstructionPolicy {
        SubagentProjectInstructionPolicy {
            inherit: true,
            files: Vec::new(),
        }
    }

    fn definition(
        name: &str,
        tools: Vec<SubagentToolSelector>,
        skills: Vec<String>,
    ) -> Result<SubagentDefinition, SubagentDefinitionError> {
        SubagentDefinition::new(
            SubagentName::parse(name).expect("canonical name"),
            "a description".to_owned(),
            "instructions".to_owned(),
            std::path::PathBuf::from("/w/.rustx/subagents/explore.md"),
            None,
            tools,
            skills,
            policy(),
            SubagentWorkspacePolicy::SharedWorkspace,
        )
    }

    #[test]
    fn canonical_names_reject_invalid_spellings_deterministically() {
        assert!(SubagentName::parse("explore").is_ok());
        assert!(SubagentName::parse("deep-research_2").is_ok());
        assert_eq!(SubagentName::parse(""), Err(SubagentNameError::Empty));
        assert_eq!(
            SubagentName::parse("Explore"),
            Err(SubagentNameError::InvalidStart { found: 'E' })
        );
        assert_eq!(
            SubagentName::parse("2explore"),
            Err(SubagentNameError::InvalidStart { found: '2' })
        );
        assert_eq!(
            SubagentName::parse("explore/child"),
            Err(SubagentNameError::InvalidCharacter { found: '/' })
        );
        assert_eq!(
            SubagentName::parse(&"a".repeat(SubagentName::MAX_BYTES + 1)),
            Err(SubagentNameError::TooLong {
                bytes: SubagentName::MAX_BYTES + 1
            })
        );
    }

    #[test]
    fn selector_order_and_repetition_do_not_change_the_digest() {
        let ordered = definition(
            "explore",
            vec![
                SubagentToolSelector::Builtin {
                    name: "glob".to_owned(),
                },
                SubagentToolSelector::Builtin {
                    name: "read".to_owned(),
                },
                SubagentToolSelector::Mcp {
                    server_id: McpServerId::new("github"),
                    name: "get_issue".to_owned(),
                },
            ],
            vec!["b-skill".to_owned(), "a-skill".to_owned()],
        )
        .expect("definition");
        let shuffled = definition(
            "explore",
            vec![
                SubagentToolSelector::Mcp {
                    server_id: McpServerId::new("github"),
                    name: "get_issue".to_owned(),
                },
                SubagentToolSelector::Builtin {
                    name: "read".to_owned(),
                },
                SubagentToolSelector::Builtin {
                    name: "read".to_owned(),
                },
                SubagentToolSelector::Builtin {
                    name: "glob".to_owned(),
                },
            ],
            vec!["a-skill".to_owned(), "b-skill".to_owned()],
        )
        .expect("definition");
        assert_eq!(ordered.digest(), shuffled.digest());
        assert_eq!(ordered.tools(), shuffled.tools());
    }

    #[test]
    fn semantically_different_definitions_have_different_digests() {
        let base = definition(
            "explore",
            vec![SubagentToolSelector::Builtin {
                name: "read".to_owned(),
            }],
            Vec::new(),
        )
        .expect("definition");
        let more_tools = definition(
            "explore",
            vec![
                SubagentToolSelector::Builtin {
                    name: "read".to_owned(),
                },
                SubagentToolSelector::Builtin {
                    name: "grep".to_owned(),
                },
            ],
            Vec::new(),
        )
        .expect("definition");
        let other_origin = definition(
            "explore",
            vec![SubagentToolSelector::Python {
                name: "read".to_owned(),
            }],
            Vec::new(),
        )
        .expect("definition");
        let other_name = definition(
            "research",
            vec![SubagentToolSelector::Builtin {
                name: "read".to_owned(),
            }],
            Vec::new(),
        )
        .expect("definition");
        let with_skill = definition(
            "explore",
            vec![SubagentToolSelector::Builtin {
                name: "read".to_owned(),
            }],
            vec!["navigation".to_owned()],
        )
        .expect("definition");
        let mut digests = vec![
            base.digest().clone(),
            more_tools.digest().clone(),
            other_origin.digest().clone(),
            other_name.digest().clone(),
            with_skill.digest().clone(),
        ];
        digests.sort();
        digests.dedup();
        assert_eq!(digests.len(), 5, "every semantic change changes the digest");
    }

    #[test]
    fn instruction_and_project_policy_changes_change_the_digest() {
        let inherit = definition("explore", Vec::new(), Vec::new()).expect("definition");
        let explicit_only = SubagentDefinition::new(
            SubagentName::parse("explore").expect("name"),
            "a description".to_owned(),
            "instructions".to_owned(),
            std::path::PathBuf::from("/w/.rustx/subagents/explore.md"),
            None,
            Vec::new(),
            Vec::new(),
            SubagentProjectInstructionPolicy {
                inherit: false,
                files: Vec::new(),
            },
            SubagentWorkspacePolicy::SharedWorkspace,
        )
        .expect("definition");
        let with_file = SubagentDefinition::new(
            SubagentName::parse("explore").expect("name"),
            "a description".to_owned(),
            "instructions".to_owned(),
            std::path::PathBuf::from("/w/.rustx/subagents/explore.md"),
            None,
            Vec::new(),
            Vec::new(),
            SubagentProjectInstructionPolicy {
                inherit: true,
                files: vec![ProjectContextFile {
                    path: std::path::PathBuf::from("/w/.rustx/subagents/explore/AGENTS.md"),
                    content: "explicit".to_owned(),
                }],
            },
            SubagentWorkspacePolicy::SharedWorkspace,
        )
        .expect("definition");
        let changed_content = SubagentDefinition::new(
            SubagentName::parse("explore").expect("name"),
            "a description".to_owned(),
            "instructions".to_owned(),
            std::path::PathBuf::from("/w/.rustx/subagents/explore.md"),
            None,
            Vec::new(),
            Vec::new(),
            SubagentProjectInstructionPolicy {
                inherit: true,
                files: vec![ProjectContextFile {
                    path: std::path::PathBuf::from("/w/.rustx/subagents/explore/AGENTS.md"),
                    content: "explicit, revised".to_owned(),
                }],
            },
            SubagentWorkspacePolicy::SharedWorkspace,
        )
        .expect("definition");
        let changed_instructions = SubagentDefinition::new(
            SubagentName::parse("explore").expect("name"),
            "a description".to_owned(),
            "different instructions".to_owned(),
            std::path::PathBuf::from("/w/.rustx/subagents/explore.md"),
            None,
            Vec::new(),
            Vec::new(),
            policy(),
            SubagentWorkspacePolicy::SharedWorkspace,
        )
        .expect("definition");
        let isolated = SubagentDefinition::new(
            SubagentName::parse("explore").expect("name"),
            "a description".to_owned(),
            "instructions".to_owned(),
            std::path::PathBuf::from("/w/.rustx/subagents/explore.md"),
            None,
            Vec::new(),
            Vec::new(),
            policy(),
            SubagentWorkspacePolicy::GitWorktree {
                require_clean_parent: false,
            },
        )
        .expect("definition");
        let mut digests = vec![
            inherit.digest().clone(),
            explicit_only.digest().clone(),
            with_file.digest().clone(),
            changed_content.digest().clone(),
            changed_instructions.digest().clone(),
            isolated.digest().clone(),
        ];
        digests.sort();
        digests.dedup();
        assert_eq!(digests.len(), 6);
    }

    #[test]
    fn the_instruction_source_path_is_not_a_semantic_digest_input() {
        let here = SubagentDefinition::new(
            SubagentName::parse("explore").expect("name"),
            "a description".to_owned(),
            "instructions".to_owned(),
            std::path::PathBuf::from("/w/a.md"),
            None,
            Vec::new(),
            Vec::new(),
            policy(),
            SubagentWorkspacePolicy::SharedWorkspace,
        )
        .expect("definition");
        let there = SubagentDefinition::new(
            SubagentName::parse("explore").expect("name"),
            "a description".to_owned(),
            "instructions".to_owned(),
            std::path::PathBuf::from("/elsewhere/b.md"),
            None,
            Vec::new(),
            Vec::new(),
            policy(),
            SubagentWorkspacePolicy::SharedWorkspace,
        )
        .expect("definition");
        assert_eq!(
            here.digest(),
            there.digest(),
            "the instruction content is the semantic identity; its host location is not"
        );
    }

    #[test]
    fn recursive_and_child_unsafe_selectors_are_rejected_structurally() {
        assert!(matches!(
            definition(
                "explore",
                vec![SubagentToolSelector::Builtin {
                    name: "subagent".to_owned()
                }],
                Vec::new(),
            ),
            Err(SubagentDefinitionError::RecursiveSelector { .. })
        ));
        assert!(matches!(
            definition(
                "explore",
                vec![SubagentToolSelector::Builtin {
                    name: "ask_user".to_owned()
                }],
                Vec::new(),
            ),
            Err(SubagentDefinitionError::ChildUnsafeSelector { .. })
        ));
    }

    #[test]
    fn the_catalog_is_keyed_and_bounded() {
        let catalog = SubagentCatalog::new([
            definition("research", Vec::new(), Vec::new()).expect("definition"),
            definition("explore", Vec::new(), Vec::new()).expect("definition"),
        ])
        .expect("catalog");
        assert_eq!(
            catalog
                .names()
                .into_iter()
                .map(SubagentName::as_str)
                .collect::<Vec<_>>(),
            vec!["explore", "research"],
            "iteration is canonical name order, not configuration order"
        );
        assert!(
            catalog
                .get(&SubagentName::parse("explore").expect("name"))
                .is_some()
        );
        assert!(
            catalog
                .get(&SubagentName::parse("missing").expect("name"))
                .is_none()
        );

        let too_many = (0..=super::MAX_SUBAGENT_DEFINITIONS)
            .map(|index| definition(&format!("agent-{index}"), Vec::new(), Vec::new()))
            .collect::<Result<Vec<_>, _>>()
            .expect("definitions");
        assert!(matches!(
            SubagentCatalog::new(too_many),
            Err(SubagentDefinitionError::TooManyDefinitions { .. })
        ));
    }
}
