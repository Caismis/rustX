"""A real stdio MCP peer whose tools/list awaits an explicit control receipt."""
import json
import os
import socket
import sys

control = socket.create_connection(("127.0.0.1", int(sys.argv[1])))
control.sendall(f"P:{os.getpid()}:{os.getppid()}:{os.getpgrp()}\n".encode())
tool_name = sys.argv[2] if len(sys.argv) > 2 else "startup_echo"
for line in sys.stdin:
    request = json.loads(line)
    if "id" not in request:
        continue
    method = request["method"]
    if method == "server/discover":
        control.sendall(b"D\n")
    if method == "initialize":
        control.sendall(b"I\n")
        result = {"protocolVersion": request["params"]["protocolVersion"], "capabilities": {"tools": {}}, "serverInfo": {"name": "gated-startup", "version": "1"}}
    elif method == "tools/list":
        control.sendall(b"L\n")
        assert control.recv(1) == b"R"
        result = {"tools": [{"name": tool_name, "description": "Echo from the prepared source", "inputSchema": {"type": "object", "properties": {"message": {"type": "string"}}, "required": ["message"]}}]}
    elif method == "tools/call":
        result = {"content": [{"type": "text", "text": "prepared echo"}]}
    elif method == "ping":
        result = {}
    else:
        print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32601, "message": "Unknown method"}}), flush=True)
        continue
    print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
    if method == "tools/list":
        control.sendall(b"T\n")
