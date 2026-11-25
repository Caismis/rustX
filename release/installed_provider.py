"""Loopback provider for the installed Web and TUI product checks."""

from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import threading


class InstalledProvider:
    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.requests: dict[str, list[str]] = {}
        self.errors: list[str] = []
        self.root = ""

    def reply(self, body: dict) -> bytes:
        messages = body["messages"]
        # Chat completions may send content as an array and add runtime notices.
        prompt = " ".join(str(message["content"]) for message in messages if message["role"] == "user")
        names = [name for name in ("web-root", "web-child", "terminal-session")
                 if f"exercise {name}" in prompt]
        if len(names) != 1:
            raise ValueError(f"Unexpected installed fixture input: {prompt[:400]!r}")
        name = names[0]
        if any("Write a concise title for the task" in str(message["content"])
               for message in messages if message["role"] == "system"):
            return self.chunk({"role": "assistant", "content": f"Test {name}"}, "stop")
        results = {message["tool_call_id"]: message["content"] for message in messages
                   if message["role"] == "tool"}
        with self.lock:
            self.requests.setdefault(name, []).append("results" if results else "start")
        if results:
            expected = {"installed_read": f"seed-{name}", "installed_bash": f"bash-{name}",
                        "installed_write": "written.txt"}
            for call, marker in expected.items():
                if marker not in results.get(call, ""):
                    raise ValueError(f"{name} {call} did not report {marker!r}: {results.get(call)!r}")
            delta = {"role": "assistant", "content": f"Installed tools complete for {name}"}
            reason = "stop"
        else:
            calls = [
                ("read", {"arguments": {"path": "seed.txt"}}),
                ("bash", {"execution_mode": "foreground", "arguments": {
                    "command": f"printf 'bash-{name}'; pwd > bash-cwd.txt",
                }}),
                ("write", {"arguments": {"path": "written.txt", "content": f"write-{name}\n"}}),
            ]
            delta = {"role": "assistant", "tool_calls": [
                {"index": index, "id": f"installed_{tool}", "type": "function",
                 "function": {"name": tool, "arguments": json.dumps(arguments)}}
                for index, (tool, arguments) in enumerate(calls)
            ]}
            reason = "tool_calls"
        return self.chunk(delta, reason)

    @staticmethod
    def chunk(delta: dict, reason: str) -> bytes:
        chunk = {"id": "installed", "object": "chat.completion.chunk", "created": 1, "model": "test",
                 "choices": [{"index": 0, "delta": delta, "finish_reason": reason}]}
        return f"data: {json.dumps(chunk)}\n\ndata: [DONE]\n\n".encode()

    def assert_calls(self, names: set[str]) -> None:
        with self.lock:
            assert not self.errors, self.errors
            selected = {name: self.requests.get(name) for name in names}
            assert selected == dict.fromkeys(names, ["start", "results"]), selected


@contextmanager
def installed_provider():
    provider = InstalledProvider()

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def do_POST(self):
            try:
                assert self.path == "/v1/chat/completions", self.path
                result = provider.reply(json.loads(self.rfile.read(int(self.headers["Content-Length"]))))
            except Exception as error:
                with provider.lock:
                    provider.errors.append(str(error))
                self.send_error(400, "Unexpected installed fixture request")
                return
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Content-Length", str(len(result)))
            self.end_headers()
            self.wfile.write(result)

    with ThreadingHTTPServer(("127.0.0.1", 0), Handler) as server:
        provider.root = f"http://127.0.0.1:{server.server_port}/v1"
        thread = threading.Thread(target=server.serve_forever)
        thread.start()
        try:
            yield provider
        finally:
            server.shutdown()
            thread.join()
