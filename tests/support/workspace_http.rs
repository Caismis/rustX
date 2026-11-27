//! Shared private IPv4 selection for real-process Workspace HTTP MCP tests.
//!
//! Real external child processes (`rustx`) enforce the production dial policy
//! without any test-only DNS override, so cross-process suites bind a permitted
//! local private IPv4 address (which may be configured on `lo` inside an
//! isolated network namespace or overridden via `RUSTX_WORKSPACE_HTTP_FIXTURE_IP`).

use std::net::Ipv4Addr;

fn permitted_private_ipv4(ip: Ipv4Addr) -> bool {
    let [first, second, ..] = ip.octets();
    let cgnat = first == 100 && (64..=127).contains(&second);
    (ip.is_private() || cgnat)
        && !ip.is_loopback()
        && !ip.is_link_local()
        && !ip.is_unspecified()
        && first != 0
        && ip != Ipv4Addr::new(100, 100, 100, 200)
}

#[allow(dead_code)]
pub(crate) fn select_private_ipv4() -> Option<Ipv4Addr> {
    if let Ok(explicit) = std::env::var("RUSTX_WORKSPACE_HTTP_FIXTURE_IP")
        && let Ok(ip) = explicit.parse::<Ipv4Addr>()
        && permitted_private_ipv4(ip)
    {
        return Some(ip);
    }
    nix::ifaddrs::getifaddrs()
        .ok()?
        .filter_map(|interface| {
            interface.address.and_then(|address| {
                address
                    .as_sockaddr_in()
                    .map(nix::sys::socket::SockaddrIn::ip)
            })
        })
        .find(|ip| permitted_private_ipv4(*ip))
}

#[allow(dead_code)]
pub(crate) enum WorkspaceHttpFixtureIp {
    Ready(Ipv4Addr),
    DelegatedToIsolatedNetns,
}

#[allow(dead_code)]
pub(crate) fn ensure_private_ipv4_or_reexec(exact_test_name: &str) -> WorkspaceHttpFixtureIp {
    if let Some(ip) = select_private_ipv4() {
        return WorkspaceHttpFixtureIp::Ready(ip);
    }

    #[cfg(target_os = "linux")]
    if std::env::var_os("RUSTX_WORKSPACE_HTTP_REEXEC").is_none() {
        let exe = std::env::current_exe().expect("current test executable");
        let mut command = std::process::Command::new("unshare");
        command.args([
            "-Urn",
            "sh",
            "-c",
            "PATH=\"$PATH:/usr/sbin:/sbin\"; ip link set lo up && ip addr add 10.254.254.1/32 dev lo && RUSTX_WORKSPACE_HTTP_FIXTURE_IP=10.254.254.1 RUSTX_WORKSPACE_HTTP_REEXEC=1 exec \"$@\"",
            "--",
        ]);
        command
            .arg(exe)
            .args(["--exact", exact_test_name, "--test-threads=1"]);
        if std::env::args().any(|arg| arg == "--ignored") {
            command.arg("--ignored");
        }
        match command.output() {
            Ok(output) if output.status.success() => {
                return WorkspaceHttpFixtureIp::DelegatedToIsolatedNetns;
            }
            Ok(output) => {
                panic!(
                    "re-executing `{exact_test_name}` inside a disposable unshare -Urn fixture netns failed with {}:\nstdout:\n{}\nstderr:\n{}",
                    output.status,
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr),
                );
            }
            Err(error) => {
                panic!(
                    "a non-loopback private IPv4 interface or RUSTX_WORKSPACE_HTTP_FIXTURE_IP is required for real-process Workspace HTTP MCP tests, and launching `unshare -Urn` failed: {error}"
                );
            }
        }
    }

    panic!(
        "a non-loopback private IPv4 interface or RUSTX_WORKSPACE_HTTP_FIXTURE_IP is required for real-process Workspace HTTP MCP tests"
    );
}
