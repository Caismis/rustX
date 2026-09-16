"""Local MCP fixture with an observable startup marker; disabled saves stay inert."""
import json
import pathlib
import sys

pathlib.Path(sys.argv[1]).write_text("started\n")
for line in sys.stdin:
    request = json.loads(line)
    if "id" not in request:
        continue
    method = request["method"]
    if method == "initialize":
        result = {"protocolVersion": request["params"]["protocolVersion"], "capabilities": {"tools": {}}, "serverInfo": {"name": "web09-local", "version": "1"}}
    elif method == "tools/list":
        result = {"tools": []}
    else:
        result = {}
    print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
