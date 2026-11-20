//! The physical invocation boundary between the root runtime and the
//! supervisor helpers.
//!
//! `ProgramInvocation` is the private launch data the runtime hands to a
//! spawned supervisor through `PROGRAM_INVOCATION_ENV`; it never comes
//! from the child environment. `companion_binary` locates the helper
//! executables (`bash-supervisor`, `interactive-supervisor`,
//! `workflow-renderer`) beside the running binary — the executable-sibling
//! contract the runtime and its tests stage against.

use std::path::PathBuf;

/// Private supervisor launch data. It never comes from the child environment.
/// The ordinary Bash path leaves this absent; standalone callers supply argv
/// without shell parsing and retain the same process-tree settlement owner.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct ProgramInvocation {
    /// Direct executable and arguments. Shell invocations carry no argv.
    pub argv: Option<Vec<String>>,
    pub stdin_socket: Option<PathBuf>,
    pub terminal: Option<PathBuf>,
    pub cwd_identity: (u64, u64),
}

/// The environment variable carrying the serialized [`ProgramInvocation`].
pub const PROGRAM_INVOCATION_ENV: &str = "RUSTX_SUPERVISOR_PROGRAM";

/// Locates a companion binary built from the same package (`bash-supervisor`,
/// `interactive-supervisor`, or `workflow-renderer`) beside the current
/// executable.
pub fn companion_binary(name: &str) -> PathBuf {
    std::env::current_exe()
        .expect("current executable")
        .parent()
        .expect("current executable directory")
        .join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}

/// Locates the `bash-supervisor` companion binary.
#[cfg(unix)]
pub fn supervisor_binary() -> PathBuf {
    companion_binary("bash-supervisor")
}

/// Locates the `interactive-supervisor` companion binary.
#[cfg(unix)]
pub fn interactive_supervisor_binary() -> PathBuf {
    companion_binary("interactive-supervisor")
}
