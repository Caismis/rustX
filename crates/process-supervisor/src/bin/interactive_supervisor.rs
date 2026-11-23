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
        Some(role @ ("client-domain" | "client-domain-init")) => {
            let mut arguments = std::env::args_os().skip(2);
            use rustx_process_supervisor::client_domain;
            let outcome = if role == "client-domain" {
                match (arguments.next(), arguments.next()) {
                    (Some(receipt), Some(program)) => {
                        client_domain::supervise(receipt.as_ref(), program.as_ref(), arguments)
                    }
                    _ => std::process::exit(1),
                }
            } else {
                let Some(program) = arguments.next() else {
                    std::process::exit(1)
                };
                client_domain::launch_init(program.as_ref(), arguments)
            };
            match outcome {
                Ok(()) => 0,
                Err(error) => {
                    eprintln!("Client execution domain: {error}");
                    1
                }
            }
        }
        _ => {
            eprintln!("interactive supervisor role is missing");
            1
        }
    };
    std::process::exit(exit);
}
