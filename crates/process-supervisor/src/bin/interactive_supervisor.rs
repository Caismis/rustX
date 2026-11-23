//! The long-lived interactive supervisor binary for MCP stdio servers.
//!
//! Dispatched by argv role (`outer`/`inner`), mirroring the Bash
//! supervisor's `RUSTX_SUPERVISOR_ROLE` dispatch. The implementation lives
//! in the library (`rustx_process_supervisor::interactive_supervisor`) so both roles
//! share the structural ownership core with the M5 Bash supervisor unit.

fn main() {
    let exit = match std::env::args().nth(1).as_deref() {
        Some("outer") => {
            let arguments: Vec<String> = std::env::args().skip(2).collect();
            rustx_process_supervisor::interactive_supervisor::run_outer(&arguments)
        }
        Some("inner") => {
            let arguments: Vec<String> = std::env::args().skip(2).collect();
            rustx_process_supervisor::interactive_supervisor::run_inner(&arguments)
        }
        #[cfg(target_os = "linux")]
        Some("client-domain") => {
            let mut arguments = std::env::args_os().skip(2);
            match (arguments.next(), arguments.next()) {
                (Some(receipt), Some(program)) => {
                    match rustx_process_supervisor::client_domain::supervise(
                        std::path::Path::new(&receipt),
                        std::path::Path::new(&program),
                        arguments,
                    ) {
                        Ok(()) => 0,
                        Err(error) => {
                            eprintln!("Client execution domain: {error}");
                            1
                        }
                    }
                }
                _ => 1,
            }
        }
        _ => {
            eprintln!("interactive supervisor role is missing");
            1
        }
    };
    std::process::exit(exit);
}
