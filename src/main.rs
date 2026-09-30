//! The rustX process entry point.
//!
//! `rustx app-server --listen stdio|ws://IP:PORT` composes the multi-Session
//! App Server, the only external product control protocol. The configuration
//! subcommands (`config`, `doctor`, `workflow`, `init`) are offline native
//! tools. A bare `rustx` prints help and starts nothing.
//!
//! Every diagnostic goes to stderr; there is no banner and no `println!`
//! anywhere in the process. App Server stdout carries protocol records only,
//! and configuration subcommands own stdout for their bounded reports.
//!
//! The internal `rustx --subagent-child` mode (Issue #60) is the one
//! deliberate exception: there fd 0 is the reliable subagent control IPC
//! and fd 1/stdout is the protocol-owned, framed Activity observation IPC
//! (Issue #178) — never human-readable output. Diagnostics still go to
//! stderr in every mode.

fn main() -> std::process::ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("rustx: cannot start the async runtime: {error}");
            return std::process::ExitCode::from(2);
        }
    };
    let code = runtime.block_on(rustx::local_runtime::run_process(
        std::env::args_os().skip(1),
    ));
    std::process::ExitCode::from(u8::try_from(code).unwrap_or(1))
}
