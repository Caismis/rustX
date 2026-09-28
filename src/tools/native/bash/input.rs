//! The typed model-facing input contract of the native Bash tool.

use std::time::Duration;

use schemars::JsonSchema;
use serde::Deserialize;

use crate::tools::native::input::decode;

/// The canonical input contract of the Bash tool.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct BashInput {
    /// The command handed to one `/bin/bash -c` invocation.
    #[schemars(length(min = 1))]
    pub command: String,
    /// Optional presentation intent, never execution or approval authority.
    /// One to 160 Unicode scalar values; whitespace-only values are rejected.
    #[schemars(length(min = 1, max = 160))]
    pub description: Option<String>,
    /// The invocation deadline in seconds. When omitted, no executor-local
    /// deadline applies: a foreground invocation is bounded by the generic
    /// Agent Loop execution-liveness hard deadline (Issue #204), and a
    /// background execution is conversation-owned and unbounded.
    #[schemars(range(min = 1))]
    pub timeout: Option<u64>,
}

impl BashInput {
    /// Deserializes and semantically validates one Bash invocation.
    ///
    /// Nothing here touches the process plane: the supervisor unit is
    /// spawned only after the input contract holds.
    ///
    /// # Errors
    ///
    /// Returns the deterministic rejection message of the first input
    /// contract violation.
    pub(super) fn parse(arguments: &serde_json::Value) -> Result<Self, String> {
        let input: Self = decode(super::NAME, arguments)?;
        input.validate()?;
        Ok(input)
    }

    /// The tool-specific semantic rule of the command: an empty command
    /// has no invocation to own. The generated schema states the same
    /// constraint, so this rule also holds for a direct executor call.
    fn validate(&self) -> Result<(), String> {
        if self.command.is_empty() {
            return Err("bash requires a non-empty command".to_owned());
        }
        if let Some(description) = &self.description
            && (description.trim().is_empty() || description.chars().count() > 160)
        {
            return Err("bash description must contain 1 to 160 Unicode scalar values and must not be blank".to_owned());
        }
        Ok(())
    }

    /// The explicitly requested invocation deadline, if any.
    ///
    /// This is the one unit boundary of the Bash tool: the model-facing
    /// contract is measured in **seconds**, and it is converted to the
    /// internal [`Duration`] representation here, at the tool boundary. The
    /// executor, the supervisor, and the whole process plane below it keep
    /// working in [`Duration`] and never see the model-facing unit.
    pub(super) fn explicit_timeout(&self) -> Option<Duration> {
        self.timeout.map(Duration::from_secs)
    }
}

#[cfg(test)]
mod tests {
    use super::BashInput;
    #[test]
    fn description_is_bounded_unicode_presentation_only() {
        let command = "printf '%s' unchanged";
        for description in [
            None,
            Some("Run the check".to_owned()),
            Some("😀".repeat(160)),
            Some("\u{1b}[2J<script>".to_owned()),
        ] {
            let mut value = serde_json::json!({"command": command, "timeout": 42});
            if let Some(description) = description {
                value["description"] = description.into();
            }
            let input = BashInput::parse(&value).unwrap();
            assert_eq!(input.command, command);
            assert_eq!(
                input.explicit_timeout(),
                Some(std::time::Duration::from_secs(42))
            );
        }
        for description in [String::new(), " \t\n".to_owned(), "😀".repeat(161)] {
            assert!(
                BashInput::parse(
                    &serde_json::json!({"command": command, "description": description})
                )
                .is_err()
            );
        }
        assert!(
            BashInput::parse(&serde_json::json!({"command": command, "justification": "please"}))
                .is_err()
        );
    }
}
