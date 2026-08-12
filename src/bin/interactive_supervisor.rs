//! The long-lived interactive supervisor binary for MCP stdio servers.
//!
//! Dispatched by argv role (`outer`/`inner`), mirroring the Bash
//! supervisor's `RUSTX_SUPERVISOR_ROLE` dispatch. The implementation lives
//! in the library (`rustx::runtime::interactive_supervisor`) so both roles
//! share the structural ownership core with the M5 Bash supervisor unit.

fn main() {
    let exit = match std::env::args().nth(1).as_deref() {
        Some("outer") => {
            let arguments: Vec<String> = std::env::args().skip(2).collect();
            rustx::runtime::interactive_supervisor::run_outer(&arguments)
        }
        Some("inner") => {
            let arguments: Vec<String> = std::env::args().skip(2).collect();
            rustx::runtime::interactive_supervisor::run_inner(&arguments)
        }
        _ => {
            eprintln!("interactive supervisor role is missing");
            1
        }
    };
    std::process::exit(exit);
}
