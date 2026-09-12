//! The one provider-independent invocation-scoped capability override
//! shared by both child launch sites (Issue #258).
//!
//! ```text
//! SubagentDefinition            the canonical DEFAULT child profile
//!         +
//! SubagentInvocationOverride    replacement of selected dimensions
//!         +
//! caller delegation authority   who may ask for what
//!         +
//! RuntimeResourceSnapshot Rn    what this generation admits at all
//!         |
//!         v
//! ResolvedSubagentSpec          the complete immutable execution contract
//! ```
//!
//! # One rule, three dimensions
//!
//! > A **missing** override dimension uses the definition's value. A
//! > **present** override dimension replaces that dimension completely.
//!
//! There is no additive mode, no removal mode, no wildcard, and no recursive
//! merge. `"tools": {"builtin": ["bash"]}` is the child's whole tool
//! selection, not the role's selection plus Bash; `"extensions": {}` is the
//! empty composition, not the role's composition with nothing changed.
//! Replacement applies per dimension and independently: an override naming
//! only `skills` leaves tools and extensions exactly as the role authored
//! them.
//!
//! # Why presence, not emptiness, is the signal
//!
//! The three dimensions all have a legitimate empty value — no tools, no
//! Skills, no composed extension — so "empty means inherit" would make those
//! three states unsayable. Each dimension is therefore an explicit `Option`
//! whose `None` is produced only by an absent key. An explicit `null` is
//! rejected rather than folded into absence, so one wire spelling never
//! carries two meanings.
//!
//! # This type carries no authority
//!
//! A request is what the caller *asked for*. Whether the caller may ask is
//! [`SubagentOverrideAuthority`], a typed native input supplied by the launch
//! site, never a field the model can set.
//!
//! [`SubagentOverrideAuthority`]: super::resolver::SubagentOverrideAuthority

use serde::{Deserialize, Serialize};

use crate::capabilities::selection::{ToolSelectionDocument, ToolSelector};
use crate::extensions::{NativeAgentExtensionSelection, NativeAgentExtensions};

use super::catalog::{CHILD_UNSAFE_BUILTIN_TOOLS, SubagentDefinition};

/// The maximum number of Skills one invocation override may select.
///
/// The bound mirrors the model-facing input posture of every other bounded
/// `subagent` argument: a selection is an exact short list, and an
/// unbounded array is refused at the input boundary rather than after
/// resolution work has already started.
pub const MAX_OVERRIDE_SKILLS: usize = 64;

/// The maximum number of Tool selectors one invocation override may express.
pub const MAX_OVERRIDE_TOOLS: usize = 128;

/// One invocation-scoped replacement of selected child capability
/// dimensions.
///
/// The same value is produced by model-generated `subagent` arguments and by
/// trusted static Workflow program data. The two differ only in the
/// [`SubagentOverrideAuthority`] their launch site supplies, never in this
/// vocabulary or in the resolution algorithm that consumes it.
///
/// [`SubagentOverrideAuthority`]: super::resolver::SubagentOverrideAuthority
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
#[derive(schemars::JsonSchema)]
pub struct SubagentInvocationOverride {
    /// The complete replacement Tool selection, when this dimension is
    /// overridden. Absent uses the definition's selection.
    #[serde(
        default,
        deserialize_with = "crate::extensions::present_and_not_null",
        skip_serializing_if = "Option::is_none"
    )]
    // Absent or a complete value; never `null`. See the module contract.
    #[schemars(with = "ToolSelectionDocument")]
    pub tools: Option<ToolSelectionDocument>,
    /// The complete replacement Skill selection, when this dimension is
    /// overridden. Absent uses the definition's selection.
    #[serde(
        default,
        deserialize_with = "crate::extensions::present_and_not_null",
        skip_serializing_if = "Option::is_none"
    )]
    // Absent or a complete value; never `null`. See the module contract.
    #[schemars(with = "Vec<String>")]
    pub skills: Option<Vec<String>>,
    /// The complete replacement native Agent Extension composition, when this
    /// dimension is overridden. Absent uses the definition's composition.
    #[serde(
        default,
        deserialize_with = "crate::extensions::present_and_not_null",
        skip_serializing_if = "Option::is_none"
    )]
    // Absent or a complete value; never `null`. See the module contract.
    #[schemars(with = "NativeAgentExtensionSelection")]
    pub extensions: Option<NativeAgentExtensionSelection>,
}

impl SubagentInvocationOverride {
    /// Whether this override replaces no dimension at all.
    ///
    /// An empty override object is a legitimate, explicitly authored request
    /// that is *exactly equivalent* to omitting the object: every dimension
    /// is missing, so every dimension comes from the definition.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.tools.is_none() && self.skills.is_none() && self.extensions.is_none()
    }

    /// The effective Tool selectors of this invocation, canonically ordered
    /// and deduplicated exactly as a definition's own selection is.
    ///
    /// Sharing the definition's normalization is what makes "no override" and
    /// "an override restating the defaults" produce one identical effective
    /// profile rather than two profiles that merely look alike.
    #[must_use]
    pub fn effective_tools(&self, definition: &SubagentDefinition) -> Vec<ToolSelector> {
        match &self.tools {
            None => definition.tools().to_vec(),
            Some(selection) => canonical_selectors(selection.selectors()),
        }
    }

    /// The effective Skill selection of this invocation, canonically ordered
    /// and deduplicated exactly as a definition's own selection is.
    #[must_use]
    pub fn effective_skills(&self, definition: &SubagentDefinition) -> Vec<String> {
        match &self.skills {
            None => definition.skills().to_vec(),
            Some(selection) => canonical_skills(selection.clone()),
        }
    }

    /// The effective frozen native Agent Extension composition of this
    /// invocation.
    ///
    /// A present dimension is resolved through
    /// [`NativeAgentExtensionSelection::resolve`], whose absent members
    /// compose nothing. The authored role/launch defaults deliberately do not
    /// participate: `"extensions": {}` is the empty composition, never the
    /// document default that composes Agent Status.
    #[must_use]
    pub fn effective_extensions(&self, definition: &SubagentDefinition) -> NativeAgentExtensions {
        match &self.extensions {
            None => definition.extensions().clone(),
            Some(selection) => selection.resolve(),
        }
    }

    /// Rejects a structurally invalid override before any authority or
    /// availability question is asked.
    ///
    /// These are the definition-boundary rules of
    /// [`SubagentDefinition::new`], applied to the same vocabulary at the
    /// invocation boundary: nested delegation and child-unsafe lifecycle
    /// owners stay structurally unreachable, Skill selectors stay nonempty,
    /// and both selections stay bounded. Duplicates are normalized rather
    /// than rejected, exactly as a definition normalizes them.
    ///
    /// # Errors
    ///
    /// Returns the canonical authoring diagnostic of the first violation,
    /// prefixed by the offending dimension so an authored path can be built
    /// from it without parsing prose.
    pub fn validate_spelling(&self) -> Result<(), SubagentOverrideError> {
        if let Some(selection) = &self.tools {
            selection
                .validate_spelling()
                .map_err(|detail| SubagentOverrideError::InvalidTools { detail })?;
            let selectors = selection.selectors();
            if selectors.len() > MAX_OVERRIDE_TOOLS {
                return Err(SubagentOverrideError::TooManyTools {
                    count: selectors.len(),
                });
            }
            for selector in &selectors {
                if let ToolSelector::Builtin { name } = selector {
                    if name == crate::tools::native::SUBAGENT_TOOL_NAME {
                        return Err(SubagentOverrideError::RecursiveSelector {
                            selector: selector.canonical(),
                        });
                    }
                    if CHILD_UNSAFE_BUILTIN_TOOLS.contains(&name.as_str()) {
                        return Err(SubagentOverrideError::ChildUnsafeSelector {
                            selector: selector.canonical(),
                        });
                    }
                }
            }
        }
        if let Some(skills) = &self.skills {
            if skills.len() > MAX_OVERRIDE_SKILLS {
                return Err(SubagentOverrideError::TooManySkills {
                    count: skills.len(),
                });
            }
            if skills.iter().any(|skill| skill.trim().is_empty()) {
                return Err(SubagentOverrideError::EmptySkillSelector);
            }
        }
        Ok(())
    }

    /// The authored field path of one dimension, for nested diagnostics.
    #[must_use]
    pub const fn dimension_path(error: &SubagentOverrideError) -> &'static str {
        match error {
            SubagentOverrideError::InvalidTools { .. }
            | SubagentOverrideError::TooManyTools { .. }
            | SubagentOverrideError::RecursiveSelector { .. }
            | SubagentOverrideError::ChildUnsafeSelector { .. } => "override.tools",
            SubagentOverrideError::TooManySkills { .. }
            | SubagentOverrideError::EmptySkillSelector => "override.skills",
        }
    }
}

impl SubagentInvocationOverride {
    /// The canonically ordered, deduplicated selectors of one authored
    /// selection document, for bounded authoring projections.
    #[must_use]
    pub fn canonical_selectors_of(selection: &ToolSelectionDocument) -> Vec<ToolSelector> {
        canonical_selectors(selection.selectors())
    }
}

/// Canonically orders and deduplicates a Tool selection.
fn canonical_selectors(mut selectors: Vec<ToolSelector>) -> Vec<ToolSelector> {
    selectors.sort();
    selectors.dedup();
    selectors
}

/// Canonically orders and deduplicates a Skill selection.
fn canonical_skills(mut skills: Vec<String>) -> Vec<String> {
    skills.sort();
    skills.dedup();
    skills
}

/// A structural invocation-override violation, decided before any authority,
/// catalog, or availability question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubagentOverrideError {
    /// A Tool selector is not a well-formed source-qualified identity.
    InvalidTools {
        /// The canonical spelling diagnostic.
        detail: String,
    },
    /// The override expresses more Tool selectors than the bound admits.
    TooManyTools {
        /// The offending count.
        count: usize,
    },
    /// The override selects the `subagent` intrinsic. Nested delegation is
    /// unsupported and is rejected structurally, exactly as it is for a
    /// named definition.
    RecursiveSelector {
        /// The offending selector.
        selector: String,
    },
    /// The override selects a capability whose lifecycle owner cannot exist
    /// in a headless one-shot child.
    ChildUnsafeSelector {
        /// The offending selector.
        selector: String,
    },
    /// The override selects more Skills than the bound admits.
    TooManySkills {
        /// The offending count.
        count: usize,
    },
    /// A Skill selector is empty.
    EmptySkillSelector,
}

impl core::fmt::Display for SubagentOverrideError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidTools { detail } => formatter.write_str(detail),
            Self::TooManyTools { count } => write!(
                formatter,
                "the invocation override selects {count} capabilities; at most \
                 {MAX_OVERRIDE_TOOLS} are admitted"
            ),
            Self::RecursiveSelector { selector } => write!(
                formatter,
                "the invocation override selects {selector}: nested subagent delegation is \
                 unsupported"
            ),
            Self::ChildUnsafeSelector { selector } => write!(
                formatter,
                "the invocation override selects {selector}, whose lifecycle owner does not \
                 exist in a headless child runtime"
            ),
            Self::TooManySkills { count } => write!(
                formatter,
                "the invocation override selects {count} Skills; at most \
                 {MAX_OVERRIDE_SKILLS} are admitted"
            ),
            Self::EmptySkillSelector => {
                formatter.write_str("the invocation override names an empty Skill")
            }
        }
    }
}

impl std::error::Error for SubagentOverrideError {}

#[cfg(test)]
mod tests {
    use super::{SubagentInvocationOverride, SubagentOverrideError};
    use crate::extensions::{NativeAgentExtensions, NativeAgentExtensionsDocument};

    fn parse(value: serde_json::Value) -> Result<SubagentInvocationOverride, String> {
        serde_json::from_value(value).map_err(|error| error.to_string())
    }

    fn definition(
        tools: Vec<crate::capabilities::selection::ToolSelector>,
        skills: Vec<String>,
        extensions: NativeAgentExtensions,
    ) -> crate::runtime::subagent::SubagentDefinition {
        crate::runtime::subagent::SubagentDefinition::new(
            crate::runtime::subagent::SubagentName::parse("reviewer").expect("name"),
            "description".to_owned(),
            "instructions".to_owned(),
            std::path::PathBuf::from("/w/reviewer.md"),
            None,
            None,
            tools,
            skills,
            crate::runtime::subagent::SubagentProjectInstructionPolicy {
                inherit: true,
                files: Vec::new(),
            },
            crate::runtime::workspace::WorkspacePolicy::SharedWorkspace,
            extensions,
        )
        .expect("definition")
    }

    fn role() -> crate::runtime::subagent::SubagentDefinition {
        definition(
            vec![crate::capabilities::selection::ToolSelector::Builtin {
                name: "read".to_owned(),
            }],
            vec!["code-review".to_owned()],
            NativeAgentExtensionsDocument::default().resolve(),
        )
    }

    /// The whole contract of the missing/present distinction, on one role.
    #[test]
    fn sub258_a_missing_dimension_inherits_and_a_present_dimension_replaces() {
        let role = role();
        let absent = SubagentInvocationOverride::default();
        assert!(absent.is_empty());
        assert_eq!(absent.effective_tools(&role), role.tools());
        assert_eq!(absent.effective_skills(&role), role.skills());
        assert_eq!(&absent.effective_extensions(&role), role.extensions());

        let empty_object = parse(serde_json::json!({})).expect("an empty override parses");
        assert_eq!(empty_object, absent, "an empty object overrides nothing");

        let tools_only =
            parse(serde_json::json!({"tools": {"builtin": ["grep"]}})).expect("parses");
        assert_eq!(
            tools_only
                .effective_tools(&role)
                .iter()
                .map(crate::capabilities::selection::ToolSelector::canonical)
                .collect::<Vec<_>>(),
            vec!["builtin:grep"],
            "a present tools dimension replaces rather than unions"
        );
        assert_eq!(
            tools_only.effective_skills(&role),
            role.skills(),
            "the missing dimensions still resolve from the definition"
        );
        assert_eq!(&tools_only.effective_extensions(&role), role.extensions());
    }

    /// The extension-default trap: the authored document composes Agent
    /// Status by default, and an explicit empty override must not.
    #[test]
    fn sub258_an_empty_extension_override_composes_nothing() {
        let role = role();
        assert!(
            role.extensions().agent_status().is_some(),
            "the role default really does compose Agent Status"
        );
        let cleared = parse(serde_json::json!({"extensions": {}})).expect("parses");
        assert_eq!(
            cleared.effective_extensions(&role),
            NativeAgentExtensions::none(),
            "an explicit empty extension selection composes no extension at all"
        );
        assert_eq!(
            NativeAgentExtensionsDocument::default().resolve(),
            *role.extensions(),
            "the authored document default is unchanged by the override vocabulary"
        );
    }

    #[test]
    fn sub258_explicit_empty_tool_and_skill_selections_are_empty() {
        let role = role();
        let cleared = parse(serde_json::json!({"tools": {}, "skills": []})).expect("parses");
        assert!(cleared.effective_tools(&role).is_empty());
        assert!(cleared.effective_skills(&role).is_empty());
    }

    /// An explicit `null` is a different spelling from an absent key and must
    /// never be folded into "missing".
    #[test]
    fn sub258_explicit_null_dimensions_are_rejected() {
        for value in [
            serde_json::json!({"tools": null}),
            serde_json::json!({"skills": null}),
            serde_json::json!({"extensions": null}),
            serde_json::json!({"extensions": {"agentStatus": null}}),
        ] {
            assert!(parse(value.clone()).is_err(), "accepted {value}");
        }
    }

    #[test]
    fn sub258_unknown_fields_and_non_goal_dimensions_are_rejected() {
        for value in [
            serde_json::json!({"model": "local/model-a"}),
            serde_json::json!({"instructions": "do it differently"}),
            serde_json::json!({"timeoutMs": 1000}),
            serde_json::json!({"worktree": {"enabled": true}}),
            serde_json::json!({"addTools": {"builtin": ["bash"]}}),
            serde_json::json!({"tools": {"builtin": ["read"], "future": []}}),
            serde_json::json!({"extensions": {"futureGoal": {"enabled": true}}}),
            serde_json::json!({"extensions": {"todo": {"future": true}}}),
            serde_json::json!({"extensions": {"todo": null}}),
            serde_json::json!({"extensions": {"agentStatus": {"future": true}}}),
            serde_json::json!({"tools": ["read"]}),
            serde_json::json!({"skills": "code-review"}),
        ] {
            assert!(parse(value.clone()).is_err(), "accepted {value}");
        }
    }

    #[test]
    fn sub258_structural_child_rules_survive_at_the_invocation_boundary() {
        let recursive = parse(serde_json::json!({"tools": {"builtin": ["subagent"]}}))
            .expect("parses")
            .validate_spelling();
        assert!(matches!(
            recursive,
            Err(SubagentOverrideError::RecursiveSelector { .. })
        ));
        let unsafe_owner = parse(serde_json::json!({"tools": {"builtin": ["execution"]}}))
            .expect("parses")
            .validate_spelling();
        assert!(matches!(
            unsafe_owner,
            Err(SubagentOverrideError::ChildUnsafeSelector { .. })
        ));
        let empty_skill = parse(serde_json::json!({"skills": ["  "]}))
            .expect("parses")
            .validate_spelling();
        assert_eq!(empty_skill, Err(SubagentOverrideError::EmptySkillSelector));
        let blank_selector = parse(serde_json::json!({"tools": {"builtin": [""]}}))
            .expect("parses")
            .validate_spelling();
        assert!(matches!(
            blank_selector,
            Err(SubagentOverrideError::InvalidTools { .. })
        ));
    }

    /// Order and repetition are normalized exactly as a definition
    /// normalizes them, so an override restating the defaults in another
    /// order is the same effective selection.
    #[test]
    fn sub258_override_selections_are_canonically_normalized() {
        let role = role();
        let shuffled = parse(serde_json::json!({
            "tools": {"builtin": ["read", "grep", "read"]},
            "skills": ["b", "a", "b"]
        }))
        .expect("parses");
        assert_eq!(
            shuffled
                .effective_tools(&role)
                .iter()
                .map(crate::capabilities::selection::ToolSelector::canonical)
                .collect::<Vec<_>>(),
            vec!["builtin:grep", "builtin:read"]
        );
        assert_eq!(
            shuffled.effective_skills(&role),
            vec!["a".to_owned(), "b".to_owned()]
        );
    }

    /// A restatement of the definition's own dimensions resolves to exactly
    /// the definition's own values.
    #[test]
    fn sub258_an_override_restating_the_defaults_is_equivalent_to_none() {
        let role = role();
        let restated = SubagentInvocationOverride {
            tools: Some(crate::capabilities::selection::ToolSelectionDocument {
                builtin: vec!["read".to_owned()],
                sources: std::collections::BTreeMap::new(),
            }),
            skills: Some(vec!["code-review".to_owned()]),
            extensions: Some(crate::extensions::NativeAgentExtensionSelection::of(
                role.extensions(),
            )),
        };
        assert!(!restated.is_empty());
        assert_eq!(restated.effective_tools(&role), role.tools());
        assert_eq!(restated.effective_skills(&role), role.skills());
        assert_eq!(&restated.effective_extensions(&role), role.extensions());
    }
}
