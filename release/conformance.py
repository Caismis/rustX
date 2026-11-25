#!/usr/bin/env python3
"""Verify and exercise the extracted rustX products outside the source tree."""

import argparse
import errno
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
import socket
import struct
import subprocess
import tarfile
import tempfile
import termios
import time
from urllib.parse import urlsplit

from installed_provider import installed_provider


ROOT = Path(__file__).resolve().parent.parent

BINARIES = {
    "rustx-server": {"rustx-server", "workflow-renderer"},
    "rustx-client": {"rustx-client", "bash-supervisor", "interactive-supervisor"},
    "rustx": {"rustx", "rustx-execution-client", "workflow-renderer", "bash-supervisor", "interactive-supervisor"},
}
PYTHON_GUARD = {"python", "python3"}
PROCESS_CASES = (
    "standalone_server_reads_cold_sessions_without_workspace_or_helpers",
    "app_server_concurrent_sessions_finish_across_external_disconnect",
    "t10_stdio_cli_eof_preserves_gated_attempt_and_original_canonical_commit",
    "app_server_websocket_drain_supervises_active_root_and_cold_resume",
    "standalone_server_commits_a_model_turn_without_a_client_or_helpers",
    "app_server_archive_stdio_download_works_without_web_or_runtime_attachment",
    "standalone_server_reports_mcp_overflow_without_launching_a_client_or_spilling",
    "near_limit_http_mcp_body_commits_only_a_bounded_preview_without_a_client",
    "http_mcp_imports_complete_output_through_the_bound_client",
    "execution_enrollment_precedes_client_and_workspace_configuration_readiness",
    "installed_workspace_picker_uses_a_private_client_and_reaps_before_registration",
    "local_execution_fails_exactly_when_the_sibling_client_is_absent",
    "remote_execution_serves_session_reads_over_websocket",
    "explicit_composition_launches_and_reaps_the_sibling_client",
    "websocket_and_unix_transports_serve_identical_seeded_operations",
    "server_sigkill_keeps_prior_client_binding_unavailable_after_restart",
    "doctor_probes_selected_managed_python_in_an_owned_client",
)
HOST_CASES = (
    "standalone_server_validates_large_host_stream_without_a_client",
    "standalone_server_arbitrates_two_streaming_host_candidates_without_a_client",
    "host_effect_is_unknown_after_process_death_and_is_never_automatically_replayed",
)
LINUX_HOST_CASES = (
    "generation::deleted_session_recovers_only_for_cleanup_in_the_original_client",
    "generation::a_published_host_output_cannot_cross_a_local_client_generation",
    "generation::host_result_after_client_death_at_private_save_cuts",
    "generation::client_death_after_canonical_host_result_cannot_rewrite_saved_history",
    "generation::process_cuts::client_death_at_wire_and_companion_boundaries_never_replays_start",
    "generation::installed_detach::installed_ui_detach_cancels_the_client_with_both_lanes_occupied",
    "generation::installed_file_tools::installed_native_file_tools_keep_large_results_on_the_client",
    "generation::installed_file_tools::installed_bash_continuation_reads_only_the_selected_client_output",
)
CLIENT_CASES = (
    "same_instance_recovery_preserves_the_process_and_output_cursor",
    "real_client_recovers_after_losing_its_revoke_receipt",
    "either_one_way_control_failure_recovers_the_same_authenticated_instance",
)
LINUX_CASES = (
    "local_restart_requires_namespace_settlement_after_server_sigkill",
    "client_sigkill_advances_only_the_original_session_generation",
    "trusted_remote_host_receipt_allows_a_new_server_to_admit_the_session",
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
    helpers = {"python-guard"} if product != "rustx-server" else set()
    require({path.name for path in (directory / "bin").iterdir()} == BINARIES[product] | helpers,
            f"Unexpected bin entries for {product}")
    if helpers:
        guard = directory / "bin/python-guard"
        require(guard.is_dir() and not guard.is_symlink()
                and {script.name for script in guard.iterdir()} == PYTHON_GUARD,
                f"Incomplete Python guard for {product}")
    directory.rename(products / product)
    return manifest


def cli(binary: Path, args: list[str], cwd: Path, environment: dict) -> str:
    result = subprocess.run([binary, *args], cwd=cwd, env=environment,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=30)
    require(result.returncode == 0, f"{binary.name} {args[0]} failed:\n{result.stdout}")
    return result.stdout


def python_guard(product_binary: Path, cwd: Path, environment: dict) -> None:
    diagnostic = "Installed Workspace Python is unavailable"
    for name in sorted(PYTHON_GUARD):
        script = product_binary.parent / "python-guard" / name
        result = subprocess.run([script], cwd=cwd, timeout=5, capture_output=True, text=True,
                                env={**environment, "RUSTX_PYTHON_GUARD_MESSAGE": diagnostic})
        require(result.returncode == 127 and not result.stdout and result.stderr.strip() == diagnostic,
                f"Installed Python guard failed: {script}")


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


def stop_terminal(terminal: subprocess.Popen, master: int, timeout: float) -> bytes:
    terminal.terminate()
    deadline = time.monotonic() + timeout
    tail = bytearray()
    # Keep receiving terminal output until the owner exits. A real terminal
    # continues consuming display writes while the TUI drains its Server.
    with selectors.DefaultSelector() as selector:
        selector.register(master, selectors.EVENT_READ)
        while True:
            status = terminal.poll()
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                if status is None:
                    raise TimeoutError(f"Installed TUI did not exit: {bytes(tail)!r}")
                break
            if not selector.select(min(remaining, 0.1) if status is None else 0):
                if status is not None:
                    break
                continue
            try:
                chunk = os.read(master, 8192)
            except OSError as error:
                if error.errno != errno.EIO:
                    raise
                break
            if not chunk:
                break
            tail.extend(chunk)
            del tail[:-8192]
    try:
        status = terminal.wait(timeout=max(0, deadline - time.monotonic()))
    except subprocess.TimeoutExpired as error:
        raise TimeoutError(f"Installed TUI did not exit: {bytes(tail)!r}") from error
    require(status == 143, f"Unexpected TUI owner shutdown status {status}: {bytes(tail)!r}")
    return bytes(tail)


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
                        if b"Resume session" in screen or b"Enter Send" in screen:
                            break
                        require(len(screen) <= 128 * 1024, "Installed TUI emitted excessive startup output")
                require(b"Resume session" in screen or b"Enter Send" in screen,
                        "Installed TUI did not connect to the running Server")
                stop_terminal(terminal, master, timeout=20)
            finally:
                if terminal.poll() is None:
                    os.killpg(terminal.pid, signal.SIGKILL)
                    terminal.wait()
    finally:
        os.close(slave)
        os.close(master)


def configure(binary: Path, cwd: Path, environment: dict, endpoint: str,
              grant: Path | None = None) -> Path:
    cli(binary, ["init", "--template", "openai-chat", "--provider", "local", "--model-id", "test",
                 "--endpoint", endpoint, "--credential-env", "TEST_KEY",
                 "--context-window", "128000", "--max-output", "4096",
                 "--compat", 'chat_reasoning_replay = "omit"'], cwd, environment)
    defaults = Path(environment["HOME"]) / "rustx/rustx.toml"
    config = cwd / "config.toml"
    authored = 'approval_mode = "full_access"\n' + defaults.read_text()
    authored += '\n[agent.tools]\nbuiltin = ["read", "bash", "write"]\n'
    if grant:
        authored += f"\n[app_server]\nenvironment_roots = [{json.dumps(str(grant))}]\n"
    config.write_text(authored)
    return config


def tui_create(binary: Path, cwd: Path, environment: dict, endpoint: str) -> None:
    workspace = cwd / "workspace"
    workspace.mkdir()
    (workspace / "seed.txt").write_text("seed-terminal-session\n")
    native = cwd / "native"
    home = cwd / "home"
    home.mkdir()
    environment = {**environment, "HOME": str(home), "TERM": "xterm-256color"}
    config = configure(binary, cwd, environment, endpoint)
    inspection = ["config", "show", "--sources", "--json", "--config", str(config), "--workspace", str(workspace)]
    for arguments in (inspection, ["workflow", "--help"]):
        native_command = subprocess.run([binary, *arguments], cwd=cwd, env=environment,
                                        capture_output=True, timeout=30)
        if arguments == inspection:
            report = json.loads(native_command.stdout)
            require(report["operation"] == "config_show" and report["validity"] == "valid",
                    "Native config inspection did not produce the expected report")
        else:
            require(native_command.returncode == 0, "Native Workflow help failed")
        forwarded = subprocess.run([binary, "tui", *arguments], cwd=cwd, env=environment,
                                   capture_output=True, timeout=30)
        require((forwarded.returncode, forwarded.stdout, forwarded.stderr)
                == (native_command.returncode, native_command.stdout, native_command.stderr),
                "Installed TUI changed the native command output or exit code")
    master, slave = pty.openpty()
    try:
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 132, 0, 0))
        with subprocess.Popen([binary, "tui", "--workspace", workspace, "--config", config,
                               "--runtime-root", native], cwd=cwd, env=environment, stdin=slave,
                              stdout=slave, stderr=slave, start_new_session=True) as terminal:
            def observe(expected: bytes) -> None:
                screen = bytearray()
                deadline = time.monotonic() + 30
                with selectors.DefaultSelector() as selector:
                    selector.register(master, selectors.EVENT_READ)
                    while terminal.poll() is None and selector.select(max(0, deadline - time.monotonic())):
                        screen.extend(os.read(master, 8192))
                        if expected in screen:
                            return
                        require(len(screen) <= 256 * 1024, "Installed TUI emitted excessive output")
                raise RuntimeError(f"Installed TUI did not render {expected!r}: {bytes(screen[-8192:])!r}")

            def bindings():
                return {state["session"]: state for path in (native / "execution-pending").glob("*.json")
                        if (state := json.loads(path.read_text()))["workspace"] == str(workspace)}

            try:
                observe(b"Enter Send")
                first = bindings()
                require(len(first) == 1, "TUI startup has no exact Client")
                os.write(master, b"/new\r")
                observe(b"created session")
                second = bindings()
                require(len(second) == 2 and second.keys() > first.keys(), "TUI /new reused the startup binding")
                require(len({state["instance"] for state in second.values()}) == 2,
                        "TUI Sessions share an execution Client instance")
                os.write(master, b"exercise terminal-session\r")
                observe(b"Installed tools complete for terminal-session")
                require((workspace / "written.txt").read_text() == "write-terminal-session\n" and
                        (workspace / "bash-cwd.txt").read_text().strip() == str(workspace),
                        "Installed TUI tools used the wrong Workspace")
                tail = stop_terminal(terminal, master, timeout=30)
                remaining = bindings()
                require(not remaining,
                        f"Installed TUI left Session execution unsettled {sorted(remaining)}: {tail!r}")
            finally:
                if terminal.poll() is None:
                    os.killpg(terminal.pid, signal.SIGTERM)
                    try:
                        terminal.wait(timeout=30)
                    except subprocess.TimeoutExpired:
                        os.killpg(terminal.pid, signal.SIGKILL)
                        terminal.wait()
    finally:
        os.close(slave)
        os.close(master)


def web(binary: Path, cwd: Path, environment: dict, endpoint: str,
        *, browser: bool, grants: bool = True, resume: bool = False) -> None:
    # Keep the Web owner separate so a failed assertion cannot leave processes behind.
    workspace = cwd / "workspace"
    home = cwd / "home"
    if not resume:
        workspace.mkdir()
        (workspace / "project").mkdir()
        (workspace / "seed.txt").write_text("seed-web-root\n")
        (workspace / "project/seed.txt").write_text("seed-web-child\n")
        skill = workspace / ".agents/skills/installed-guide"
        skill.mkdir(parents=True)
        (skill / "SKILL.md").write_text(
            "---\nname: installed-guide\ndescription: Installed Workspace guide.\n---\nPrivate Client catalog marker.\n")
        home.mkdir()
    native = cwd / "native"
    environment = {**environment, "HOME": str(home),
                   "PLAYWRIGHT_BROWSERS_PATH": os.environ.get(
                       "PLAYWRIGHT_BROWSERS_PATH", str(Path.home() / ("Library/Caches/ms-playwright"
                       if os.uname().sysname == "Darwin" else ".cache/ms-playwright")))}
    if key := os.environ.get("RUSTX_INSTALLED_BROWSER_EXECUTABLE"):
        environment["RUSTX_INSTALLED_BROWSER_EXECUTABLE"] = key
    config = cwd / "config.toml" if resume else configure(binary, cwd, environment, endpoint, workspace if grants else None)
    started = time.monotonic()
    with (cwd / "web.log").open("w+") as errors:
        launcher = subprocess.Popen([binary, "web", "--workspace", workspace, "--no-open",
                                     "--runtime-root", native, "--config", config],
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
                with socket.create_connection((url.hostname, url.port), timeout=5) as malformed:
                    malformed.sendall(f"GET // HTTP/1.1\r\nHost: {url.netloc}\r\nConnection: close\r\n\r\n".encode())
                    with malformed.makefile("rb") as raw:
                        require(raw.readline().startswith(b"HTTP/1.1 400 "), "Installed carrier accepted an invalid request target")
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
                def post(method: str, body: dict, status: int = 200):
                    connection.request("POST", f"/product-host/{method}", body=json.dumps(body), headers={
                        **bearer, "Content-Type": "application/json",
                    })
                    response = connection.getresponse()
                    payload = response.read()
                    require(response.status == status, f"Unexpected Web status for {method}: {payload!r}")
                    return json.loads(payload)

                catalog = post("list", {})
                require(catalog["picker"]["kind"] == "browser", "Installed Host must expose the bound Workspace picker")
                [location] = catalog["picker"]["locations"]
                scope = {key: catalog[key] for key in ("authorityId", "endpoint")}
                coordinates = {"scope": scope, "location": location["id"]}
                post("browse-workspace", {**coordinates, "path": "../"}, 400)
                if grants:
                    root = post("browse-workspace", {**coordinates, "path": ""})
                    require("project" in root["folders"], "Installed picker lost the directory")
                    selected = post("browse-workspace", {**coordinates, "path": "project"})
                    if not resume:
                        require(selected["folders"] == [], "Empty selection returned extra entries")
                    if not browser:
                        identifier = post("register-workspace-folder", {**coordinates, "selection": {
                            "path": "project", "revision": selected["revision"],
                        }})
                        registered = post("list", {})
                        require(any(row["id"] == identifier and Path(row["displayPath"]) == workspace / "project"
                                    for row in registered["workspaces"]), "Installed picker did not register the confirmed directory")
                else:
                    post("browse-workspace", {**coordinates, "path": "project"}, 400)
            finally:
                connection.close()
            print(f"Web bootstrap and static assets ready in {(time.monotonic() - started) * 1000:.1f} ms", flush=True)
            if browser:
                run = subprocess.run([shutil.which("node"), ROOT / "web-console/test/installed-composition.ts",
                                      startup.decode(), native, workspace,
                                      "resumed" if resume else "granted" if grants else "exact-only"],
                                     cwd=cwd, env=environment, timeout=180)
                require(run.returncode == 0, "Installed Web browser proof failed")
                print("Installed Web browser and Workspace ownership verified", flush=True)
            # Separate native connection proves that the invalid request did
            # not terminate the Server owned by this still-running carrier.
            tui(binary, cwd, environment, bootstrap, workspace)
            print("Installed TUI rendered the Session catalog and disconnected", flush=True)
            require(launcher.poll() is None, "TUI disconnect shut down the Web owner")
            launcher.terminate()
            require(launcher.wait(timeout=30) == 143, "Unexpected Web owner shutdown status")
        finally:
            if launcher.poll() is None:
                # The launcher owns detached Server and carrier groups. Let it
                # drain them even if a smoke assertion fails before shutdown.
                launcher.terminate()
                try:
                    launcher.wait(timeout=30)
                except subprocess.TimeoutExpired:
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
        run_case(harness, test, environment)


def run_case(harness: Path, name: str, environment: dict) -> None:
    run = subprocess.run([harness, name, "--exact", "--include-ignored"], cwd=ROOT,
                         env=environment, timeout=180, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    print(run.stdout, end="", flush=True)
    require(run.returncode == 0 and "1 passed; 0 failed;" in run.stdout, f"Installed conformance case did not pass: {name}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archives", type=Path, nargs=3)
    parser.add_argument("--process-harness", type=Path)
    parser.add_argument("--client-harness", type=Path)
    parser.add_argument("--mount-proof", action="store_true")
    parser.add_argument("--browser-proof", action="store_true", help="exercise the extracted Web in Chromium")
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
        python_guard(client, cwd, environment)
        python_guard(bundle, cwd, environment)
        require(shutil.which("node") is not None, "Node.js 24 is required for installed Web and TUI proofs")
        environment["PATH"] = os.environ["PATH"]
        environment["TEST_KEY"] = "fake-provider-unused"
        require("--local-execution-workspace" in cli(bundle, ["tui", "--help"], cwd, environment),
                "Installed TUI help is incomplete")
        with installed_provider() as provider:
            for name in ("web", "terminal", "exact-root"):
                (cwd / name).mkdir()
            try:
                web(bundle, cwd / "web", environment, provider.root, browser=options.browser_proof)
                if options.browser_proof:
                    web(bundle, cwd / "web", environment, provider.root, browser=True, resume=True)
                    web(bundle, cwd / "exact-root", environment, provider.root, browser=True, grants=False)
                tui_create(bundle, cwd / "terminal", environment, provider.root)
            finally:
                require(not provider.errors, f"Installed tool results: {provider.errors}")
            names = {"terminal-session"}
            if options.browser_proof:
                names.update(("web-root", "web-child", "web-fork", "web-resume"))
            provider.assert_calls(names)
        if options.process_harness:
            process_proof(options.process_harness.resolve(), products, options.mount_proof)
        if options.client_harness:
            for name in CLIENT_CASES:
                run_case(options.client_harness.resolve(), name,
                         {**os.environ, "RUSTX_INSTALLED_CLIENT_BINARY": str(client)})
        print(f"Installed {manifests[0]['source']} ({manifests[0]['platform']}); manifest and launcher checks passed")


if __name__ == "__main__":
    main()
