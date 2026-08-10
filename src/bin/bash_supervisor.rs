//! The dedicated per-invocation Bash supervisor binary.
//!
//! One process per Bash invocation, dispatched by role through the
//! `RUSTX_SUPERVISOR_ROLE` environment variable. The binary writes nothing
//! to stdout/stderr — those descriptors are the invocation's output capture
//! pipes — so the captured streams contain exactly the bash output.

fn main() {
    let exit = match std::env::var("RUSTX_SUPERVISOR_ROLE").as_deref() {
        Ok(rustx::tools::native::bash_supervisor::ROLE_OUTER) => {
            rustx::tools::native::bash_supervisor::run_outer_supervisor()
        }
        Ok(rustx::tools::native::bash_supervisor::ROLE_INNER) => {
            rustx::tools::native::bash_supervisor::run_inner_supervisor()
        }
        _ => 1,
    };
    std::process::exit(exit);
}
