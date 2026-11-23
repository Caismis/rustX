//! The physical rustX process-supervisor package.
//!
//! This crate owns the dedicated supervisor helper binaries and their
//! shared supervised-process-unit core. The root `rustx` runtime drives
//! the units through `crate::invocation` (launch data plus the
//! executable-sibling boundary) and the runner-side wire readers; the
//! supervisor halves — the code that actually forks, signals, waits, and
//! answers the control protocol — live here without any dependency on the
//! root runtime.

#[cfg(unix)]
pub mod bash_supervisor;
#[cfg(not(unix))]
pub mod bash_supervisor {
    /// The supervisor role names remain available to the dedicated binary,
    /// which reports the unsupported platform explicitly at runtime.
    pub const ROLE_OUTER: &str = "outer";
    /// The supervisor role names remain available to the dedicated binary,
    /// which reports the unsupported platform explicitly at runtime.
    pub const ROLE_INNER: &str = "inner";

    /// Unix is required for the process-group ownership proof used by the
    /// Bash supervisor.
    pub fn run_outer_supervisor() -> ! {
        eprintln!("Bash supervisor requires Unix process supervision");
        std::process::exit(1);
    }

    /// Unix is required for the process-group ownership proof used by the
    /// Bash supervisor.
    pub fn run_inner_supervisor() -> ! {
        eprintln!("Bash supervisor requires Unix process supervision");
        std::process::exit(1);
    }
}
#[cfg(unix)]
pub mod interactive_process;
#[cfg(unix)]
pub mod interactive_supervisor;
#[cfg(not(unix))]
pub mod interactive_supervisor {
    /// The control socket environment variable used by the Unix supervisor.
    pub const RUSTX_CONTROL_ENV: &str = "RUSTX_INTERACTIVE_CONTROL";

    /// Non-Unix platforms cannot provide the supervisor's process-group
    /// ownership proof.
    pub fn run_outer(_arguments: &[String]) -> i32 {
        eprintln!("interactive supervisor requires Unix process supervision");
        1
    }

    /// Non-Unix platforms cannot provide the supervisor's process-group
    /// ownership proof.
    pub fn run_inner(_arguments: &[String]) -> i32 {
        eprintln!("interactive supervisor requires Unix process supervision");
        1
    }
}
#[cfg(target_os = "linux")]
pub mod client_domain;
pub mod effects;
pub mod identity;
pub mod invocation;
#[cfg(unix)]
pub mod nested_containment;
pub mod output_text;
#[cfg(unix)]
pub mod process_runner;
pub mod process_supervision;
#[cfg(unix)]
pub mod process_wait;
pub mod signal;
#[cfg(unix)]
pub mod supervised_unit;
