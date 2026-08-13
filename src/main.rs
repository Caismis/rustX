//! The spawnable `rustx` local conversation runtime process.
//!
//! The binary is a thin async bootstrap over
//! [`rustx::local_runtime`]: it parses the bounded startup arguments,
//! composes the one conversation runtime, and serves its Runtime Client
//! endpoint over the stdio/JSONL transport.
//!
//! stdout carries Runtime Client protocol records and nothing else — there
//! is no banner, no progress text, and no `println!` anywhere in the
//! process. Every diagnostic goes to stderr, and a startup configuration
//! failure exits non-zero having written zero bytes to stdout.

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
    let code = runtime.block_on(rustx::local_runtime::run_process(std::env::args().skip(1)));
    std::process::ExitCode::from(u8::try_from(code).unwrap_or(1))
}
