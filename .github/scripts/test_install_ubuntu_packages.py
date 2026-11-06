import pathlib
import subprocess
import tempfile
import unittest
from unittest import mock

import install_ubuntu_packages as installer


class UbuntuPackages(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.mirrors = pathlib.Path(temporary.name) / "apt-mirrors.txt"
        self.mirrors.write_text(
            "http://azure.archive.ubuntu.com/ubuntu/\tpriority:1\n"
            "https://archive.ubuntu.com/ubuntu/\tpriority:2\n"
            "https://security.ubuntu.com/ubuntu/\tpriority:3\n"
        )
        self.patch = mock.patch.object(installer, "MIRROR_LIST", self.mirrors)
        self.patch.start()
        self.addCleanup(self.patch.stop)

    @mock.patch.object(installer.subprocess, "run")
    def test_official_archive_is_selected_before_refresh_and_install(self, run):
        observations = []
        run.side_effect = lambda *args, **kwargs: observations.append(self.mirrors.read_text())
        installer.install(["bubblewrap", "apparmor-profiles", "libreoffice-writer"])
        self.assertEqual(observations, ["https://archive.ubuntu.com/ubuntu/\n"] * 2)
        commands = [call.args[0] for call in run.call_args_list]
        self.assertEqual(commands[0][-1], "update")
        self.assertEqual(commands[1][-5:], ["install", "--yes", "bubblewrap", "apparmor-profiles", "libreoffice-writer"])
        for command, call in zip(commands, run.call_args_list):
            self.assertEqual(command[0], "apt-get")
            self.assertIn("Acquire::http::Timeout=30", command)
            self.assertIn("Acquire::https::Timeout=30", command)
            self.assertIn("Acquire::Retries=2", command)
            self.assertIn("APT::Update::Error-Mode=any", command)
            self.assertEqual(call.kwargs, {"check": True})

    @mock.patch.object(installer.subprocess, "run")
    def test_index_failure_stops_before_installation(self, run):
        run.side_effect = subprocess.CalledProcessError(100, "apt-get update")
        with self.assertRaises(subprocess.CalledProcessError):
            installer.install(["libclang-dev"])
        self.assertEqual(run.call_count, 1)
        self.assertEqual(run.call_args.args[0][-1], "update")

    @mock.patch.object(installer.subprocess, "run")
    def test_package_failure_is_not_suppressed(self, run):
        run.side_effect = [None, subprocess.CalledProcessError(100, "apt-get install")]
        with self.assertRaises(subprocess.CalledProcessError):
            installer.install(["libclang-dev"])
        self.assertEqual(run.call_count, 2)

    @mock.patch.object(installer.subprocess, "run")
    def test_missing_runner_mirror_list_fails_before_running_apt(self, run):
        self.mirrors.unlink()
        with self.assertRaisesRegex(RuntimeError, "mirror list is missing"):
            installer.install(["libclang-dev"])
        run.assert_not_called()
        self.assertFalse(self.mirrors.exists())


if __name__ == "__main__":
    unittest.main()
