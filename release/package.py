#!/usr/bin/env python3
"""Build the native rustX distributions and record their installed contents."""

import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import tarfile
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parent.parent
SERVER = ("rustx", "workflow-renderer")
CLIENT = ("rustx-execution-client", "bash-supervisor", "interactive-supervisor")
PYTHON_GUARD = ("python", "python3")
PRODUCTS = ("rustx-server", "rustx-client", "rustx")
UI_PROJECTS = ("dev", "tui", "protocol/app-server", "web-console")


def run(*args: str, cwd: Path = ROOT) -> None:
    subprocess.run(args, cwd=cwd, check=True)


def output(*args: str) -> str:
    return subprocess.check_output(args, cwd=ROOT, text=True).strip()


def digest(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def validate_output_directory(path: Path, flag: str, parser: argparse.ArgumentParser) -> None:
    if path.exists() and not path.is_dir():
        parser.error(f"{flag} must name a directory: {path}")
    if not path.is_relative_to(ROOT):
        return
    if path == ROOT:
        parser.error(f"{flag} must not name the source checkout: {path}")
    relative = path.relative_to(ROOT).as_posix() + "/"
    ignored = subprocess.run(("git", "check-ignore", "--quiet", "--", relative), cwd=ROOT).returncode == 0
    if not ignored or output("git", "ls-files", "--", relative):
        parser.error(f"{flag} must name a directory outside the checkout or an ignored directory without tracked files: {path}")


def graph(package: str) -> set[str]:
    return set(output("cargo", "tree", "--locked", "-p", package, "-e", "normal,build",
                      "--prefix", "none", "--format", "{p}").splitlines())


def audit(package: str, forbidden: set[str], directory: Path) -> None:
    dependencies = graph(package)
    found = sorted({line.split()[0] for line in dependencies} & forbidden)
    if found:
        raise ValueError(f"{package} acquired forbidden production dependencies: {found}")
    directory.mkdir(parents=True, exist_ok=True)
    (directory / f"{package}.txt").write_text("\n".join(sorted(dependencies)) + "\n")


def build_server(target: Path) -> None:
    audit("rustx", {"rustx-execution-client"}, target / "dependency-audit")
    # The native macOS runner spent the remainder of its three-hour gate in
    # the single rustx LLVM unit. Use Cargo's normal release partitioning there.
    codegen = ("--config", "profile.release.package.rustx.codegen-units=16") if platform.system() == "Darwin" else ()
    run("cargo", "build", "--locked", "--release", *codegen, "--target-dir", str(target),
        "-p", "rustx", "--bin", "rustx", "--bin", "workflow-renderer")


def build_client(target: Path) -> None:
    audit("rustx-execution-client",
          {"rustx", "rustx-execution-server", "async-openai", "rusqlite", "libsqlite3-sys", "reqwest"},
          target / "dependency-audit")
    run("cargo", "build", "--locked", "--release", "--target-dir", str(target),
        "-p", "rustx-execution-client", "--bin", "rustx-execution-client")
    run("cargo", "build", "--locked", "--release", "--target-dir", str(target),
        "-p", "rustx-process-supervisor", "--bins")


def copy(source: Path, target: Path) -> None:
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, target)


def tree(source: Path, target: Path) -> None:
    shutil.copytree(source, target, symlinks=True)


def runtime_modules(source: Path, project: str, names: list[str], target: Path) -> None:
    """Install only the locked runtime closure from pnpm's isolated layout."""
    modules = source / project / "node_modules"
    packages = modules / ".pnpm"
    visited: set[Path] = set()

    def install(edge: Path) -> None:
        package = edge.resolve(strict=True)
        if not package.is_relative_to(packages):
            raise ValueError(f"{edge} resolves outside {project}'s isolated pnpm store")
        source_parent = package.parent
        while source_parent.name != "node_modules":
            source_parent = source_parent.parent
        destination = target / edge.relative_to(modules)
        destination.parent.mkdir(parents=True, exist_ok=True)
        link = os.path.relpath(target / package.relative_to(modules), destination.parent)
        if not destination.is_symlink():
            destination.symlink_to(link)
        elif os.readlink(destination) != link:
            raise ValueError(f"Conflicting pnpm dependency at {destination}")
        if package in visited:
            return
        visited.add(package)
        tree(package, target / package.relative_to(modules))
        # pnpm materializes the resolved required, peer and optional edges in
        # each package's private node_modules. A scoped name nests one level.
        for child in sorted(source_parent.iterdir()):
            if child.name.startswith("."):
                continue
            edges = child.iterdir() if child.name.startswith("@") else [child]
            for dependency in edges:
                if dependency.is_symlink():
                    install(dependency)

    for name in names:
        install(modules / name)


def pnpm_command(source: Path) -> tuple[str, str]:
    managers = {json.loads((source / project / "package.json").read_text())["packageManager"]
                for project in UI_PROJECTS}
    if len(managers) != 1 or not next(iter(managers)).startswith("pnpm@"):
        raise ValueError("UI packages must agree on their pinned pnpm release")
    return "corepack", managers.pop()


def install_ui(source: Path, target: Path, revision: str) -> None:
    # An install rebuilds the native modules, and Vite replaces dist. Build
    # from this release's tracked inputs away from any running browser carrier.
    source.mkdir()
    with tempfile.TemporaryFile() as stream:
        subprocess.run(("git", "archive", revision, "--", *UI_PROJECTS), cwd=ROOT,
                       stdout=stream, check=True)
        stream.seek(0)
        with tarfile.open(fileobj=stream) as tracked:
            tracked.extractall(source, filter="data")

    pnpm = pnpm_command(source)
    for project in UI_PROJECTS:
        run(*pnpm, "--dir", project, "install", "--frozen-lockfile", cwd=source)
    run(*pnpm, "--dir", "web-console", "build", cwd=source)
    for location in ("dev", "tui", "web-console"):
        copy(source / location / "package.json", target / location / "package.json")
    for location in ("dev/src", "tui/src", "protocol/app-server", "web-console/shared", "web-console/dist"):
        shutil.copytree(source / location, target / location, symlinks=True,
                        ignore=shutil.ignore_patterns("node_modules", "test", "fixtures")
                        if location == "protocol/app-server" else None)
    web = target / "web-console"
    shutil.copytree(source / "web-console/host", web / "host",
                    ignore=shutil.ignore_patterns("workspace-fs"))
    native = "host/workspace-fs/build/Release/workspace_fs.node"
    for path in ("browser-session.ts", "scripts/installed-carrier.ts", "src/workspaces/endpoint.ts",
                 "src/workspaces/host.ts", native):
        copy(source / "web-console" / path, web / path)
    runtime_modules(source, "dev", ["open"], target / "dev/node_modules")
    runtime_modules(source, "tui", ["@earendil-works/pi-tui", "ajv", "ajv-formats"], target / "tui/node_modules")
    runtime_modules(source, "web-console", ["node-pty", "saxes", "@deepseek-ai/libreoffice-kit"], web / "node_modules")
    copy(source / "tui/THIRD_PARTY_NOTICES.md", target / "tui/THIRD_PARTY_NOTICES.md")


def archive(directory: Path, path: Path, epoch: int) -> str:
    def normalized(member: tarfile.TarInfo) -> tarfile.TarInfo:
        member.uid = member.gid = 0
        member.uname = member.gname = ""
        member.mtime = epoch
        return member

    temporary = directory.parent / path.name
    with temporary.open("xb") as raw:
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=epoch) as compressed:
            with tarfile.open(fileobj=compressed, mode="w") as tar:
                tar.add(directory, arcname=directory.name, filter=normalized)
    checksum = digest(temporary)
    # Publish only a complete archive and preserve any earlier release.
    path.hardlink_to(temporary)
    return checksum


def package(product: str, target: Path, server: Path, client: Path, revision: str,
            version: str, machine: str, epoch: int) -> None:
    name = f"{product}-{version}-{machine}"
    result = target / f"{name}.tar.gz"
    with tempfile.TemporaryDirectory(prefix=f".{product}-", dir=target) as scratch:
        directory = Path(scratch) / name
        if product != "rustx-client":
            for binary in SERVER:
                installed = "rustx-server" if binary == "rustx" and product == "rustx-server" else binary
                copy(server / "release" / binary, directory / "bin" / installed)
            copy(server / "dependency-audit/rustx.txt", directory / "share/rustx-server-dependencies.txt")
            copy(ROOT / "examples/app-server/connect.mjs", directory / "share/examples/app-server/connect.mjs")
        if product != "rustx-server":
            for binary in CLIENT:
                installed = "rustx-client" if binary == "rustx-execution-client" and product == "rustx-client" else binary
                copy(client / "release" / binary, directory / "bin" / installed)
            for script in PYTHON_GUARD:
                copy(ROOT / "crates/execution-client/assets/python-guard" / script,
                     directory / "bin/python-guard" / script)
            copy(client / "dependency-audit/rustx-execution-client.txt", directory / "share/rustx-client-dependencies.txt")
        if product == "rustx":
            install_ui(Path(scratch) / "ui-source", directory / "share/rustx", revision)
        copy(ROOT / "LICENSE", directory / "share/LICENSE")
        for document in ("installation.md", "execution-protocol.md", "exec-04-evidence.md"):
            copy(ROOT / "docs" / document, directory / "share" / document)
        files = {}
        for path in sorted(directory.rglob("*")):
            if path.is_symlink():
                if not path.resolve(strict=True).is_relative_to(directory):
                    raise ValueError(f"Installed link escapes the archive: {path.relative_to(directory)}")
                files[path.relative_to(directory).as_posix()] = {"link": os.readlink(path)}
            elif path.is_file():
                path.chmod(0o755 if path.stat().st_mode & 0o111 else 0o644)
                files[path.relative_to(directory).as_posix()] = {
                    "sha256": digest(path), "bytes": path.stat().st_size,
                    "mode": oct(path.stat().st_mode & 0o777),
                }
        protocol = (ROOT / "crates/execution-protocol/src/lib.rs").read_text()
        revision_match = re.search(r"pub const PROTOCOL_VERSION: u16 = (\d+);", protocol)
        if revision_match is None:
            raise ValueError("Cannot determine Execution Protocol revision")
        manifest = {"product": product, "version": version, "source": revision, "platform": machine,
                    "execution_protocol": int(revision_match[1]), "files": files}
        (directory / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
        checksum = archive(directory, result, epoch)
        result.with_name(f"{result.name}.sha256").write_text(f"{checksum}  {result.name}\n")
    print(result, flush=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("product", choices=(*PRODUCTS, "all"))
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--server-target", type=Path, default=ROOT / "target/release-server")
    parser.add_argument("--client-target", type=Path, default=ROOT / "target/release-client")
    args = parser.parse_args()
    machine = f"{platform.system().lower()}-{platform.machine().lower()}"
    if platform.system() not in {"Linux", "Darwin"}:
        parser.error("Build natively on Linux or macOS (use WSL on Windows).")
    target, server, client = args.out.resolve(), args.server_target.resolve(), args.client_target.resolve()
    for flag, directory in (("--out", target), ("--server-target", server), ("--client-target", client)):
        validate_output_directory(directory, flag, parser)
    if server == client:
        parser.error("Use separate Server and Client Cargo targets.")
    revision = output("git", "rev-parse", "HEAD")
    if output("git", "status", "--porcelain", "--untracked-files=normal"):
        parser.error("Use a clean source checkout before building a versioned archive.")
    epoch = int(output("git", "show", "-s", "--format=%ct", revision))
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
    client_version = tomllib.loads((ROOT / "crates/execution-client/Cargo.toml").read_text())["package"]["version"]
    if version != client_version:
        parser.error("The Server and Client must use the same release version.")
    products = PRODUCTS if args.product == "all" else (args.product,)
    target.mkdir(parents=True, exist_ok=True)
    if args.product != "rustx-client":
        build_server(server)
    if args.product != "rustx-server":
        build_client(client)
    if output("git", "rev-parse", "HEAD") != revision or output("git", "status", "--porcelain"):
        parser.error("The source checkout changed during the build. Rebuild from a clean commit.")
    for product in products:
        package(product, target, server, client, revision, version, machine, epoch)


if __name__ == "__main__":
    main()
