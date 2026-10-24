"""A real stdio MCP peer whose tools/list awaits an explicit control receipt."""
import json
import socket
import sys

control = None
for line in sys.stdin:
    request = json.loads(line)
    if "id" not in request:
        continue
    method = request["method"]
    if method == "initialize":
        result = {"protocolVersion": request["params"]["protocolVersion"], "capabilities": {"tools": {}}, "serverInfo": {"name": "gated-startup", "version": "1"}}
    elif method == "tools/list":
        control = socket.create_connection(("127.0.0.1", int(sys.argv[1])))
        control.sendall(b"L")
        assert control.recv(1) == b"R"
        result = {"tools": [{"name": "startup_echo", "description": "Echo from the prepared source", "inputSchema": {"type": "object", "properties": {"message": {"type": "string"}}, "required": ["message"]}}]}
    elif method == "tools/call":
        result = {"content": [{"type": "text", "text": "prepared echo"}]}
    elif method == "ping":
        result = {}
    else:
        print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32601, "message": "Unknown method"}}), flush=True)
        continue
    print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
