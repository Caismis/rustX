"""Deterministic stdio MCP image source for real runtime/browser acceptance.

Uses only the standard MCP initialize/tools vocabulary. rustX performs Tool
execution, image artifact allocation and canonical result publication.
"""
import json
import sys

PNG = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aD1sAAAAASUVORK5CYII="
for line in sys.stdin:
    request = json.loads(line)
    if "id" not in request:
        continue
    method = request["method"]
    if method == "initialize":
        result = {"protocolVersion": request["params"]["protocolVersion"], "capabilities": {"tools": {}}, "serverInfo": {"name": "native-image-fixture", "version": "1"}}
    elif method == "tools/list":
        result = {"tools": [{"name": "render_image", "description": "Produce a deterministic one-pixel PNG", "inputSchema": {"type": "object", "properties": {}}}]}
    elif method == "tools/call":
        result = {"content": [{"type": "image", "data": PNG, "mimeType": "image/png"}]}
    elif method == "ping":
        result = {}
    else:
        print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32601, "message": "Unknown method"}}), flush=True)
        continue
    print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
