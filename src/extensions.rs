//! Closed Rust-owned Agent Plugins. User-facing configuration uses `plugins`.
//!
//! Root and named profiles each opt into Agent Status, Todo, and Goal. Omitted
//! Plugins are off. Workspace replaces each Plugin identity as a whole object.
//! Plugins compose through the existing Context Assembly, Tool Plane, and
//! Conversation owners; there is no dynamic loader or third-party lifecycle API.
//!
//! A configuration generation freezes composition. Native preparation stages and publishes
//! the complete next generation at a safe boundary. Admitted work keeps its old
//! composition. Todo lists and Goal state remain Conversation domain state.
//! Named Agents own independent composition; Root authorizes their names, not
//! their Tool or Plugin dimensions. Child scope constraints remain native.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::context::{
    AgentStatusClock, AgentStatusConfig, AgentStatusEngine, BackgroundStatusConfig,
    TimeStatusConfig,
};

/// The closed authored composition surface of native Agent Extensions.
///
/// Every member is a concrete named extension in a closed record.
/// Unknown extension names are rejected. Runtime TOML layers use the separate
/// `snake_case` authoring boundary; resource and wire documents retain their
/// own serialization contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields, default)]
#[derive(schemars::JsonSchema)]
pub struct NativeAgentExtensionsDocument {
    /// The Agent Status extension: optional provider-independent runtime
    /// context for an already-established model step.
    pub agent_status: AgentStatusExtensionDocument,
    /// The Todo extension: the conversation-owned task list, its
    /// model-facing `todo` Tool, and its bounded status presentation
    /// (Issue #259).
    pub todo: TodoExtensionDocument,
    /// Root-only persistent Goal pursuit, disabled by default.
    pub goal: GoalExtensionDocument,
}

impl Default for NativeAgentExtensionsDocument {
    fn default() -> Self {
        Self {
            agent_status: AgentStatusExtensionDocument {
                enabled: false,
                ..AgentStatusExtensionDocument::default()
            },
            todo: TodoExtensionDocument { enabled: false },
            goal: GoalExtensionDocument::default(),
        }
    }
}

/// Authored opt-in Goal composition.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct GoalExtensionDocument {
    pub enabled: bool,
}

/// Frozen Goal composition; domain bounds are native, not launch settings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct GoalExtensionConfig {}

/// The authored Agent Status extension.
///
/// `enabled` composes the extension in or out of the runtime as a whole;
/// `time` and `background` remain the two bounded status contributors, with
/// exactly the semantics they had before the migration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
#[derive(schemars::JsonSchema, Default)]
pub struct AgentStatusExtensionDocument {
    /// Whether this composition includes the Agent Status extension at all.
    ///
    /// With `false` the runtime composes no status engine: the Agent Loop
    /// emits no Agent Status and is otherwise a completely ordinary
    /// `ConversationRuntime`.
    pub enabled: bool,
    /// The Time contributor configuration.
    pub time: TimeStatusConfig,
    /// The Background contributor configuration.
    pub background: BackgroundStatusConfig,
}

/// The authored Todo extension (Issue #259).
///
/// Todo is *one* capability with several faces, and `enabled` composes all
/// of them together or none of them:
///
/// ```text
/// enabled = true    conversation-owned ConversationTodoList authority
///                   the model-facing `todo` Tool
///                   the bounded read-only Todo status presentation
///                   the Runtime Client / TUI Todo projection
///
/// enabled = false   none of the above for this runtime; canonical history
///                   keeps every Todo ToolCall/ToolResult it already holds
/// ```
///
/// It carries no contributor settings today. That is a statement about the
/// extension, not a placeholder: the list's bounds, transitions, and
/// dependency rules are owned by
/// [`ConversationTodoList`](crate::tools::todo::ConversationTodoList) and are
/// not launch configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
#[derive(schemars::JsonSchema, Default)]
pub struct TodoExtensionDocument {
    /// Whether this composition includes the Todo extension at all.
    pub enabled: bool,
}

/// The **frozen** Todo extension configuration of one composition.
///
/// Present means "this Agent/Conversation composes Todo". The type carries no
/// field because nothing about the list is launch-configurable; it exists so
/// the closed composition holds one typed member per extension rather than a
/// bare `bool`, exactly like [`AgentStatusConfig`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct TodoExtensionConfig {}

impl NativeAgentExtensionsDocument {
    /// Freezes this authored document into the composition one launch runs
    /// against.
    ///
    /// This is the **only** transition from mutable configuration to
    /// executed composition. Root composition calls it once, at
    /// `LocalConversationCore::compose`; named-role loading calls it once,
    /// while building the immutable `NamedAgentDefinition`.
    #[must_use]
    pub fn resolve(&self) -> NativeAgentExtensions {
        NativeAgentExtensions {
            agent_status: self.agent_status.enabled.then(|| AgentStatusConfig {
                time: self.agent_status.time.clone(),
                background: self.agent_status.background.clone(),
            }),
            todo: self.todo.enabled.then(TodoExtensionConfig::default),
            goal: self.goal.enabled.then(GoalExtensionConfig::default),
        }
    }
}

/// The **presence-aware** closed selection an invocation override expresses.
///
/// Complete profiles and invocation selections share omission semantics:
/// omitted members compose no extension. The outer invocation dimension is
/// presence-aware: absent retains the named default; present replaces it.
/// This wire record preserves the invocation protocol's camelCase spelling
/// and rejects explicit null members.
///
/// The record stays closed exactly like the authored document: an unknown
/// extension name is rejected by `deny_unknown_fields`, so a misspelled or
/// not-yet-implemented extension fails deterministically instead of being
/// silently ignored.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
#[derive(schemars::JsonSchema)]
pub struct NativeAgentExtensionSelection {
    /// The requested Agent Status extension, when this selection composes it.
    ///
    /// `null` is not accepted: the field is either absent (not composed) or a
    /// complete authored configuration. Collapsing an explicit `null` into
    /// "absent" would give one wire spelling two meanings.
    #[serde(
        default,
        deserialize_with = "crate::extensions::present_and_not_null",
        skip_serializing_if = "Option::is_none"
    )]
    // The published schema must say what the runtime accepts: an absent key
    // or a complete document, never `null`. The default `Option` rendering
    // would advertise a spelling the deserializer refuses.
    #[schemars(with = "AgentStatusExtensionDocument")]
    pub agent_status: Option<AgentStatusExtensionDocument>,
    /// The requested Todo extension, when this selection composes it
    /// (Issue #259).
    ///
    /// Absent means the child composes no Todo at all — no list authority, no
    /// `todo` Tool, no status contribution — never "whatever the role
    /// authored". `null` is refused for the same reason `agentStatus` refuses
    /// it: one wire spelling may not carry two meanings.
    #[serde(
        default,
        deserialize_with = "crate::extensions::present_and_not_null",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "TodoExtensionDocument")]
    pub todo: Option<TodoExtensionDocument>,
    #[serde(
        default,
        deserialize_with = "crate::extensions::present_and_not_null",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "GoalExtensionDocument")]
    pub goal: Option<GoalExtensionDocument>,
}

/// Deserializes a field that may be **absent**, but never explicitly `null`.
///
/// serde reaches this function only when the key is present, so delegating to
/// the inner type turns `"agentStatus": null` into that type's ordinary
/// "invalid type: null" rejection while an omitted key still takes the
/// container's `default`.
///
/// # Errors
///
/// Returns the inner type's deserialization error, including for `null`.
pub(crate) fn present_and_not_null<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

impl NativeAgentExtensionSelection {
    /// Freezes this requested selection into the composition a child runs
    /// against.
    ///
    /// An absent member composes nothing. A present member composes exactly
    /// what it authored, including `enabled: false`, which is "named and
    /// switched off" and therefore still composes nothing — the same rule
    /// [`NativeAgentExtensionsDocument::resolve`] applies.
    #[must_use]
    pub fn resolve(&self) -> NativeAgentExtensions {
        NativeAgentExtensions {
            agent_status: self
                .agent_status
                .as_ref()
                .filter(|status| status.enabled)
                .map(|status| AgentStatusConfig {
                    time: status.time.clone(),
                    background: status.background.clone(),
                }),
            todo: self
                .todo
                .filter(|todo| todo.enabled)
                .map(|_| TodoExtensionConfig::default()),
            goal: self
                .goal
                .filter(|goal| goal.enabled)
                .map(|_| GoalExtensionConfig {}),
        }
    }

    /// The selection that reproduces an already-frozen composition exactly.
    ///
    /// This is what makes "no override" and "an explicit override restating
    /// the defaults" the same effective profile rather than two shapes that
    /// merely look alike.
    #[must_use]
    pub fn of(frozen: &NativeAgentExtensions) -> Self {
        Self {
            agent_status: frozen
                .agent_status()
                .map(|config| AgentStatusExtensionDocument {
                    enabled: true,
                    time: config.time.clone(),
                    background: config.background.clone(),
                }),
            todo: frozen
                .todo()
                .map(|_| TodoExtensionDocument { enabled: true }),
            goal: frozen
                .goal()
                .map(|_| GoalExtensionDocument { enabled: true }),
        }
    }
}

/// The one-shot child scope support of the closed extension vocabulary.
///
/// Extension **authorization** and child-**scope support** are independent
/// checks: an extension a caller is fully entitled to compose may still be
/// meaningless — or actively wrong — inside a one-shot child, and must then
/// fail deterministically before the child is staged rather than be silently
/// dropped.
///
/// The match below is exhaustive over the closed composition on purpose: it
/// is the seam a future extension author has to visit, and it is a compile
/// error to add a member without deciding this question. Both members of
/// today's vocabulary are supported: a child is an ordinary
/// `ConversationRuntime` whose Agent Loop composes the same status engine and
/// the same conversation-owned task list a root does.
#[must_use]
pub fn unsupported_child_scope(
    composition: &NativeAgentExtensions,
) -> Option<UnsupportedChildScope> {
    // The exhaustive match is the guarantee: adding a member to the closed
    // composition without deciding its child scope does not compile.
    match composition {
        NativeAgentExtensions {
            goal: Some(GoalExtensionConfig {}),
            agent_status: _,
            todo: _,
        } => Some(UnsupportedChildScope {
            extension: "goal",
            reason: "Goal requires a root conversation; one-shot children cannot continue across turns",
        }),
        // Agent Status contributes one bounded structured fact that Context
        // Assembly admits at request time. A one-shot child owns Context
        // Assembly exactly like a root Agent and needs no multi-round or
        // resumable lifecycle for it, so it is supported in child scope.
        //
        // Todo is conversation-owned state published through ordinary
        // canonical ToolResults. A one-shot child owns its own conversation,
        // its own Ledger, and its own Tool Plane, so it composes its own list
        // — which is exactly why a child's list can never alias its parent's.
        // The child's bounded final report is unchanged: a list is working
        // state of the child conversation, never part of its result.
        NativeAgentExtensions {
            agent_status: None | Some(AgentStatusConfig { .. }),
            todo: None | Some(TodoExtensionConfig { .. }),
            goal: None,
        } => None,
    }
}

/// The canonical authored names of the extensions one composition composes.
///
/// The list is derived from the closed composition rather than written out at
/// each call site, so a diagnostic can never name an extension the composition
/// does not actually hold — and adding a member updates every caller at once.
#[must_use]
pub fn composed_extension_names(composition: &NativeAgentExtensions) -> Vec<&'static str> {
    let NativeAgentExtensions {
        agent_status,
        todo,
        goal,
    } = composition;
    let mut names = Vec::new();
    if agent_status.is_some() {
        names.push(AGENT_STATUS_EXTENSION);
    }
    if todo.is_some() {
        names.push(TODO_EXTENSION);
    }
    if goal.is_some() {
        names.push("goal");
    }
    names
}

/// The canonical model-facing names of the Tools one composition contributes
/// (Issue #259).
///
/// Derived from the composition's own Tool registrations rather than written
/// out separately, so a prospective diagnostic can never advertise a Tool the
/// composition would not actually publish.
#[must_use]
pub fn composed_extension_tool_names(composition: &NativeAgentExtensions) -> Vec<String> {
    composition
        .prospective_tool_names()
        .into_iter()
        .map(str::to_owned)
        .collect()
}

/// One recognized extension that a one-shot child cannot own.
///
/// This is a scope fact, never an authority fact: the caller may have been
/// fully entitled to compose the extension, and the request still fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnsupportedChildScope {
    /// The canonical authored extension name.
    pub extension: &'static str,
    /// The bounded reason the one-shot child scope cannot own it.
    pub reason: &'static str,
}

/// The **frozen** native Agent Extension composition of one concrete
/// Agent/Conversation.
///
/// An absent member means the extension is not part of this composition —
/// not that it is present and idle. An empty composition is an ordinary
/// runtime with nothing added, never a second semantic runtime mode.
///
/// The type is serializable because a child's frozen composition crosses the
/// subagent process boundary inside `ResolvedSubagentSpec`. It is
/// deliberately not mutable after construction: there is no installer,
/// uninstaller, or runtime extension manager anywhere in the runtime.
///
/// There is deliberately no `Default`: "the composition an unconfigured
/// launch or role gets" is a decision of
/// [`NativeAgentExtensionsDocument::resolve`], and "no extension at all" is
/// [`NativeAgentExtensions::none`]. Conflating the two behind a derive is
/// exactly how an empty extension set would start meaning something else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeAgentExtensions {
    /// The frozen Agent Status extension configuration, when this
    /// composition includes the extension.
    #[serde(default)]
    agent_status: Option<AgentStatusConfig>,
    /// The frozen Todo extension configuration, when this composition
    /// includes the extension (Issue #259).
    #[serde(default)]
    todo: Option<TodoExtensionConfig>,
    #[serde(default)]
    goal: Option<GoalExtensionConfig>,
}

impl NativeAgentExtensions {
    pub(crate) fn without_goal(mut self) -> Self {
        self.goal = None;
        self
    }

    /// The composition with no native Agent Extension at all.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            agent_status: None,
            todo: None,
            goal: None,
        }
    }

    /// The composition containing exactly the Agent Status extension with
    /// the supplied contributor configuration.
    #[must_use]
    pub const fn with_agent_status(agent_status: AgentStatusConfig) -> Self {
        Self {
            agent_status: Some(agent_status),
            todo: None,
            goal: None,
        }
    }

    /// The composition containing exactly the Todo extension (Issue #259).
    #[must_use]
    pub const fn with_todo() -> Self {
        Self {
            agent_status: None,
            todo: Some(TodoExtensionConfig {}),
            goal: None,
        }
    }

    /// This composition with the Todo extension composed.
    #[must_use]
    pub const fn and_todo(mut self) -> Self {
        self.todo = Some(TodoExtensionConfig {});
        self
    }

    /// This composition with the Agent Status extension composed from
    /// `config`.
    ///
    /// The counterpart of [`Self::and_todo`], used where a caller already
    /// holds the contributor configuration a status engine will be — or has
    /// been — materialized from, and needs the frozen composition to say so.
    #[must_use]
    pub fn and_agent_status(mut self, config: AgentStatusConfig) -> Self {
        self.agent_status = Some(config);
        self
    }

    /// The frozen Agent Status configuration, when the extension is composed.
    #[must_use]
    pub const fn agent_status(&self) -> Option<&AgentStatusConfig> {
        self.agent_status.as_ref()
    }

    /// The frozen Todo configuration, when the extension is composed.
    ///
    /// `Some` is the *whole* Todo composition decision: the conversation
    /// composes a [`ConversationTodoList`](crate::tools::todo::ConversationTodoList),
    /// publishes the `todo` Tool, and offers its bounded status presentation.
    #[must_use]
    pub const fn todo(&self) -> Option<&TodoExtensionConfig> {
        self.todo.as_ref()
    }

    /// The root-only Goal extension configuration.
    #[must_use]
    pub const fn goal(&self) -> Option<&GoalExtensionConfig> {
        self.goal.as_ref()
    }

    /// Compose Goal explicitly.
    #[must_use]
    pub const fn and_goal(mut self) -> Self {
        self.goal = Some(GoalExtensionConfig {});
        self
    }

    /// Whether this composition contains no native Agent Extension.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.agent_status.is_none() && self.todo.is_none() && self.goal.is_none()
    }

    /// Reads this composition back out of the runtime extension owners a
    /// materialization produced.
    ///
    /// The Agent Status engine a composition materializes carries the exact
    /// frozen contributor configuration it was built from, and whether that
    /// engine exists at all *is* the composed/absent fact; the conversation's
    /// [`ConversationTodoList`] exists for exactly as long as the Todo
    /// extension is composed, and its presence *is* that fact. Reading the
    /// owners back therefore yields the composition they were built from —
    /// **if** they were all built from the same one.
    ///
    /// That "if" is the whole point of this function since Issue #259. It is
    /// no longer how anything *reads* the composition — the conversation tool
    /// runtime stores the one frozen value, and
    /// [`ConversationRuntime::native_extensions`](crate::runtime::ConversationRuntime::native_extensions)
    /// returns it directly. It is how the `ConversationRuntime`
    /// ownership-transfer boundary **proves** that every materialized facet
    /// followed from that single stored decision, so a projection can never
    /// describe a composition the runtime is not actually running.
    ///
    /// [`ConversationTodoList`]: crate::tools::todo::ConversationTodoList
    #[must_use]
    pub(crate) fn from_materialized(
        status_engine: Option<&AgentStatusEngine>,
        todos: Option<&crate::tools::todo::ConversationTodoList>,
        goal: Option<&crate::goal::GoalDomain>,
    ) -> Self {
        Self {
            agent_status: status_engine.map(|engine| engine.config().clone()),
            todo: todos.map(|_| TodoExtensionConfig {}),
            goal: goal.map(|_| GoalExtensionConfig {}),
        }
    }

    /// The model-facing names of the extension-provided Tools this frozen
    /// composition **would** publish (Issue #259).
    ///
    /// This is the *prospective* answer — what a launch with this
    /// composition will offer — and its only callers are the launch
    /// diagnostics that describe a composition before it is materialized,
    /// plus this module's own tests. It deliberately does **not** compose the
    /// running Tool Plane: the Tool Plane a live runtime serves is
    /// [`ExtensionToolPlane`], which only
    /// [`ConversationToolRuntime::extension_tool_plane`] can construct, and
    /// which is derived from the extension owners that runtime actually
    /// materialized.
    ///
    /// ```text
    /// ordinary selected Tool capabilities      agent.tools / named tools
    /// + explicitly enabled Plugin Tools        agent.plugins / named plugins
    /// + already-admitted domain protocols      Workflow output, ...
    /// ```
    ///
    /// An empty ordinary Tool selection does not disable an explicitly enabled
    /// Plugin. Naming a Plugin Tool in the ordinary whitelist cannot enable it.
    /// Root and named-Agent Plugins default off.
    ///
    /// The answer is deliberately a list of **names**, not registrations: a
    /// prospective description must not be able to put an executable Tool
    /// anywhere. Nothing in the process can turn a composition into a
    /// registered Tool except a materialized [`ExtensionToolPlane`].
    ///
    /// The returned set is a function of the frozen composition alone, so it
    /// is stable for the composition's whole lifetime: Todo list contents,
    /// emptiness, and mutations cannot add or remove a Tool, and a resource
    /// reload — which never reaches a frozen composition — cannot install or
    /// uninstall one.
    ///
    /// [`ConversationToolRuntime::extension_tool_plane`]: crate::tools::runtime::ConversationToolRuntime::extension_tool_plane
    #[must_use]
    pub(crate) fn prospective_tool_names(&self) -> Vec<&'static str> {
        let NativeAgentExtensions {
            // Agent Status contributes context, never a Tool.
            agent_status: _,
            todo,
            goal,
        } = self;
        let mut names = Vec::new();
        if todo.is_some() {
            names.push(crate::tools::native::TODO_TOOL_NAME);
        }
        if goal.is_some() {
            names.extend(crate::tools::native::GOAL_TOOL_NAMES);
        }
        names
    }

    /// The [`ExtensionToolPlane`] shape a correctly materialized runtime of
    /// this composition must have (Issue #259).
    ///
    /// The `ConversationRuntime` ownership-transfer boundary compares the
    /// coordinator's actual plane against this value, which is how a
    /// coordinator composed from a *different* conversation's materialization
    /// is refused instead of quietly serving a Tool surface the attached
    /// conversation owns no state for.
    #[must_use]
    pub(crate) fn expected_tool_plane(&self) -> ExtensionToolPlaneShape {
        ExtensionToolPlaneShape {
            todo: self.todo.is_some(),
            goal: self.goal.is_some(),
            invalid_goal: false,
        }
    }

    /// Materializes the attempt-owned Agent Status engine of this frozen
    /// composition.
    ///
    /// This is the one materialization seam between the extension set and
    /// the runtime: `None` composes a runtime with no status engine, and no
    /// other Agent Loop, Tool Plane, cancellation, or durability path
    /// consults the extension set at all.
    #[must_use]
    pub fn agent_status_engine(
        &self,
        clock: Arc<dyn AgentStatusClock>,
    ) -> Option<AgentStatusEngine> {
        self.agent_status
            .clone()
            .map(|config| AgentStatusEngine::new(config, clock))
    }

    /// The deterministic canonical framing of this composition **as
    /// authored**, for the named-role source-definition digest.
    ///
    /// Length-prefixed like every other digest field, and explicit about
    /// absence: a role that omits Agent Status is a different definition
    /// from one that disables its two contributors.
    ///
    /// This framing keeps the authored `timezone` spelling — an omitted zone
    /// and an explicitly authored `UTC` are two different source documents —
    /// because the definition digest identifies the *source definition*, not
    /// the behavior it produces. [`Self::effective_digest_framing`] is the
    /// framing for the latter; the two are deliberately different questions.
    #[must_use]
    pub fn authored_digest_framing(&self) -> String {
        let agent_status = match &self.agent_status {
            None => "agent_status=\u{0}absent".to_owned(),
            Some(config) => format!(
                "agent_status=present:time={}:timezone={}:background={}",
                config.time.enabled,
                config
                    .time
                    .timezone
                    .map_or_else(|| "\u{0}none".to_owned(), |zone| zone.name().to_owned()),
                config.background.enabled,
            ),
        };
        format!(
            "{agent_status}|{}|goal={}",
            self.todo_digest_framing(),
            self.goal.is_some()
        )
    }

    /// The framing of the Todo member, shared by both digests.
    ///
    /// Todo has no contributor configuration, so "composed" and "absent" are
    /// the whole vocabulary and the authored and effective framings coincide.
    /// It is still framed explicitly rather than omitted: a role that
    /// composes Todo is a different definition — and a different execution
    /// profile — from one that does not.
    fn todo_digest_framing(&self) -> &'static str {
        match &self.todo {
            None => "todo=\u{0}absent",
            Some(TodoExtensionConfig {}) => "todo=present",
        }
    }

    /// The deterministic canonical framing of what this composition
    /// **executes**, for the effective child execution-profile digest.
    ///
    /// It differs from [`Self::authored_digest_framing`] in exactly one
    /// dimension — the timezone — and in two ways, both of which follow from
    /// the same rule: *frame what the frozen composition does, not how it was
    /// spelled*.
    ///
    /// ```text
    /// time.enabled = true    the zone executes, so the EFFECTIVE zone is
    ///                        framed, through `effective_timezone()`. An
    ///                        omitted zone renders UTC, so `omitted` and an
    ///                        explicit `UTC` are one effective profile.
    ///
    /// time.enabled = false   the Time contributor never runs, so no zone
    ///                        executes and none may distinguish the profile.
    ///                        Every zone spelling — including omission —
    ///                        frames as one inactive sentinel.
    /// ```
    ///
    /// The disabled case is not an oversight-shaped shortcut: a child frozen
    /// with Time off emits no Time contribution whatever its `timezone` says,
    /// so two such compositions are behaviorally identical and must correlate
    /// to one `profile_digest`. That a later configuration edit could
    /// re-enable Time is irrelevant here — the composition this framing
    /// identifies is frozen, and re-enabling Time changes `time.enabled`,
    /// which is framed.
    ///
    /// The sentinel is a spelling no IANA zone name can carry, so it can
    /// never collide with an enabled zone's framing.
    ///
    /// [`Self::authored_digest_framing`] deliberately keeps distinguishing a
    /// disabled contributor's authored zone, because a definition digest
    /// identifies the **source document**, not the behavior it produces.
    #[must_use]
    pub fn effective_digest_framing(&self) -> String {
        let agent_status = match &self.agent_status {
            None => "agent_status=\u{0}absent".to_owned(),
            Some(config) => format!(
                "agent_status=present:time={}:timezone={}:background={}",
                config.time.enabled,
                if config.time.enabled {
                    config.time.effective_timezone().name().to_owned()
                } else {
                    INACTIVE_TIMEZONE_FRAMING.to_owned()
                },
                config.background.enabled,
            ),
        };
        format!(
            "{agent_status}|{}|goal={}",
            self.todo_digest_framing(),
            self.goal.is_some()
        )
    }
}

/// The **materialized** extension-provided Tool surfaces of one conversation
/// (Issue #259).
///
/// This is the value the capability plane composes on top of ordinary Tool
/// selection, and it is the whole reason the central #259 invariant is
/// structural rather than conventional:
///
/// ```text
/// NativeAgentExtensions                  one frozen composition
///   -> ConversationToolRuntime           materializes the extension OWNERS
///        ConversationTodoList            (Todo state authority)
///        -> ExtensionToolPlane           derived from those owners
///             -> CapabilityCoordinator   the model Tool surface
/// ```
///
/// [`ConversationToolRuntime::extension_tool_plane`] is the only constructor
/// in the process, and it reads the owners rather than a configuration
/// value. So a plane that publishes `todo` exists only where a
/// [`ConversationTodoList`] exists to serve it — "Tool offered, state absent"
/// is not a state this type can hold, and the deterministic
/// `tool_runtime.todos()` failure it would cause is unreachable. The reverse
/// pairing, a Todo-owning conversation whose coordinator was composed from
/// *another* conversation's plane, is refused at the `ConversationRuntime`
/// ownership-transfer boundary through [`Self::shape`].
///
/// It is intentionally not `Clone`-cheap, not mutable, and not extendable:
/// there is no `push`, no merge, and no registry. A composition's Tool
/// surface is decided once, when its owners are materialized.
///
/// [`ConversationTodoList`]: crate::tools::todo::ConversationTodoList
/// [`ConversationToolRuntime::extension_tool_plane`]: crate::tools::runtime::ConversationToolRuntime::extension_tool_plane
#[derive(Clone)]
pub struct ExtensionToolPlane {
    registrations: Vec<crate::tools::executor::ToolRegistration>,
    shape: ExtensionToolPlaneShape,
}

impl std::fmt::Debug for ExtensionToolPlane {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtensionToolPlane")
            .field("shape", &self.shape)
            .field(
                "tools",
                &self
                    .registrations
                    .iter()
                    .map(|registration| registration.definition.name.as_str())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl ExtensionToolPlane {
    /// The plane of a composition that materialized no Tool-providing
    /// extension at all.
    ///
    /// This constructor is public where
    /// [`Self::of_materialized_owners`] is not, and the asymmetry is the
    /// invariant: an **empty** plane is safe from anywhere, because the
    /// failure #259 is about — offering the model a Tool whose state owner
    /// does not exist — needs a *non-empty* plane. Only materializing the
    /// owners can produce one of those, so the dangerous direction stays
    /// unrepresentable while a coordinator composed with no conversation
    /// behind it (a capability-plane unit fixture, a standalone MCP
    /// preparation test) can still say what it means.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            registrations: Vec::new(),
            shape: ExtensionToolPlaneShape {
                todo: false,
                goal: false,
                invalid_goal: false,
            },
        }
    }

    /// Derives the plane from the extension owners a conversation tool
    /// runtime **materialized**.
    ///
    /// Crate-private, and called from exactly one place. Every argument is a
    /// live owner rather than a configuration flag, which is what makes the
    /// state/Tool agreement hold by construction: the `todo` registration is
    /// produced by the presence of the list, not by a value that claims a
    /// list should exist.
    #[must_use]
    pub(crate) fn of_materialized_owners(
        todos: Option<&crate::tools::todo::ConversationTodoList>,
        goal: Option<&crate::goal::GoalDomain>,
    ) -> Self {
        let mut registrations = Vec::new();
        if todos.is_some() {
            registrations.push(crate::tools::native::todo_tool_registration());
        }
        if goal.is_some() {
            registrations.extend(crate::tools::native::goal_tool_registrations());
        }
        Self {
            registrations,
            shape: ExtensionToolPlaneShape {
                todo: todos.is_some(),
                goal: goal.is_some(),
                invalid_goal: false,
            },
        }
    }

    /// The composition-identifying shape of this plane, for the
    /// `ConversationRuntime` construction invariant.
    #[must_use]
    pub(crate) const fn shape(&self) -> ExtensionToolPlaneShape {
        self.shape
    }

    /// The registrations this plane contributes to the model Tool set.
    #[must_use]
    pub(crate) fn registrations(&self) -> &[crate::tools::executor::ToolRegistration] {
        &self.registrations
    }

    /// The model-facing Tool names this plane publishes.
    #[must_use]
    pub fn tool_names(&self) -> Vec<String> {
        self.registrations
            .iter()
            .map(|registration| registration.definition.name.clone())
            .collect()
    }

    /// Registers this plane's Tools into `registry`.
    ///
    /// The counterpart of
    /// [`register_native_tools`](crate::tools::native::register_native_tools),
    /// and the separation is the contract: native registration composes the
    /// ordinary capability plane, this composes the extension plane, and no
    /// ordinary activation policy is applied to what it registers. Unlike the
    /// seam it replaces, it cannot be reached without first materializing the
    /// extension owners whose Tools it registers.
    ///
    /// Crate-private: the capability plane composes extension Tools through
    /// `select_tools`, and nothing outside this crate has a reason to inject
    /// them into a registry after profile selection. The plane retains the
    /// Conversation-owned executors backing every registration.
    ///
    /// # Errors
    ///
    /// Returns the specific [`ToolRegistryError`] of the first registration
    /// violation — in practice, an identity collision with a Tool the
    /// registry already holds.
    ///
    /// [`ToolRegistryError`]: crate::tools::executor::ToolRegistryError
    pub fn register_into(
        &self,
        registry: &mut crate::tools::executor::ToolRegistry,
    ) -> Result<(), crate::tools::executor::ToolRegistryError> {
        for registration in &self.registrations {
            let crate::tools::executor::ToolRegistration {
                definition,
                executor,
                normalizer,
                ..
            } = registration.clone();
            registry.register_with_execution_metadata(
                definition,
                executor,
                normalizer,
                crate::tools::deadline::ForegroundPolicy::Leaf,
            )?;
        }
        Ok(())
    }
}

/// The composition-identifying shape of one extension Tool surface.
///
/// One `bool` per Tool-providing extension. It exists so the
/// `ConversationRuntime` ownership-transfer boundary can compare Tool
/// surfaces against *what the conversation's frozen composition says they
/// must be*, without comparing executors.
///
/// The same closed shape describes two genuinely different facts, and #259's
/// invariant needs both:
///
/// ```text
/// ExtensionToolPlane::shape()             what a coordinator is CONFIGURED
///                                         to publish into a future prepared
///                                         candidate
///
/// Self::of_published_registry()           what the CURRENT ACTIVE capability
///                                         generation actually executes
/// ```
///
/// They are not interchangeable. A coordinator holds its configured plane
/// from construction but publishes nothing until a prepared candidate is
/// committed, so a revision-zero coordinator names `todo` in its configured
/// plane while its active registry is empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ExtensionToolPlaneShape {
    /// Whether the surface carries the `todo` Tool.
    pub(crate) todo: bool,
    pub(crate) goal: bool,
    /// Partial or altered Goal commands are neither an absent nor a complete surface.
    pub(crate) invalid_goal: bool,
}

impl ExtensionToolPlaneShape {
    /// The extension Tool authority one **currently published** capability
    /// generation actually carries (Issue #259).
    ///
    /// This reads the active model-facing registry — the executable authority
    /// an attempt really runs against — rather than the plane a coordinator
    /// was configured with. "Configured to publish Todo" and "Todo has been
    /// published into the currently executable generation" are different
    /// facts, and only the second one lets the model call `todo`.
    ///
    /// Identity is the **exact canonical [`ToolDefinition`]** the extension
    /// owns, never the model-facing name: a same-named MCP Tool, or a `todo`
    /// whose schema, origin, or execution policies differ, is not the Todo
    /// extension's authority and must not satisfy the invariant.
    ///
    /// The match is exhaustive over the Tool-providing half of the closed
    /// composition, built from the very registrations
    /// [`ExtensionToolPlane::of_materialized_owners`] publishes — so there is
    /// no second, hand-maintained list of extension Tool identities anywhere,
    /// and `ConversationRuntime` never learns a Tool id of its own.
    ///
    /// [`ToolDefinition`]: crate::tools::types::ToolDefinition
    #[must_use]
    pub(crate) fn of_published_registry(registry: &crate::tools::executor::ToolRegistry) -> Self {
        let published = registry.definitions();
        let carries = |canonical: &crate::tools::types::ToolDefinition| {
            published.iter().any(|definition| definition == canonical)
        };
        let goal_tools = crate::tools::native::goal_tool_registrations();
        let goal = goal_tools.iter().all(|tool| carries(&tool.definition));
        let claims_goal = |definition: &crate::tools::types::ToolDefinition| {
            definition.origin == crate::tools::types::ToolOrigin::Builtin
                && goal_tools.iter().any(|tool| {
                    tool.definition.id == definition.id || tool.definition.name == definition.name
                })
        };
        let invalid_goal = published.iter().any(|definition| {
            claims_goal(definition)
                && (!goal || !goal_tools.iter().any(|tool| tool.definition == *definition))
        });
        Self {
            todo: carries(&crate::tools::native::todo_tool_registration().definition),
            goal,
            invalid_goal,
        }
    }
}

/// The framing token a **disabled** Time contributor's timezone renders as
/// in [`NativeAgentExtensions::effective_digest_framing`].
///
/// It carries a NUL, which no IANA timezone name may contain, so an inactive
/// zone can never be confused with a zone that actually executes.
const INACTIVE_TIMEZONE_FRAMING: &str = "\u{0}inactive";

/// The canonical authored name of the Agent Status extension.
pub const AGENT_STATUS_EXTENSION: &str = "agentStatus";

/// The canonical authored name of the Todo extension (Issue #259).
pub const TODO_EXTENSION: &str = "todo";

#[cfg(test)]
mod tests {
    use super::*;

    fn document(value: serde_json::Value) -> NativeAgentExtensionsDocument {
        serde_json::from_value(value).expect("the closed extension document parses")
    }

    #[test]
    fn cfg273_omitted_extension_document_selects_no_extensions() {
        assert!(
            NativeAgentExtensionsDocument::default()
                .resolve()
                .is_empty()
        );
        assert!(document(serde_json::json!({})).resolve().is_empty());
    }

    #[test]
    fn cfg332_root_plugins_default_off() {
        assert!(
            crate::local_runtime::config::builtin_root_profile()
                .extensions
                .resolve()
                .is_empty()
        );
    }

    /// The two members are independent axes. Switching either off leaves the
    /// other exactly as it was, and only switching both off yields the empty
    /// composition.
    #[test]
    fn ext259_todo_and_agent_status_compose_independently() {
        let todo_only = document(serde_json::json!({"todo": {"enabled": true}})).resolve();
        assert!(todo_only.agent_status().is_none() && todo_only.todo().is_some());
        assert_eq!(todo_only, NativeAgentExtensions::with_todo());
        assert!(!todo_only.is_empty());
        assert_eq!(
            todo_only.prospective_tool_names(),
            vec![crate::tools::native::TODO_TOOL_NAME]
        );

        let status_only =
            document(serde_json::json!({"agent_status": {"enabled": true}})).resolve();
        assert!(status_only.agent_status().is_some() && status_only.todo().is_none());
        assert!(
            status_only.prospective_tool_names().is_empty(),
            "Agent Status contributes context, never a Tool"
        );

        let neither = document(
            serde_json::json!({"agent_status": {"enabled": false}, "todo": {"enabled": false}}),
        )
        .resolve();
        assert_eq!(neither, NativeAgentExtensions::none());
        assert!(neither.is_empty());
        assert!(composed_extension_names(&neither).is_empty());
    }

    /// Todo is supported in one-shot child scope: a child is an ordinary
    /// conversation with its own Ledger, so it composes its own list.
    #[test]
    fn ext259_todo_is_supported_in_child_scope() {
        for composition in [
            NativeAgentExtensions::with_todo(),
            NativeAgentExtensionsDocument::default().resolve(),
        ] {
            assert!(unsupported_child_scope(&composition).is_none());
        }
    }

    #[test]
    fn ext256_disabling_agent_status_removes_the_extension_from_the_composition() {
        let resolved = document(
            serde_json::json!({"agent_status": {"enabled": false}, "todo": {"enabled": false}}),
        )
        .resolve();
        assert_eq!(resolved, NativeAgentExtensions::none());
        assert!(resolved.is_empty());
        assert!(resolved.agent_status().is_none());
        assert!(
            resolved
                .agent_status_engine(Arc::new(crate::context::SystemClock))
                .is_none(),
            "an absent extension materializes no engine"
        );
    }

    #[test]
    fn ext256_contributor_settings_survive_the_freeze_unchanged() {
        let resolved = document(serde_json::json!({
            "agent_status": {
                "enabled": true,
                "time": {"enabled": true, "timezone": "Asia/Shanghai"},
                "background": {"enabled": false}
            }
        }))
        .resolve();
        let agent_status = resolved.agent_status().expect("Agent Status is composed");
        assert!(agent_status.time.enabled);
        assert_eq!(
            agent_status
                .time
                .timezone
                .map(|zone| zone.name().to_owned()),
            Some("Asia/Shanghai".to_owned())
        );
        assert!(!agent_status.background.enabled);
    }

    /// Disabling the extension keeps its contributor settings out of the
    /// frozen composition entirely: absence is one fact, not "present but
    /// with both contributors off".
    #[test]
    fn ext256_disabled_agent_status_discards_contributor_configuration() {
        let resolved = document(serde_json::json!({
            "agent_status": {"enabled": false, "time": {"timezone": "Asia/Shanghai"}}
        }))
        .resolve();
        assert!(resolved.agent_status().is_none());
    }

    #[test]
    fn ext256_unknown_extension_names_and_fields_are_rejected() {
        for value in [
            serde_json::json!({"futureGoal": {"enabled": true}}),
            serde_json::json!({"agent_status": {"future": true}}),
            serde_json::json!({"agent_status": {"enabled": true, "time": {"future": true}}}),
            serde_json::json!({"agent_status": {"enabled": true, "background": {"future": true}}}),
            // Todo's closed record is just as strict: it has one member.
            serde_json::json!({"todo": {"future": true}}),
            serde_json::json!({"todo": {"enabled": "true"}}),
        ] {
            assert!(
                serde_json::from_value::<NativeAgentExtensionsDocument>(value.clone()).is_err(),
                "accepted {value}"
            );
        }
    }

    #[test]
    fn ext256_the_digest_framing_separates_every_semantic_composition() {
        let framings = [
            NativeAgentExtensions::none(),
            NativeAgentExtensions::with_todo().and_agent_status(AgentStatusConfig::default()),
            document(serde_json::json!({"agent_status": {"enabled": true, "time": {"enabled": false}}})).resolve(),
            document(serde_json::json!({"agent_status": {"enabled": true, "background": {"enabled": false}}}))
                .resolve(),
            document(serde_json::json!({"agent_status": {"enabled": true, "time": {"timezone": "Asia/Shanghai"}}}))
                .resolve(),
            // Issue #259: Todo is framed too, so a role that composes it is a
            // different definition from one that does not.
            document(serde_json::json!({"agent_status": {"enabled": true}})).resolve(),
            NativeAgentExtensions::with_todo(),
        ]
        .map(|composition| composition.authored_digest_framing());
        let mut unique = framings.to_vec();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            framings.len(),
            "every semantic composition frames differently"
        );
    }

    /// The Runtime Client effective-extension projection reads the frozen
    /// composition back off the owners it materialized. That round trip is
    /// the whole reason there is only one source of truth, so it is proven
    /// for every semantic composition rather than assumed.
    #[test]
    fn ext256_materialized_owners_recover_the_exact_frozen_composition() {
        for composition in [
            NativeAgentExtensions::none(),
            crate::local_runtime::config::builtin_root_profile()
                .extensions
                .resolve(),
            document(serde_json::json!({"agent_status": {"enabled": true, "time": {"enabled": false}}})).resolve(),
            document(serde_json::json!({"agent_status": {"enabled": true, "background": {"enabled": false}}}))
                .resolve(),
            document(serde_json::json!({"agent_status": {"enabled": true, "time": {"timezone": "Asia/Shanghai"}}}))
                .resolve(),
            document(serde_json::json!({"agent_status": {"enabled": true}})).resolve(),
            NativeAgentExtensions::with_todo(),
        ] {
            let engine = composition.agent_status_engine(Arc::new(crate::context::SystemClock));
            // The Todo owner a composition materializes is the conversation's
            // own list; its presence is the composed/absent fact, exactly as
            // the status engine's is.
            let todos = composition.todo().map(|_| {
                crate::tools::todo::ConversationTodoList::new(
                    crate::runtime::identity::ConversationId::new(
                        "conv_5570a61c-c5b9-7294-843c-8fabb529cb86",
                    ),
                )
            });
            assert_eq!(
                NativeAgentExtensions::from_materialized(engine.as_ref(), todos.as_ref(), None),
                composition,
                "the materialized owners recover exactly what was frozen"
            );
        }
    }

    /// The frozen composition is what crosses the subagent process
    /// boundary, so it must survive its real serialization contract exactly.
    #[test]
    fn ext256_the_frozen_composition_survives_its_wire_contract() {
        for composition in [
            NativeAgentExtensions::none(),
            NativeAgentExtensions::with_todo(),
            document(serde_json::json!({"agent_status": {"enabled": true, "time": {"timezone": "Asia/Shanghai"}}}))
                .resolve(),
        ] {
            let encoded = serde_json::to_vec(&composition).expect("encodes");
            let decoded: NativeAgentExtensions = serde_json::from_slice(&encoded).expect("decodes");
            assert_eq!(decoded, composition);
        }
    }

    fn zone(name: &str) -> chrono_tz::Tz {
        name.parse().expect("a real IANA timezone")
    }

    /// One Agent Status composition, spelled as the three facts that matter.
    fn status(
        time: bool,
        timezone: Option<chrono_tz::Tz>,
        background: bool,
    ) -> NativeAgentExtensions {
        NativeAgentExtensions::with_agent_status(AgentStatusConfig {
            time: TimeStatusConfig {
                enabled: time,
                timezone,
            },
            background: BackgroundStatusConfig {
                enabled: background,
            },
        })
    }

    /// Omitted timezone renders the native UTC default.
    #[test]
    fn sub258_the_effective_timezone_owner_is_the_presentation_owner() {
        assert_eq!(
            TimeStatusConfig {
                enabled: true,
                timezone: None,
            }
            .effective_timezone()
            .name(),
            "UTC",
            "an absent zone renders the UTC label the Time contributor emits"
        );
        assert_eq!(
            crate::context::effective_status_timezone(Some(zone("Asia/Shanghai"))).name(),
            "Asia/Shanghai"
        );
    }

    /// The two framings answer two different questions, and the difference
    /// is exactly the authored timezone spelling.
    #[test]
    fn sub258_effective_framing_normalizes_a_timezone_the_authored_framing_keeps() {
        let omitted = status(true, None, true);
        let explicit_utc = status(true, Some(chrono_tz::UTC), true);
        assert_ne!(
            omitted.authored_digest_framing(),
            explicit_utc.authored_digest_framing(),
            "two different source documents keep two source identities"
        );
        assert_eq!(
            omitted.effective_digest_framing(),
            explicit_utc.effective_digest_framing(),
            "one rendered behavior is one effective execution profile"
        );
        assert_ne!(
            omitted.effective_digest_framing(),
            status(true, Some(zone("Asia/Shanghai")), true).effective_digest_framing(),
            "a different rendered zone is a different effective profile"
        );
    }

    /// A **disabled** Time contributor never executes, so no timezone
    /// spelling may separate two effective execution profiles — while the
    /// authored framing keeps every one of them apart, because it identifies
    /// the source document rather than the behavior.
    ///
    /// The two questions and their two answers:
    ///
    /// ```text
    /// effective  time off + UTC == time off + Asia/Shanghai == time off + omitted
    /// authored   all three are three different source documents
    /// ```
    #[test]
    fn sub258_a_disabled_time_contributor_has_no_effective_timezone() {
        let disabled_utc = status(false, Some(chrono_tz::UTC), true);
        let disabled_shanghai = status(false, Some(zone("Asia/Shanghai")), true);
        let disabled_omitted = status(false, None, true);

        assert_eq!(
            disabled_utc.effective_digest_framing(),
            disabled_shanghai.effective_digest_framing(),
            "a zone that never renders cannot identify a frozen composition"
        );
        assert_eq!(
            disabled_utc.effective_digest_framing(),
            disabled_omitted.effective_digest_framing(),
            "an omitted zone is the same inactive configuration"
        );

        // The source-definition question is a different question, and its
        // answer deliberately did not change.
        let authored = [
            disabled_utc.authored_digest_framing(),
            disabled_shanghai.authored_digest_framing(),
            disabled_omitted.authored_digest_framing(),
        ];
        let mut unique = authored.to_vec();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            authored.len(),
            "three different source documents keep three source identities"
        );
    }

    /// An **enabled** Time contributor's effective zone is behavior and still
    /// identifies the profile, and the already-correct normalization of an
    /// omitted zone survives the disabled-case correction.
    #[test]
    fn sub258_an_enabled_time_contributor_still_frames_its_effective_timezone() {
        assert_ne!(
            status(true, Some(chrono_tz::UTC), true).effective_digest_framing(),
            status(true, Some(zone("Asia/Shanghai")), true).effective_digest_framing(),
            "an enabled contributor renders its zone, so the zone is behavior"
        );
        assert_eq!(
            status(true, Some(chrono_tz::UTC), true).effective_digest_framing(),
            status(true, None, true).effective_digest_framing(),
            "an omitted zone renders UTC, so the two produce one Time behavior"
        );

        // Enabling Time is itself behavior, and the inactive sentinel can
        // never be confused with a zone that actually executes: no IANA name
        // may contain a NUL.
        for timezone in [None, Some(chrono_tz::UTC), Some(zone("Asia/Shanghai"))] {
            assert_ne!(
                status(true, timezone, true).effective_digest_framing(),
                status(false, timezone, true).effective_digest_framing(),
                "switching the contributor off is a different effective profile"
            );
        }
        assert!(
            !status(true, Some(chrono_tz::UTC), true)
                .effective_digest_framing()
                .contains(super::INACTIVE_TIMEZONE_FRAMING),
            "an executing zone never frames as the inactive sentinel"
        );
    }
}
