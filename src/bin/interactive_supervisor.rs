//! Small long-lived supervisor for MCP stdio servers.
//!
//! The protocol streams use the supervisor's stdin/stdout/stderr. Control
//! traffic deliberately uses a private Unix socket, so an MCP server never
//! shares its protocol stdin with rustX lifecycle messages.

#[cfg(unix)]
use std::os::unix::process::CommandExt;

#[cfg(unix)]
fn main() {
    let mut args = std::env::args().skip(1);
    let role = args.next().unwrap_or_default();
    let arguments: Vec<String> = args.collect();
    let result = match role.as_str() {
        "outer" => run_outer(arguments),
        "inner" => run_inner(&arguments),
        _ => Err("interactive supervisor role is missing".to_owned()),
    };
    std::process::exit(match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("interactive supervisor: {error}");
            1
        }
    });
}

#[cfg(not(unix))]
fn main() {
    eprintln!("interactive supervisor requires Unix process groups");
    std::process::exit(1);
}

#[cfg(unix)]
fn run_outer(arguments: Vec<String>) -> Result<i32, String> {
    let socket = std::env::var_os("RUSTX_INTERACTIVE_CONTROL")
        .ok_or_else(|| "control socket path is missing".to_owned())?;
    let inner_socket = std::env::temp_dir().join(format!(
        "rustx-interactive-inner-{}-{}.sock",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_nanos()
    ));
    let inner_listener = std::os::unix::net::UnixListener::bind(&inner_socket)
        .map_err(|error| format!("cannot bind inner control socket: {error}"))?;
    let mut control = std::os::unix::net::UnixStream::connect(socket)
        .map_err(|error| format!("cannot connect control socket: {error}"))?;
    control
        .set_nonblocking(true)
        .map_err(|error| format!("cannot configure control socket: {error}"))?;

    let current_exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let mut inner = std::process::Command::new(current_exe);
    inner.arg("inner").args(arguments);
    inner.env("RUSTX_INTERACTIVE_INNER_CONTROL", &inner_socket);
    inner
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit());
    let mut child = inner
        .spawn()
        .map_err(|error| format!("cannot spawn interactive process owner: {error}"))?;
    let (mut inner_control, _) = inner_listener
        .accept()
        .map_err(|error| format!("cannot accept inner control socket: {error}"))?;
    let _ = std::fs::remove_file(&inner_socket);
    inner_control
        .set_nonblocking(true)
        .map_err(|error| format!("cannot configure inner control socket: {error}"))?;
    let mut stopping = false;
    loop {
        if !stopping {
            let mut command = [0u8; 1];
            match std::io::Read::read(&mut control, &mut command) {
                Ok(1) if command[0] == 0x10 => {
                    stopping = true;
                    write_event(&mut inner_control, 0x10)?;
                }
                Ok(0) => {
                    stopping = true;
                    let _ = write_event(&mut inner_control, 0x10);
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(format!("control socket read failed: {error}")),
            }
        }
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("interactive child wait failed: {error}"))?
        {
            if stopping && status.success() {
                // The inner owner has already settled the server process
                // group before its own direct exit.
            }
            write_event(&mut control, 0x20)?;
            return Ok(status.code().unwrap_or(1));
        }
        if stopping {
            let mut event = [0u8; 1];
            match std::io::Read::read(&mut inner_control, &mut event) {
                Ok(1) if event[0] == 0x20 => {
                    let status = child
                        .wait()
                        .map_err(|error| format!("interactive child wait failed: {error}"))?;
                    write_event(&mut control, 0x20)?;
                    return Ok(status.code().unwrap_or(1));
                }
                Ok(0) => {
                    let status = child
                        .wait()
                        .map_err(|error| format!("interactive child wait failed: {error}"))?;
                    write_event(&mut control, 0x20)?;
                    return Ok(status.code().unwrap_or(1));
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(format!("inner control read failed: {error}")),
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[cfg(unix)]
#[allow(unsafe_code, clippy::items_after_statements)]
fn run_inner(arguments: &[String]) -> Result<i32, String> {
    nix::unistd::setsid().map_err(|error| format!("cannot establish process group: {error}"))?;
    become_child_subreaper()?;
    let inner_socket = std::env::var_os("RUSTX_INTERACTIVE_INNER_CONTROL")
        .ok_or_else(|| "inner control socket path is missing".to_owned())?;
    let mut control = std::os::unix::net::UnixStream::connect(inner_socket)
        .map_err(|error| format!("cannot connect inner control socket: {error}"))?;
    control
        .set_nonblocking(true)
        .map_err(|error| format!("cannot configure inner control socket: {error}"))?;
    let Some(program) = arguments.first() else {
        return Err("interactive program is missing".to_owned());
    };
    let mut command = std::process::Command::new(program);
    command.args(&arguments[1..]);
    command
        .env_remove("RUSTX_INTERACTIVE_CONTROL")
        .env_remove("RUSTX_INTERACTIVE_INNER_CONTROL");
    unsafe {
        command.pre_exec(|| {
            nix::unistd::setpgid(nix::unistd::Pid::from_raw(0), nix::unistd::Pid::from_raw(0))
                .map_err(std::io::Error::other)
        });
    }
    let mut child = command
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .map_err(|error| format!("cannot spawn interactive server: {error}"))?;
    let server_pgid = nix::unistd::Pid::from_raw(
        i32::try_from(child.id()).map_err(|_| "interactive pid does not fit i32".to_owned())?,
    );
    let mut stopping = false;
    let mut term_sent_at = None;
    let status = loop {
        if !stopping {
            let mut event = [0u8; 1];
            match std::io::Read::read(&mut control, &mut event) {
                Ok(1) if event[0] == 0x10 => {
                    signal_group(server_pgid, nix::sys::signal::Signal::SIGTERM)?;
                    stopping = true;
                    term_sent_at = Some(std::time::Instant::now());
                }
                Ok(0) => {
                    let _ = signal_group(server_pgid, nix::sys::signal::Signal::SIGTERM);
                    stopping = true;
                    term_sent_at = Some(std::time::Instant::now());
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(format!("inner control read failed: {error}")),
            }
        }
        if stopping
            && term_sent_at.is_some_and(|sent| sent.elapsed() >= std::time::Duration::from_secs(2))
        {
            let _ = signal_group(server_pgid, nix::sys::signal::Signal::SIGKILL);
            term_sent_at = None;
        }
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("interactive server wait failed: {error}"))?
        {
            break status;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    wait_for_group_terminal(server_pgid)?;
    // Reap descendants adopted by this supervisor before declaring the group
    // settled. There is no process-wide reaper in rustX; this loop owns only
    // the children of this dedicated process group.
    loop {
        match nix::sys::wait::waitpid(None, Some(nix::sys::wait::WaitPidFlag::WNOHANG)) {
            Ok(nix::sys::wait::WaitStatus::StillAlive) => {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Ok(_) | Err(nix::errno::Errno::EINTR) => {}
            Err(nix::errno::Errno::ECHILD) => break,
            Err(error) => return Err(format!("cannot reap interactive descendants: {error}")),
        }
    }
    write_event(&mut control, 0x20)?;
    Ok(status.code().unwrap_or(1))
}

#[cfg(target_os = "linux")]
fn become_child_subreaper() -> Result<(), String> {
    nix::sys::prctl::set_child_subreaper(true)
        .map_err(|error| format!("cannot establish child subreaper: {error}"))
}

#[cfg(not(target_os = "linux"))]
fn become_child_subreaper() -> Result<(), String> {
    Err("interactive MCP stdio supervision requires Linux child-subreaper support".to_owned())
}

#[cfg(target_os = "linux")]
fn wait_for_group_terminal(pgid: nix::unistd::Pid) -> Result<(), String> {
    let own_pid = std::process::id();
    loop {
        let mut other_member = false;
        for entry in std::fs::read_dir("/proc").map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let Ok(process_id) = name.parse::<u32>() else {
                continue;
            };
            if process_id == own_pid {
                continue;
            }
            let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
                continue;
            };
            let Some(end) = stat.rfind(") ") else {
                continue;
            };
            let fields: Vec<&str> = stat[end + 2..].split_whitespace().collect();
            if fields.get(2).and_then(|field| field.parse::<i32>().ok()) == Some(pgid.as_raw()) {
                other_member = true;
                break;
            }
        }
        if !other_member {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[cfg(not(target_os = "linux"))]
fn wait_for_group_terminal(_pgid: nix::unistd::Pid) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn signal_group(pgid: nix::unistd::Pid, signal: nix::sys::signal::Signal) -> Result<(), String> {
    match nix::sys::signal::killpg(pgid, signal) {
        Ok(()) | Err(nix::errno::Errno::ESRCH) => Ok(()),
        Err(error) => Err(format!("cannot signal interactive process group: {error}")),
    }
}

#[cfg(unix)]
fn write_event(stream: &mut std::os::unix::net::UnixStream, event: u8) -> Result<(), String> {
    std::io::Write::write_all(stream, &[event])
        .map_err(|error| format!("cannot write supervisor event: {error}"))
}
