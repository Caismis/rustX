"""Readonly catalog fixture for Workflow JSON payload browser acceptance."""
import json
import sys

def catalog(request):
    if "--empty-enum" in sys.argv:
        return {"tools": [{"name": "empty_" + kind, "description": "Accept an explicit empty string", "inputSchema": {"type": "object", "properties": {"mode": {"type": "string", "enum": ["", "explicit"]}}, "required": ["mode"] if kind == "required" else [], "additionalProperties": False}} for kind in ["required", "optional"]]}
    if "--large-catalog" not in sys.argv:
        return {"tools": [{"name": "accept_payload", "description": "Accept a JSON object", "inputSchema": {"type": "object", "properties": {"metadata": {"type": "object"}}, "required": ["metadata"], "additionalProperties": False}}]}
    offset = int(request.get("params", {}).get("cursor", "0"))
    properties = {f"field_{i}": {"type": "string", "description": "A field description. " * 6} for i in range(64)}
    result = {"tools": [{"name": f"tool_{i:03}", "description": f"Catalog tool {i}", "inputSchema": {"type": "object", "properties": properties, "additionalProperties": False}} for i in range(offset, min(offset + 16, 128))]}
    if offset + 16 < 128:
        result["nextCursor"] = str(offset + 16)
    return result


for line in sys.stdin:
    request = json.loads(line)
    if "id" not in request:
        continue
    method = request["method"]
    if method == "initialize":
        result = {"protocolVersion": request["params"]["protocolVersion"], "capabilities": {"tools": {}}, "serverInfo": {"name": "workflow-json", "version": "1"}}
    elif method == "tools/list":
        result = catalog(request)
    elif method == "ping":
        result = {}
    else:
        print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32601, "message": "Catalog-only fixture"}}), flush=True)
        continue
    print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
