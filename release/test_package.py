"""Exercise package preflight and build commands without compiling a release."""

from contextlib import redirect_stderr
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import package


class PackagePaths(unittest.TestCase):
    def test_native_server_targets_use_their_release_codegen(self):
        target = Path("/tmp/isolated-server-build")
        for system, extra in (
            ("Linux", ()),
            ("Darwin", ("--config", "profile.release.package.rustx.codegen-units=16")),
        ):
            with (
                self.subTest(system=system),
                patch.object(package.platform, "system", return_value=system),
                patch.object(package, "audit") as audit,
                patch.object(package, "run") as build,
            ):
                package.build_server(target)
                audit.assert_called_once_with("rustx", {"rustx-execution-client"}, target / "dependency-audit")
                build.assert_called_once_with(
                    "cargo", "build", "--locked", "--release", *extra, "--target-dir", str(target),
                    "-p", "rustx", "--bin", "rustx", "--bin", "workflow-renderer",
                )

    def test_shared_pnpm_parent_copies_a_dependency_link_once(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            modules = root / "source/dev/node_modules"
            shared = modules / ".pnpm/shared/node_modules"
            peer = modules / ".pnpm/peer/node_modules/peer"
            peer.mkdir(parents=True)
            for name in ("first", "second"):
                (shared / name).mkdir(parents=True)
                (modules / name).symlink_to(os.path.relpath(shared / name, modules))
            (shared / "peer").symlink_to(os.path.relpath(peer, shared))
            (peer / "peer.txt").write_text("peer bytes")
            output = root / "output"
            package.runtime_modules(root / "source", "dev", ["first", "second"], output)
            self.assertEqual((output / "first").resolve(), output / ".pnpm/shared/node_modules/first")
            self.assertEqual((output / "second").resolve(), output / ".pnpm/shared/node_modules/second")
            self.assertEqual((output / ".pnpm/shared/node_modules/peer/peer.txt").read_text(), "peer bytes")

    def test_pnpm_command_follows_the_current_product_manifests(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for project in package.UI_PROJECTS:
                (root / project).mkdir(parents=True)
                (root / project / "package.json").write_text(json.dumps({"packageManager": "pnpm@12.7.3"}))
            self.assertEqual(package.pnpm_command(root), ("corepack", "pnpm@12.7.3"))
            (root / "web-console/package.json").write_text(json.dumps({"packageManager": "pnpm@12.8.0"}))
            with self.assertRaisesRegex(ValueError, "must agree"):
                package.pnpm_command(root)

    def test_each_unignored_path_fails_before_build_or_output_creation(self):
        with tempfile.TemporaryDirectory() as directory:
            parent = Path(directory).resolve()
            root = parent / "source"
            root.mkdir()
            subprocess.run(["git", "init", "--quiet", root], check=True)
            (root / ".gitignore").write_text("/target/\n")
            subprocess.run(["git", "-C", root, "add", ".gitignore"], check=True)
            allowed = ["package.py", "all", "--out", str(parent / "archives"),
                       "--server-target", str(root / "target/server"),
                       "--client-target", str(root / "target/client")]
            for flag in ("--out", "--server-target", "--client-target"):
                with self.subTest(flag=flag):
                    args = allowed.copy()
                    rejected = root / flag.removeprefix("--")
                    args[args.index(flag) + 1] = str(rejected)
                    with (patch.object(package, "ROOT", root), patch("sys.argv", args),
                          patch.object(package, "build_server") as build,
                          redirect_stderr(io.StringIO()) as errors,
                          self.assertRaises(SystemExit) as caught):
                        package.main()
                    self.assertEqual(caught.exception.code, 2)
                    self.assertIn(f"{flag} must name a directory", errors.getvalue())
                    build.assert_not_called()
                    self.assertFalse(rejected.exists())


if __name__ == "__main__":
    unittest.main()
