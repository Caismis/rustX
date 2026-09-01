"""A dependency-free managed FastMCP server for the rustX local-runtime example.

rustX discovers this package because it sits at
<workspace>/.agents/tools/echo/; there is no registration entry in
rustx.jsonc. The entrypoint is fixed: this file must expose a top-level
`mcp` FastMCP server object, and rustX launches it as `server.py:mcp`.
"""

from fastmcp import FastMCP

mcp = FastMCP("echo")


@mcp.tool
def echo(message: str) -> str:
    """Echo the supplied message back to the caller."""
    return message
