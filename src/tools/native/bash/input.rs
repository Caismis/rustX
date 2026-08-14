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
    /// The invocation deadline in seconds. A foreground invocation without
    /// one uses the default foreground timeout.
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
        Ok(())
    }

    /// The explicitly requested invocation deadline, if any.
    ///
    /// This is the one unit boundary of the Bash tool: the model-facing
    /// contract is measured in **seconds**, and it is converted to the
    /// internal [`Duration`] representation here, at the tool boundary. The
    /// executor, the supervisor, and the whole process plane below it keep
    /// working in [`Duration`] and never see the model-facing unit. The
    /// mode-dependent default remains an execution-policy decision of the
    /// executor.
    pub(super) fn explicit_timeout(&self) -> Option<Duration> {
        self.timeout.map(Duration::from_secs)
    }
}
