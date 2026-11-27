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
