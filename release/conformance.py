#!/usr/bin/env python3
"""Verify and exercise the extracted rustX products outside the source tree."""

import argparse
import fcntl
import hashlib
from http.client import HTTPConnection
import json
import os
from pathlib import Path
import pty
import re
import selectors
import shutil
import signal
import struct
import subprocess
import tarfile
import tempfile
import termios
import time
from urllib.parse import urlsplit


ROOT = Path(__file__).resolve().parent.parent

BINARIES = {
    "rustx-server": {"rustx-server", "workflow-renderer"},
    "rustx-client": {"rustx-client", "bash-supervisor", "interactive-supervisor"},
    "rustx": {"rustx", "rustx-execution-client", "workflow-renderer", "bash-supervisor", "interactive-supervisor"},
}
PROCESS_CASES = (
    "standalone_server_reads_cold_sessions_without_workspace_or_helpers",
    "standalone_server_commits_a_model_turn_without_a_client_or_helpers",
    "standalone_server_reports_mcp_overflow_without_launching_a_client_or_spilling",
    "near_limit_http_mcp_body_commits_only_a_bounded_preview_without_a_client",
    "http_mcp_imports_complete_output_through_the_bound_client",
    "execution_enrollment_precedes_client_and_workspace_configuration_readiness",
    "local_execution_fails_exactly_when_the_sibling_client_is_absent",
    "remote_execution_serves_session_reads_over_websocket",
    "explicit_composition_launches_and_reaps_the_sibling_client",
    "websocket_and_unix_transports_serve_identical_seeded_operations",
)
HOST_CASES = (
    "standalone_server_validates_large_host_stream_without_a_client",
    "host_effect_is_unknown_after_process_death_and_is_never_automatically_replayed",
)
LINUX_HOST_CASES = (
    "generation::a_published_host_output_cannot_cross_a_local_client_generation",
    "generation::host_result_after_client_death_at_private_save_cuts",
    "generation::client_death_after_canonical_host_result_cannot_rewrite_saved_history",
    "generation::process_cuts::client_death_at_wire_and_companion_boundaries_never_replays_start",
)
LINUX_CASES = (
    "local_restart_requires_namespace_settlement_after_server_sigkill",
    "client_sigkill_advances_only_the_original_session_generation",
    "server_sigkill_keeps_prior_client_binding_unavailable_after_restart",
)
MOUNT_CASES = (
    "explicit_composition_reads_from_client_mount_namespace",
    "managed_python_prepares_and_runs_in_the_bound_client_mount",
    "stdio_mcp_session_runs_and_publishes_from_the_client_mount_view",
)


def digest(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def require(condition: bool, failure: str) -> None:
    if not condition:
        raise ValueError(failure)


def extract(archive: Path, products: Path) -> dict:
    checksum = archive.with_name(archive.name + ".sha256").read_text().split()
    require(checksum == [digest(archive), archive.name], f"Invalid archive checksum: {archive}")
    stem = archive.name.removesuffix(".tar.gz")
    with tarfile.open(archive, "r:gz") as contents:
        require(all(Path(entry.name).parts[0] == stem for entry in contents), "Expected one archive prefix")
        contents.extractall(products, filter="data")
    directory = products / stem
    manifest = json.loads((directory / "manifest.json").read_text())
    product = manifest["product"]
    require(product in BINARIES, f"Unknown product: {product}")
    for name, entry in manifest["files"].items():
        member = directory / name
        require(member.resolve().is_relative_to(directory), f"File outside archive prefix: {name}")
        if "link" in entry:
            require(member.is_symlink() and os.readlink(member) == entry["link"] and member.exists(),
                    f"Invalid installed link: {name}")
        else:
            require(member.is_file() and not member.is_symlink(), f"Missing regular file: {name}")
            require(digest(member) == entry["sha256"] and member.stat().st_size == entry["bytes"],
                    f"Invalid installed contents: {name}")
            require(member.stat().st_mode & 0o777 == int(entry["mode"], 8), f"Invalid installed mode: {name}")
    expected = {"manifest.json", *manifest["files"]}
    actual = {path.relative_to(directory).as_posix() for path in directory.rglob("*")
              if path.is_symlink() or path.is_file()}
    require(actual == expected, "Manifest does not enumerate the complete installation")
    require({path.name for path in (directory / "bin").iterdir()} == BINARIES[product],
            f"Unexpected executables for {product}")
    directory.rename(products / product)
    return manifest


def cli(binary: Path, args: list[str], cwd: Path, environment: dict) -> str:
    result = subprocess.run([binary, *args], cwd=cwd, env=environment,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=30)
    require(result.returncode == 0, f"{binary.name} {args[0]} failed:\n{result.stdout}")
    return result.stdout


def tui(binary: Path, cwd: Path, environment: dict, bootstrap: dict, workspace: Path) -> None:
    with open(cwd / "tui-token", "x", opener=lambda path, flags: os.open(path, flags, 0o600)) as token:
        token.write(bootstrap["appServerTransportToken"])
    try:
        example = binary.parent.parent / "share/examples/app-server/connect.mjs"
        info = json.loads(cli(Path(shutil.which("node") or "node"),
                             [str(example), bootstrap["appServerEndpoint"], str(cwd / "tui-token")],
                             cwd, environment))
        require(isinstance(info.get("capabilities"), dict), "Installed third-party example did not obtain Server info")
        tui_terminal(binary, cwd, environment, bootstrap, workspace)
    finally:
        (cwd / "tui-token").unlink()


def tui_terminal(binary: Path, cwd: Path, environment: dict, bootstrap: dict, workspace: Path) -> None:
    master, slave = pty.openpty()
    try:
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 32, 120, 0, 0))
        with subprocess.Popen([binary, "tui", "--connect", bootstrap["appServerEndpoint"],
                               "--token-file", cwd / "tui-token", "--workspace", workspace, "--resume"],
                              cwd=cwd, env={**environment, "TERM": "xterm-256color"}, stdin=slave,
                              stdout=slave, stderr=slave, start_new_session=True) as terminal:
            try:
                screen = bytearray()
                deadline = time.monotonic() + 30
                with selectors.DefaultSelector() as selector:
                    selector.register(master, selectors.EVENT_READ)
                    while time.monotonic() < deadline and terminal.poll() is None:
                        if not selector.select(min(1, max(0, deadline - time.monotonic()))):
                            continue
                        screen.extend(os.read(master, 8192))
                        if b"Resume session" in screen:
                            break
                        require(len(screen) <= 128 * 1024, "Installed TUI emitted excessive startup output")
                require(b"Resume session" in screen, "Installed TUI did not render the real Session catalog")
                terminal.terminate()
                require(terminal.wait(timeout=20) == 143, "Unexpected TUI owner shutdown status")
            finally:
                if terminal.poll() is None:
                    os.killpg(terminal.pid, signal.SIGKILL)
                    terminal.wait()
    finally:
        os.close(slave)
        os.close(master)


def web(binary: Path, cwd: Path, environment: dict) -> None:
    # Keep the Web owner separate so a failed assertion cannot leave processes behind.
    workspace = cwd / "workspace"
    workspace.mkdir()
    native = cwd / "native"
    cli(binary, ["init", "--template", "openai-chat", "--provider", "local", "--model-id", "test",
                 "--endpoint", "http://127.0.0.1:9/v1", "--credential-env", "TEST_KEY",
                 "--context-window", "128000", "--max-output", "4096",
                 "--compat", 'chat_reasoning_replay = "omit"'], cwd, environment)
    config = Path(environment["HOME"]) / "rustx/rustx.toml"
    with config.open("a") as source:
        source.write(f"\n[app_server]\nenvironment_roots = [{json.dumps(str(workspace))}]\n")
    started = time.monotonic()
    with (cwd / "web.log").open("w+") as errors:
        launcher = subprocess.Popen([binary, "web", "--workspace", workspace, "--no-open",
                                     "--runtime-root", native, "--local-execution-workspace", workspace],
                                    cwd=cwd, env=environment, stdin=subprocess.DEVNULL,
                                    stdout=subprocess.PIPE, stderr=errors, start_new_session=True, bufsize=0)
        try:
            assert launcher.stdout is not None
            startup = b""
            with selectors.DefaultSelector() as selector:
                selector.register(launcher.stdout, selectors.EVENT_READ)
                deadline = started + 45
                while time.monotonic() < deadline:
                    if not selector.select(max(0, deadline - time.monotonic())):
                        break
                    line = launcher.stdout.readline()
                    if b"rustX Web: " in line:
                        startup = line.rpartition(b"rustX Web: ")[2].strip()
                        break
                    if not line:
                        break
            if not startup:
                errors.seek(0)
                raise RuntimeError(f"Packaged Web did not start\n{errors.read()}")
            url = urlsplit(startup.decode())
            connection = HTTPConnection(url.hostname, url.port, timeout=5)

            def get(path: str, status: int, headers: dict | None = None) -> bytes:
                connection.request("GET", path, headers=headers or {})
                response = connection.getresponse()
                require(response.status == status, f"Unexpected Web HTTP status for {path.split('?')[0]}")
                return response.read()

            try:
                get("/__rustx/bootstrap", 401)
                landing = get(url.path + "?" + url.query, 200)
                proof = re.search(rb'setItem\("rustx-browser-session","([A-Za-z0-9_-]{43})"\)', landing)
                require(proof is not None, "Browser handoff did not issue a separated browser session")
                bearer = {"X-Rustx-Browser-Session": proof[1].decode()}
                bootstrap = json.loads(get("/__rustx/bootstrap", 200, bearer))
                require(bootstrap["appServerEndpoint"].startswith("ws://127.0.0.1:"), "Unexpected App Server endpoint")
                require(bootstrap["appServerTransportToken"].encode() != proof[1], "Browser and transport credentials overlap")
                index = get("/", 200)
                asset = re.search(rb'src="(/assets/[^\"]+\.js)"', index)
                require(asset is not None, "Installed Web index has no JS entry")
                require(bool(get(asset[1].decode(), 200)), "Installed Web JS entry is empty")
                get("/assets/no-such-artifact.js", 404)
                connection.request("POST", "/product-host/list", body="{}", headers={
                    **bearer, "Content-Type": "application/json",
                })
                response = connection.getresponse()
                catalog = json.loads(response.read())
                require(response.status == 200 and catalog["picker"]["kind"] == "configured",
                        "Installed Host must offer only explicitly configured locations")
            finally:
                connection.close()
            print(f"Web bootstrap and static assets ready in {(time.monotonic() - started) * 1000:.1f} ms", flush=True)
            tui(binary, cwd, environment, bootstrap, workspace)
            print("Installed TUI rendered the enrolled Session catalog and disconnected", flush=True)
            require(launcher.poll() is None, "TUI disconnect shut down the Web owner")
            launcher.terminate()
            require(launcher.wait(timeout=30) == 143, "Unexpected Web owner shutdown status")
        finally:
            if launcher.poll() is None:
                os.killpg(launcher.pid, signal.SIGKILL)
                launcher.wait()


def process_proof(harness: Path, products: Path, mount: bool) -> None:
    environment = {**os.environ, "RUSTX_INSTALLED_PRODUCTS": str(products), "RUSTX_REQUIRE_PROVIDER_EMULATOR": "1"}
    cases = PROCESS_CASES + (LINUX_CASES if os.uname().sysname == "Linux" else ())
    if mount:
        require(os.uname().sysname == "Linux", "The separate mount fixture requires Linux")
        environment["RUSTX_MOUNT_VIEW_PROOF"] = "1"
        cases += MOUNT_CASES
    # The harness reads repository Fake Provider responses and test fixtures.
    # Its child processes use isolated runtime and Workspace directories.
    tests = [f"app_server::{test}" for test in cases]
    host_cases = HOST_CASES + (LINUX_HOST_CASES if os.uname().sysname == "Linux" else ())
    tests.extend(f"app_server_host::{test}" for test in host_cases)
    for test in tests:
        run = subprocess.run([harness, test, "--exact", "--include-ignored"], cwd=ROOT,
                             env=environment, timeout=180, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        print(run.stdout, end="", flush=True)
        require(run.returncode == 0 and "1 passed; 0 failed;" in run.stdout, f"Installed conformance case did not pass: {test}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archives", type=Path, nargs=3)
    parser.add_argument("--process-harness", type=Path)
    parser.add_argument("--mount-proof", action="store_true")
    options = parser.parse_args()
    if options.mount_proof and not options.process_harness:
        parser.error("--mount-proof requires --process-harness")
    with tempfile.TemporaryDirectory(prefix="rustx installed ") as folder:
        directory = Path(folder).resolve()
        products = directory / "products"
        products.mkdir()
        manifests = [extract(archive.resolve(), products) for archive in options.archives]
        require({manifest["product"] for manifest in manifests} == BINARIES.keys(), "One archive per product is required")
        for field in ("source", "version", "platform", "execution_protocol"):
            require(len({manifest[field] for manifest in manifests}) == 1, f"Products disagree on {field}")
        cwd = directory / "outside source"
        cwd.mkdir()
        home = cwd / "home"
        home.mkdir()
        # Standalone help must run with no helpers, Python, Node, or Cargo on PATH.
        environment = {"HOME": str(home), "PATH": str(cwd), "LANG": "C"}
        server = products / "rustx-server/bin/rustx-server"
        client = products / "rustx-client/bin/rustx-client"
        bundle = products / "rustx/bin/rustx"
        require("--execution-bootstrap-out" in cli(server, ["app-server", "--help"], cwd, environment),
                "Server bootstrap help is incomplete")
        require("--binding-file" in cli(client, ["--help"], cwd, environment), "Client bootstrap help is incomplete")
        expected_client_version = (f"rustX Client {manifests[0]['version']} "
                                   f"(Execution Protocol v{manifests[0]['execution_protocol']})")
        require(cli(client, ["--version"], cwd, environment).strip() == expected_client_version,
                "Client release or protocol revision disagrees with manifest")
        require(cli(server, ["--version"], cwd, environment).strip() == f"rustX {manifests[0]['version']}",
                "Server release version disagrees with manifest")
        require(shutil.which("node") is not None, "Node.js 24 is required for installed Web and TUI proofs")
        environment["PATH"] = os.environ["PATH"]
        environment["TEST_KEY"] = "fake-provider-unused"
        require("--local-execution-workspace" in cli(bundle, ["tui", "--help"], cwd, environment),
                "Installed TUI help is incomplete")
        web(bundle, cwd, environment)
        if options.process_harness:
            process_proof(options.process_harness.resolve(), products, options.mount_proof)
        print(f"Installed {manifests[0]['source']} ({manifests[0]['platform']}); manifest and launcher checks passed")


if __name__ == "__main__":
    main()
