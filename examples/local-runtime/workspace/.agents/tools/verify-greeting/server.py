"""Project-owned check, executed through the ordinary managed Python Tool Plane."""

import runpy
from pathlib import Path

from fastmcp import FastMCP
from fastmcp.tools import ToolResult

mcp = FastMCP("verify-greeting")


@mcp.tool
def verify_greeting() -> ToolResult:
    """Check this workspace's greeting.py against two fixed project cases."""
    # The ordinary Tool workspace is the authority; no alternate cwd or process
    # runner is introduced. Loading/contract errors remain native MCP failures.
    source = Path("greeting.py")
    if source.stat().st_size > 4096:
        raise ValueError("greeting.py exceeds this example's 4096-byte source limit")
    greeting = runpy.run_path(str(source))["greeting"]
    failures = []
    for name, expected in (("Ada", "Hello, Ada!"), ("", "Hello, friend!")):
        actual = greeting(name)
        if not isinstance(actual, str) or len(actual) > 256:
            raise ValueError("greeting must return a string of at most 256 characters")
        if actual != expected:
            failures.append({"case": name, "expected": expected, "actual": actual})
    # Explicitly one Text part, then structuredContent: native JSON is part 1.
    # A completed checker with findings is Success, even when passed is false.
    return ToolResult(
        content="Executed two fixed greeting checks.",
        structured_content={"passed": not failures, "failures": failures},
    )
