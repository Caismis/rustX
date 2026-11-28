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
const WORKSPACE_HTTP_PROOF_FILE_ENV: &str = "RUSTX_WORKSPACE_HTTP_PROOF_FILE";

#[allow(dead_code)]
struct ProofFileCleanup(std::path::PathBuf);

#[allow(dead_code)]
impl ProofFileCleanup {
    fn new(test_name: &str) -> Self {
        static NEXT_PROOF_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let sanitized: String = test_name
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
            .take(48)
            .collect();
        let seq = NEXT_PROOF_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "rustx-workspace-http-proof-{}-{seq}-{sanitized}.marker",
            std::process::id(),
        ));
        let _ = std::fs::remove_file(&path);
        Self(path)
    }
}

impl Drop for ProofFileCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[allow(dead_code)]
pub(crate) fn verify_reexec_proof(
    exact_test_name: &str,
    output: &std::process::Output,
    proof_path: &std::path::Path,
) -> Result<(), String> {
    if !output.status.success() {
        return Err(format!("child exited with {}", output.status));
    }
    let marker = std::fs::read_to_string(proof_path).map_err(|error| {
        format!(
            "target test `{exact_test_name}` did not write execution proof at {}: {error}",
            proof_path.display()
        )
    })?;
    if marker != exact_test_name {
        return Err(format!(
            "execution proof mismatch: expected `{exact_test_name}`, recorded `{marker}`"
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.contains("test result: ok. 1 passed;") {
        return Err(format!(
            "expected `test result: ok. 1 passed;` in child libtest output for `{exact_test_name}`"
        ));
    }
    Ok(())
}

#[allow(dead_code)]
pub(crate) fn ensure_private_ipv4_or_reexec(exact_test_name: &str) -> WorkspaceHttpFixtureIp {
    if let Some(ip) = select_private_ipv4() {
        if let Some(proof_file) = std::env::var_os(WORKSPACE_HTTP_PROOF_FILE_ENV) {
            std::fs::write(&proof_file, exact_test_name)
                .expect("write workspace HTTP fixture execution proof");
        }
        return WorkspaceHttpFixtureIp::Ready(ip);
    }

    #[cfg(target_os = "linux")]
    if std::env::var_os("RUSTX_WORKSPACE_HTTP_REEXEC").is_none() {
        let exe = std::env::current_exe().expect("current test executable");
        let proof_file = ProofFileCleanup::new(exact_test_name);
        let mut command = std::process::Command::new("unshare");
        command.args([
            "-Urn",
            "sh",
            "-c",
            "PATH=\"$PATH:/usr/sbin:/sbin\"; ip link set lo up && ip addr add 10.254.254.1/32 dev lo && RUSTX_WORKSPACE_HTTP_FIXTURE_IP=10.254.254.1 RUSTX_WORKSPACE_HTTP_REEXEC=1 exec \"$@\"",
            "--",
        ]);
        command
            .env(WORKSPACE_HTTP_PROOF_FILE_ENV, &proof_file.0)
            .arg(exe)
            .args(["--exact", exact_test_name, "--test-threads=1"]);
        if std::env::args().any(|arg| arg == "--ignored") {
            command.arg("--ignored");
        }
        match command.output() {
            Ok(output) => {
                if let Err(reason) = verify_reexec_proof(exact_test_name, &output, &proof_file.0) {
                    panic!(
                        "re-executing `{exact_test_name}` inside a disposable unshare -Urn fixture netns failed verification ({reason}):\nstdout:\n{}\nstderr:\n{}",
                        String::from_utf8_lossy(&output.stdout),
                        String::from_utf8_lossy(&output.stderr),
                    );
                }
                return WorkspaceHttpFixtureIp::DelegatedToIsolatedNetns;
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
