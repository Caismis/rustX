//! Captured host credentials and explicit references. No document interpolation.

use crate::model::catalog::{CredentialEnvironment, ResolvedCredential};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, OnceLock};

/// A validated reference, written as `$ENV_VAR` only in declared secret fields.
#[derive(Debug, Clone, PartialEq, Eq, schemars::JsonSchema)]
#[schemars(transparent)]
pub struct EnvironmentReference(#[schemars(pattern(r"^\$[A-Za-z_][A-Za-z0-9_]*$"))] String);

/// Environment names are ASCII identifiers, never shell expressions.
#[must_use]
pub fn valid_environment_name(name: &str) -> bool {
    name.as_bytes()
        .first()
        .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_')
        && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

/// Names referenced by `$NAME` in an MCP HTTP template.
pub(crate) fn referenced_environment_names(value: &str) -> BTreeSet<String> {
    let bytes = value.as_bytes();
    let mut names = BTreeSet::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'$'
            || index + 1 == bytes.len()
            || !(bytes[index + 1].is_ascii_alphabetic() || bytes[index + 1] == b'_')
        {
            index += 1;
            continue;
        }
        let start = index + 1;
        index = start + 1;
        while index < bytes.len() && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
        {
            index += 1;
        }
        names.insert(value[start..index].to_owned());
    }
    names
}

impl EnvironmentReference {
    pub(crate) fn name(&self) -> &str {
        &self.0
    }

    pub(crate) fn from_name(name: String) -> Self {
        debug_assert!(valid_environment_name(&name));
        Self(name)
    }
}

impl Serialize for EnvironmentReference {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!("${}", self.0))
    }
}

impl<'de> Deserialize<'de> for EnvironmentReference {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Parse through Value so type errors cannot print a supplied secret.
        let value = serde_json::Value::deserialize(deserializer)?;
        let name = value
            .as_str()
            .and_then(|s| s.strip_prefix('$'))
            .filter(|name| valid_environment_name(name))
            .ok_or_else(|| serde::de::Error::custom("expected a $ENV_VAR credential reference"))?;
        Ok(Self(name.to_owned()))
    }
}

/// Immutable host snapshot. Values cannot be serialized or debug-printed.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct CredentialSnapshot(Arc<BTreeMap<String, String>>);

impl std::fmt::Debug for CredentialSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CredentialSnapshot(<redacted>)")
    }
}

impl CredentialSnapshot {
    /// Capture once at the host boundary, never in a reconnect loop.
    #[must_use]
    pub fn capture() -> Self {
        #[cfg(test)]
        crate::local_runtime::static_effects::observe(
            crate::local_runtime::static_effects::Effect::Credentials,
        );
        Self::new(
            std::env::vars_os().filter_map(|(key, value)| {
                Some((key.into_string().ok()?, value.into_string().ok()?))
            }),
        )
    }

    /// Explicit injection for hosts and deterministic tests.
    pub fn new(values: impl IntoIterator<Item = (String, String)>) -> Self {
        Self(Arc::new(values.into_iter().collect()))
    }
}

impl CredentialEnvironment for CredentialSnapshot {
    fn snapshot(&self) -> Self {
        self.clone()
    }
    fn var(&self, name: &str) -> Option<String> {
        #[cfg(test)]
        crate::local_runtime::static_effects::observe(
            crate::local_runtime::static_effects::Effect::Credentials,
        );
        self.0.get(name).cloned()
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClientHydrationState {
    #[default]
    Unhydrated,
    Hydrated,
}

/// Declared source secrets plus a private, once-resolved instance cache.
/// Only reference names cross normal serialization; resolved bytes never do.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceCredentials {
    /// Sensitive process entries, separate from ordinary literal environment.
    #[serde(default)]
    pub environment: BTreeMap<String, EnvironmentReference>,
    /// Sensitive HTTP headers, separate from ordinary literal headers.
    #[serde(default)]
    pub headers: BTreeMap<String, EnvironmentReference>,
    /// Workspace sources can only use values granted by their Client.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) client_owned: bool,
    /// The winning definition originated in the Workspace document.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) workspace_source: bool,
    /// Variables used in workspace HTTP endpoint and header templates.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) templates: BTreeMap<String, EnvironmentReference>,
    #[serde(skip)]
    pub(crate) captured: CredentialSnapshot,
    #[serde(skip)]
    pub(crate) resolved: Arc<OnceLock<Result<ResolvedSourceCredentials, String>>>,
    /// Native transport literals remain write-only in every observation.
    #[serde(skip)]
    pub(crate) literal_redactions: Vec<ResolvedCredential>,
    #[serde(skip)]
    pub(crate) client_hydration: ClientHydrationState,
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde's skip predicate receives a reference.
fn is_false(value: &bool) -> bool {
    !*value
}

impl PartialEq for SourceCredentials {
    fn eq(&self, other: &Self) -> bool {
        self.environment == other.environment
            && self.headers == other.headers
            && self.client_owned == other.client_owned
            && self.workspace_source == other.workspace_source
            && self.templates == other.templates
            && self.captured == other.captured
            && self.literal_redactions == other.literal_redactions
    }
}
impl Eq for SourceCredentials {}

/// Values exposed only to the existing process and connection constructors.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedSourceCredentials {
    pub environment: BTreeMap<String, ResolvedCredential>,
    pub headers: BTreeMap<String, ResolvedCredential>,
    pub templates: BTreeMap<String, ResolvedCredential>,
}

impl SourceCredentials {
    /// Private process transfer of this already captured source environment.
    pub(crate) fn export_process_credentials(&mut self, output: &mut Vec<(String, String)>) {
        let base = output.len();
        for (index, reference) in self
            .environment
            .values_mut()
            .chain(self.headers.values_mut())
            .chain(self.templates.values_mut())
            .enumerate()
        {
            let name = format!("RUSTX_ADMITTED_SOURCE_{}", base + index);
            let value = self.captured.var(&reference.0).unwrap_or_default();
            output.push((name.clone(), value));
            reference.0 = name;
        }
    }

    pub(crate) fn client_environment_names(&self) -> BTreeSet<String> {
        self.templates
            .values()
            .chain(self.headers.values())
            .map(|reference| reference.name().to_owned())
            .collect()
    }

    pub(crate) fn needs_client_hydration(&self) -> bool {
        self.client_owned
            && self.client_hydration == ClientHydrationState::Unhydrated
            && self
                .templates
                .values()
                .chain(self.headers.values())
                .any(|reference| self.captured.var(reference.name()).is_none())
    }

    pub(crate) fn mark_client_hydrated(&mut self) {
        self.client_hydration = ClientHydrationState::Hydrated;
    }

    /// Capture the immutable environment for a newly admitted child instance.
    pub fn capture_from(&mut self, environment: &dyn CredentialEnvironment) {
        self.capture(environment.snapshot());
    }

    pub(crate) fn redact(&self, message: &str) -> String {
        let mut values = self
            .literal_redactions
            .iter()
            .map(ResolvedCredential::expose)
            .collect::<Vec<_>>();
        if let Some(Ok(resolved)) = self.resolved.get() {
            values.extend(
                resolved
                    .environment
                    .values()
                    .chain(resolved.headers.values())
                    .chain(resolved.templates.values())
                    .map(ResolvedCredential::expose),
            );
        }
        // Redact complete values before any overlapping shorter value.
        values.sort_unstable_by(|left, right| right.len().cmp(&left.len()).then(left.cmp(right)));
        values.dedup();
        values.retain(|value| !value.is_empty());
        let mut redacted = String::with_capacity(message.len());
        let mut remaining = message;
        while !remaining.is_empty() {
            if let Some(value) = values.iter().find(|value| remaining.starts_with(**value)) {
                redacted.push_str("<redacted>");
                remaining = &remaining[value.len()..];
            } else {
                let character = remaining.chars().next().expect("nonempty text");
                redacted.push(character);
                remaining = &remaining[character.len_utf8()..];
            }
        }
        redacted
    }
    /// Bind host authority without resolving any reference.
    pub fn capture(&mut self, captured: CredentialSnapshot) {
        self.captured = captured;
        self.resolved = Arc::new(OnceLock::new());
    }

    pub(crate) fn resolve(&self) -> Result<&ResolvedSourceCredentials, String> {
        self.resolved
            .get_or_init(|| {
                let resolve = |refs: &BTreeMap<String, EnvironmentReference>| {
                    refs.iter()
                        .map(|(key, reference)| {
                            let value = self
                                .captured
                                .var(&reference.0)
                                .filter(|value| !value.is_empty())
                                .ok_or_else(|| {
                                    format!("required credential env:{} is not set", reference.0)
                                })?;
                            Ok((key.clone(), ResolvedCredential::new(value)))
                        })
                        .collect::<Result<BTreeMap<_, _>, String>>()
                };
                Ok(ResolvedSourceCredentials {
                    environment: resolve(&self.environment)?,
                    headers: resolve(&self.headers)?,
                    templates: resolve(&self.templates)?,
                })
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    pub(crate) fn interpolate(&self, value: &str) -> Result<String, String> {
        let resolved = self.resolve()?;
        let bytes = value.as_bytes();
        let mut result = String::with_capacity(value.len());
        let mut start = 0;
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] != b'$'
                || index + 1 == bytes.len()
                || !(bytes[index + 1].is_ascii_alphabetic() || bytes[index + 1] == b'_')
            {
                index += 1;
                continue;
            }
            let name_start = index + 1;
            let mut end = name_start + 1;
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
                end += 1;
            }
            result.push_str(&value[start..index]);
            let secret = resolved
                .templates
                .get(&value[name_start..end])
                .ok_or_else(|| "MCP environment reference is unavailable".to_owned())?;
            result.push_str(secret.expose());
            index = end;
            start = end;
        }
        result.push_str(&value[start..]);
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SENTINEL: &str = "CFG233_SENTINEL_NEVER_PROJECT_8b94";

    #[test]
    fn client_http_credentials_round_trip_as_references_for_fresh_authorization() {
        assert_eq!(
            referenced_environment_names("Bearer $TOKEN and $OTHER/$TOKEN"),
            BTreeSet::from(["OTHER".into(), "TOKEN".into()])
        );
        let mut credentials = SourceCredentials {
            client_owned: true,
            headers: BTreeMap::from([(
                "X-Token".into(),
                EnvironmentReference::from_name("HEADER_TOKEN".into()),
            )]),
            templates: BTreeMap::from([(
                "TOKEN".into(),
                EnvironmentReference::from_name("TOKEN".into()),
            )]),
            ..Default::default()
        };
        credentials.capture(CredentialSnapshot::new([
            ("TOKEN".into(), SENTINEL.into()),
            ("HEADER_TOKEN".into(), SENTINEL.into()),
        ]));
        credentials.mark_client_hydrated();
        assert_eq!(
            credentials.interpolate("Bearer $TOKEN").unwrap(),
            format!("Bearer {SENTINEL}")
        );
        assert!(!format!("{credentials:?}").contains(SENTINEL));
        let wire = serde_json::to_string(&credentials).unwrap();
        assert!(!wire.contains(SENTINEL));
        let mut restored: SourceCredentials = serde_json::from_str(&wire).unwrap();
        assert!(restored.needs_client_hydration());
        assert!(restored.resolve().is_err());
        assert_eq!(
            restored.client_environment_names(),
            BTreeSet::from(["HEADER_TOKEN".into(), "TOKEN".into()])
        );
        restored.capture(CredentialSnapshot::new([
            ("TOKEN".into(), "new-template-value".into()),
            ("HEADER_TOKEN".into(), "new-header-value".into()),
        ]));
        assert_eq!(
            restored.interpolate("Bearer $TOKEN").unwrap(),
            "Bearer new-template-value"
        );
        assert_eq!(
            restored.resolve().unwrap().headers["X-Token"].expose(),
            "new-header-value"
        );
    }

    #[test]
    fn literal_redactions_are_private_nonempty_and_longest_first() {
        let credentials = SourceCredentials {
            literal_redactions: ["", "literal", "literal-secret"]
                .into_iter()
                .map(ResolvedCredential::new)
                .collect(),
            ..Default::default()
        };
        assert_eq!(
            credentials.redact("literal-secret literal"),
            "<redacted> <redacted>"
        );
        for projection in [
            format!("{credentials:?}"),
            serde_json::to_string(&credentials).unwrap(),
        ] {
            assert!(!projection.contains("literal-secret"));
        }
    }

    #[test]
    fn redaction_never_rescans_inserted_masks() {
        let mask = "<redacted>";
        let mut credentials = SourceCredentials::default();
        credentials
            .literal_redactions
            .push(ResolvedCredential::new("secret"));
        for start in 0..mask.len() {
            for end in start + 1..=mask.len() {
                credentials
                    .literal_redactions
                    .push(ResolvedCredential::new(&mask[start..end]));
            }
        }
        assert_eq!(credentials.redact("secret / λ"), "<redacted> / λ");
    }

    #[test]
    fn references_are_explicit_validated_and_not_interpolation() {
        for value in [serde_json::json!("$KEY_1"), serde_json::json!("$_KEY")] {
            assert!(serde_json::from_value::<EnvironmentReference>(value).is_ok());
        }
        for value in [
            serde_json::json!(SENTINEL),
            serde_json::json!("${KEY}"),
            serde_json::json!("$(command)"),
            serde_json::json!("$1BAD"),
            serde_json::json!({"env": SENTINEL}),
            serde_json::json!(""),
        ] {
            let error = serde_json::from_value::<EnvironmentReference>(value).unwrap_err();
            assert!(!error.to_string().contains(SENTINEL));
        }
    }

    #[test]
    fn source_credentials_freeze_once_and_never_serialize_resolved_values() {
        let mut credentials: SourceCredentials = serde_json::from_value(serde_json::json!({
            "environment": {"TOKEN":"$KEY"}, "headers":{"Authorization":"$KEY"}
        }))
        .unwrap();
        credentials.capture(CredentialSnapshot::new([("KEY".into(), SENTINEL.into())]));
        let first = credentials.resolve().unwrap();
        assert_eq!(first.headers["Authorization"].expose(), SENTINEL);
        assert!(std::ptr::eq(first, credentials.resolve().unwrap()));
        for projection in [
            format!("{credentials:?}"),
            serde_json::to_string(&credentials).unwrap(),
            credentials.redact(&format!("peer echoed {SENTINEL}")),
        ] {
            assert!(!projection.contains(SENTINEL));
        }
        let replay: SourceCredentials =
            serde_json::from_str(&serde_json::to_string(&credentials).unwrap()).unwrap();
        assert!(replay.resolve().unwrap_err().contains("env:KEY"));
    }

    #[test]
    fn missing_reference_is_frozen_and_not_an_empty_credential() {
        let credentials: SourceCredentials =
            serde_json::from_value(serde_json::json!({"environment":{"TOKEN":"$MISSING"}}))
                .unwrap();
        let first = credentials.resolve().unwrap_err();
        assert_eq!(first, "required credential env:MISSING is not set");
        assert_eq!(credentials.resolve().unwrap_err(), first);
    }

    #[test]
    fn private_child_transfer_preserves_each_source_instance_without_secret_serialization() {
        let mut output = Vec::new();
        let mut sources = Vec::new();
        for value in ["CFG233_FIRST_SOURCE_SECRET", "CFG233_SECOND_SOURCE_SECRET"] {
            let mut source: SourceCredentials =
                serde_json::from_value(serde_json::json!({"environment":{"TOKEN":"$SHARED_NAME"}}))
                    .unwrap();
            source.capture(CredentialSnapshot::new([(
                "SHARED_NAME".into(),
                value.into(),
            )]));
            source.resolve().unwrap();
            source.export_process_credentials(&mut output);
            sources.push(source);
        }
        assert_ne!(output[0].0, output[1].0);
        let child_environment = CredentialSnapshot::new(output);
        for (source, expected) in sources
            .iter()
            .zip(["CFG233_FIRST_SOURCE_SECRET", "CFG233_SECOND_SOURCE_SECRET"])
        {
            let wire = serde_json::to_string(source).unwrap();
            assert!(!wire.contains(expected));
            let mut child: SourceCredentials = serde_json::from_str(&wire).unwrap();
            child.capture(child_environment.clone());
            assert_eq!(
                child.resolve().unwrap().environment["TOKEN"].expose(),
                expected
            );
        }
    }
}
