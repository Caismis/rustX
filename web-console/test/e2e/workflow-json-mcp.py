"""Readonly catalog fixture for Workflow JSON payload browser acceptance."""
import json
import sys

for line in sys.stdin:
    request = json.loads(line)
    if "id" not in request:
        continue
    method = request["method"]
    if method == "initialize":
        result = {"protocolVersion": request["params"]["protocolVersion"], "capabilities": {"tools": {}}, "serverInfo": {"name": "workflow-json", "version": "1"}}
    elif method == "tools/list":
        result = {"tools": [{"name": "accept_payload", "description": "Accept a JSON object", "inputSchema": {"type": "object", "properties": {"metadata": {"type": "object"}}, "required": ["metadata"], "additionalProperties": False}}]}
    elif method == "ping":
        result = {}
    else:
        print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32601, "message": "Catalog-only fixture"}}), flush=True)
        continue
    print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
