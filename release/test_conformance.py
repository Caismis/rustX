import os
import pty
import selectors
import subprocess
import sys
import unittest

from conformance import stop_terminal


class TerminalShutdownTest(unittest.TestCase):
    def test_owner_drains_pending_terminal_output_before_asserting_exit(self):
        script = """
import signal
import sys


def stop(*_):
    sys.stdout.buffer.write(b'x' * (256 * 1024))
    sys.stdout.buffer.write(b'shutdown-display-complete\\n')
    sys.stdout.buffer.flush()
    raise SystemExit(143)


signal.signal(signal.SIGTERM, stop)
print('ready', flush=True)
signal.pause()
"""
        master, slave = pty.openpty()
        try:
            with subprocess.Popen([sys.executable, "-c", script], stdout=slave) as terminal:
                try:
                    with selectors.DefaultSelector() as selector:
                        selector.register(master, selectors.EVENT_READ)
                        self.assertTrue(selector.select(5), "terminal did not install its handler")
                        self.assertIn(b"ready", os.read(master, 8192))
                    tail = stop_terminal(terminal, master, timeout=5)
                    self.assertLessEqual(len(tail), 8192)
                    self.assertIn(b"shutdown-display-complete", tail)
                finally:
                    if terminal.poll() is None:
                        terminal.kill()
        finally:
            os.close(slave)
            os.close(master)
