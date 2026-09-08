"""Fixed checker. Exit zero means checks ran, not that business checks passed."""
import contextlib
import runpy
import sys
from pathlib import Path

source = Path("greeting.py")
if source.stat().st_size > 4096:
    raise ValueError("greeting.py exceeds the 4096-byte source limit")
# Candidate stdout cannot forge the checker's exact output contract.
with contextlib.redirect_stdout(sys.stderr):
    greeting = runpy.run_path(str(source))["greeting"]
    actual = [greeting("Ada"), greeting("")]
if any(not isinstance(value, str) or len(value) > 256 for value in actual):
    raise ValueError("greeting must return bounded strings")
sys.stdout.write("passed" if actual == ["Hello, Ada!", "Hello, friend!"] else "failed")
