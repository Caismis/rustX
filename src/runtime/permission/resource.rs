//! Per-resource three-mode permission overrides.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::mode::ExecutionPermissionMode;

/// One mode entry on a resource permission object.
///
/// Automatic model review is never a resource enum value: it is Automatic
/// mode's built-in `default` behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(schemars::JsonSchema)]
pub enum ResourcePermissionValue {
    /// Delegate to the selected mode's built-in matrix behavior.
    #[default]
    Default,
    /// Allow the call without ordinary approval or Auxiliary review.
    Allow,
    /// Require human approval for the exact frozen invocation.
    Ask,
    /// Hard-deny this invocation; elevation cannot override it.
    Deny,
}

impl ResourcePermissionValue {
    /// Stable `snake_case` spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Allow => "allow",
            Self::Ask => "ask",
            Self::Deny => "deny",
        }
    }
}

/// Complete three-mode permission object for one resource or exact tool.
///
/// An authored object must supply all three entries. Omission of the whole
/// object inherits the winning source default or product defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct ResourcePermissionSet {
    /// Plan-mode override.
    pub plan: ResourcePermissionValue,
    /// Automatic-mode override.
    pub automatic: ResourcePermissionValue,
    /// Full Access-mode override.
    pub full_access: ResourcePermissionValue,
}

impl Default for ResourcePermissionSet {
    fn default() -> Self {
        Self {
            plan: ResourcePermissionValue::Default,
            automatic: ResourcePermissionValue::Default,
            full_access: ResourcePermissionValue::Default,
        }
    }
}

impl ResourcePermissionSet {
    /// Selects the entry for the active mode.
    #[must_use]
    pub const fn for_mode(self, mode: ExecutionPermissionMode) -> ResourcePermissionValue {
        match mode {
            ExecutionPermissionMode::Plan => self.plan,
            ExecutionPermissionMode::Automatic => self.automatic,
            ExecutionPermissionMode::FullAccess => self.full_access,
        }
    }
}

/// Resolves the effective resource permission for one mode.
///
/// Exact-tool objects completely replace source defaults when present.
/// `None` means no authored override — use built-in matrix defaults.
#[must_use]
pub fn resolve_resource_permission(
    exact_tool: Option<ResourcePermissionSet>,
    source_default: Option<ResourcePermissionSet>,
    mode: ExecutionPermissionMode,
) -> ResourcePermissionValue {
    exact_tool
        .or(source_default)
        .unwrap_or_default()
        .for_mode(mode)
}

/// Selects the stricter of two mode-local resource permission values.
///
/// Aggregate multi-source helpers require approval when any queried source
/// requires it, so Deny beats Ask, Ask beats Default, and Default beats Allow.
#[must_use]
pub const fn stricter_resource_permission(
    left: ResourcePermissionValue,
    right: ResourcePermissionValue,
) -> ResourcePermissionValue {
    use ResourcePermissionValue::{Allow, Ask, Default, Deny};
    match (left, right) {
        (Deny, _) | (_, Deny) => Deny,
        (Ask, _) | (_, Ask) => Ask,
        (Default, _) | (_, Default) => Default,
        (Allow, Allow) => Allow,
    }
}

/// Resolves the mode-local permission for one prepared invocation.
///
/// A frozen single-source object wins. Otherwise a non-empty per-server map
/// resolves the named `server` argument, or the strictest value across every
/// target when the call aggregates without a server.
#[must_use]
pub fn effective_invocation_resource_permission(
    frozen: Option<ResourcePermissionSet>,
    targets: &BTreeMap<String, ResourcePermissionSet>,
    arguments: &serde_json::Value,
    mode: ExecutionPermissionMode,
) -> ResourcePermissionValue {
    if let Some(set) = frozen {
        return set.for_mode(mode);
    }
    if targets.is_empty() {
        return ResourcePermissionValue::Default;
    }
    if let Some(server) = arguments.get("server").and_then(serde_json::Value::as_str) {
        return targets
            .get(server)
            .copied()
            .unwrap_or_default()
            .for_mode(mode);
    }
    targets
        .values()
        .map(|set| set.for_mode(mode))
        .fold(ResourcePermissionValue::Allow, stricter_resource_permission)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_tool_replaces_source_default() {
        let source = ResourcePermissionSet {
            plan: ResourcePermissionValue::Allow,
            automatic: ResourcePermissionValue::Allow,
            full_access: ResourcePermissionValue::Allow,
        };
        let exact = ResourcePermissionSet {
            plan: ResourcePermissionValue::Deny,
            automatic: ResourcePermissionValue::Ask,
            full_access: ResourcePermissionValue::Ask,
        };
        assert_eq!(
            resolve_resource_permission(Some(exact), Some(source), ExecutionPermissionMode::Plan),
            ResourcePermissionValue::Deny
        );
        assert_eq!(
            resolve_resource_permission(
                Some(exact),
                Some(source),
                ExecutionPermissionMode::Automatic
            ),
            ResourcePermissionValue::Ask
        );
    }

    #[test]
    fn omitted_object_uses_default_entries() {
        assert_eq!(
            resolve_resource_permission(None, None, ExecutionPermissionMode::FullAccess),
            ResourcePermissionValue::Default
        );
    }

    #[test]
    fn named_target_uses_selected_server_object() {
        let mut targets = BTreeMap::new();
        targets.insert(
            "alpha".into(),
            ResourcePermissionSet {
                plan: ResourcePermissionValue::Deny,
                automatic: ResourcePermissionValue::Deny,
                full_access: ResourcePermissionValue::Deny,
            },
        );
        targets.insert(
            "beta".into(),
            ResourcePermissionSet {
                plan: ResourcePermissionValue::Allow,
                automatic: ResourcePermissionValue::Allow,
                full_access: ResourcePermissionValue::Allow,
            },
        );
        assert_eq!(
            effective_invocation_resource_permission(
                None,
                &targets,
                &serde_json::json!({"server":"beta"}),
                ExecutionPermissionMode::Automatic,
            ),
            ResourcePermissionValue::Allow
        );
        assert_eq!(
            effective_invocation_resource_permission(
                None,
                &targets,
                &serde_json::json!({}),
                ExecutionPermissionMode::Automatic,
            ),
            ResourcePermissionValue::Deny
        );
    }

    #[test]
    fn aggregate_includes_default_servers_against_allow_overrides() {
        let mut targets = BTreeMap::new();
        targets.insert(
            "allowing".into(),
            ResourcePermissionSet {
                plan: ResourcePermissionValue::Allow,
                automatic: ResourcePermissionValue::Allow,
                full_access: ResourcePermissionValue::Allow,
            },
        );
        targets.insert("defaulting".into(), ResourcePermissionSet::default());
        assert_eq!(
            effective_invocation_resource_permission(
                None,
                &targets,
                &serde_json::json!({}),
                ExecutionPermissionMode::Automatic,
            ),
            ResourcePermissionValue::Default
        );
    }
}
