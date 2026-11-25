//! Native Server CLI and installed single-machine UI entry point.
//!
//! The bundled Web and TUI launchers retain their existing process ownership.
//! Other commands run natively. Stdout belongs to the selected protocol or
//! CLI report, including the internal `--subagent-child` process mode.

use std::{os::unix::process::CommandExt, process::ExitCode};

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    match args.as_slice() {
        [version] if version == "--version" => {
            println!("rustX {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        [mode, tail @ ..] if mode == "web" || mode == "tui" => {
            if let Err(error) = launch_ui(mode, tail) {
                eprintln!("rustx: {error}");
            }
            return ExitCode::from(2);
        }
        _ => {}
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("rustx: cannot start the async runtime: {error}");
            return ExitCode::from(2);
        }
    };
    let code = runtime.block_on(rustx::local_runtime::run_process(args));
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}

fn launch_ui(mode: &std::ffi::OsStr, args: &[std::ffi::OsString]) -> Result<(), String> {
    let binary = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .map_err(|error| format!("cannot resolve the installed rustx executable: {error}"))?;
    let prefix = binary
        .parent()
        .and_then(std::path::Path::parent)
        .ok_or("cannot determine the installation prefix")?;
    let launcher = prefix.join("share/rustx/dev/src/main.ts");
    if !launcher.is_file() {
        return Err(format!(
            "UI launcher unavailable at {}. Install the complete rustx bundle.",
            launcher.display()
        ));
    }
    let error = std::process::Command::new("node")
        .arg(launcher)
        .arg("--installed")
        .arg(mode)
        .arg("--binary")
        .arg(binary)
        .args(args)
        .exec();
    Err(format!(
        "cannot start the UI launcher with Node.js 24: {error}"
    ))
}
