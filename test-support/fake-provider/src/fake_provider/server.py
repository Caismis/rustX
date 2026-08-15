"""The loopback HTTP/1.1 provider process.

The server speaks raw HTTP/1.1 on purpose. The scenarios this harness exists
for need byte-level control an ASGI framework hides: a gate *between* two
flushed chunks, an abrupt disconnect at an exact position, deliberately
malformed SSE framing, and observation of the client closing the connection
while the response is suspended. Request parsing needed here is one request
line, headers, and a `Content-Length` body — rustX never sends anything else.

Every response carries `Connection: close`: one request per connection keeps
the request record, the disconnect observation, and the stream lifecycle
unambiguously paired.
"""

from __future__ import annotations

import asyncio
import json
from typing import Any
from urllib.parse import parse_qs, urlsplit

from fake_provider import control
from fake_provider.control import ScenarioRun
from fake_provider.protocols import codec_for, protocol_of_path
from fake_provider.scenario import (
    Disconnect,
    Gate,
    HttpError,
    Raw,
    RawResponse,
    RecordedRequest,
    Stream,
    sanitize_headers,
)

CONTROL_PREFIX = "/__control"

#: The upper bound on a control-plane barrier wait. It is deadlock
#: protection, never the ordering mechanism.
DEFAULT_AWAIT_TIMEOUT_MS = 10_000


class ProviderServer:
    """One scenario, served on one loopback port."""

    def __init__(self, run: ScenarioRun) -> None:
        self.run = run
        self._server: asyncio.Server | None = None

    async def start(self, host: str, port: int) -> tuple[str, int]:
        self._server = await asyncio.start_server(self._handle, host, port)
        bound = self._server.sockets[0].getsockname()
        return bound[0], bound[1]

    async def serve_until_shutdown(self) -> None:
        await self.run.shutdown_requested.wait()
        if self._server is not None:
            self._server.close()
            await self._server.wait_closed()

    # -- connection handling ---------------------------------------------

    async def _handle(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        try:
            head = await _read_head(reader)
            if head is None:
                return
            method, target, headers = head
            body = await _read_body(reader, headers)
            path = urlsplit(target).path
            if path.startswith(CONTROL_PREFIX):
                await self._control(method, target, writer)
                return
            await self._provider(method, path, headers, body, reader, writer)
        except (ConnectionResetError, BrokenPipeError):
            return
        finally:
            writer.close()
            try:
                await writer.wait_closed()
            except (ConnectionResetError, BrokenPipeError):
                pass

    # -- the provider surface --------------------------------------------

    async def _provider(
        self,
        method: str,
        path: str,
        headers: dict[str, str],
        body: bytes,
        reader: asyncio.StreamReader,
        writer: asyncio.StreamWriter,
    ) -> None:
        index = len(self.run.requests)
        protocol = protocol_of_path(path)
        recorded_headers, credential_headers = sanitize_headers(headers)
        body_text = body.decode("utf-8", errors="replace")
        try:
            parsed = json.loads(body_text) if body_text else None
        except json.JSONDecodeError:
            parsed = None
        codec = codec_for(protocol) if protocol else None
        request = RecordedRequest(
            index=index,
            method=method,
            path=path,
            protocol=protocol,
            model=codec.model_of(parsed) if codec and isinstance(parsed, dict) else None,
            headers=recorded_headers,
            credential_headers=credential_headers,
            body_text=body_text,
            json=parsed if isinstance(parsed, dict) else None,
        )
        self.run.requests.append(request)
        await self.run.observe(
            control.REQUEST_ACCEPTED, request_index=index, path=path, protocol=protocol
        )

        if index >= len(self.run.scenario.steps):
            await self.run.fail(
                f"unexpected provider request #{index + 1}: the scenario declares "
                f"{len(self.run.scenario.steps)} step(s)",
                request_index=index,
            )
            await _write_json(
                writer,
                500,
                {"error": {"type": "scenario_error", "message": "unexpected extra request"}},
            )
            return

        step = self.run.scenario.steps[index]
        expected_codec = codec_for(step.expect.protocol)
        failures = step.expect.failures(request, expected_codec)
        if failures:
            for failure in failures:
                await self.run.fail(f"request #{index + 1}: {failure}", request_index=index)
            await _write_json(
                writer,
                500,
                {
                    "error": {
                        "type": "scenario_error",
                        "message": f"request #{index + 1} did not match the scenario step",
                        "failures": failures,
                    }
                },
            )
            return

        self.run.steps_consumed = index + 1
        await self._respond(step, expected_codec, request, index, reader, writer)
        if self.run.steps_consumed == len(self.run.scenario.steps):
            await self.run.observe(control.SCENARIO_COMPLETED)

    async def _respond(
        self,
        step: Any,
        codec: Any,
        request: RecordedRequest,
        index: int,
        reader: asyncio.StreamReader,
        writer: asyncio.StreamWriter,
    ) -> None:
        response = step.respond
        if isinstance(response, HttpError):
            await _write_json(writer, response.status, dict(response.body))
            await self.run.observe(control.HEADERS_SENT, request_index=index)
            await self.run.observe(control.RESPONSE_COMPLETED, request_index=index)
            return

        model = request.model or "fake-model"
        if isinstance(response, Stream):
            emits = codec.encode(response.script, model)
            status, extra_headers = 200, {"Content-Type": codec.content_type}
        else:
            assert isinstance(response, RawResponse)
            # A raw response bypasses the codec entirely: only literal bytes
            # and the two control items are meaningful, and anything else is
            # a scenario authoring mistake rather than something to encode.
            emits = []
            for item in response.script:
                if isinstance(item, Raw):
                    emits.append(item.data)
                elif isinstance(item, (Gate, Disconnect)):
                    emits.append(item)
                else:
                    raise TypeError(
                        f"a RawResponse script accepts Raw/Gate/Disconnect only, got {item!r}"
                    )
            status, extra_headers = response.status, dict(response.headers)

        disconnected = asyncio.Event()
        watcher = asyncio.create_task(self._watch_disconnect(reader, disconnected, index))
        completed = False
        try:
            await _write_head(writer, status, extra_headers)
            await self.run.observe(control.HEADERS_SENT, request_index=index)
            for emit in emits:
                if isinstance(emit, Gate):
                    await self.run.reach_gate(emit.name, index)
                    continue
                if isinstance(emit, Disconnect):
                    writer.transport.abort()
                    return
                if isinstance(emit, bytes):
                    writer.write(emit)
                    try:
                        await writer.drain()
                    except (ConnectionResetError, BrokenPipeError):
                        disconnected.set()
                        return
                    await self.run.observe(
                        control.CHUNK_FLUSHED, request_index=index, bytes=len(emit)
                    )
                else:  # pragma: no cover - the union is closed
                    raise TypeError(f"unsupported emission {emit!r}")
            completed = True
            await self.run.observe(control.RESPONSE_COMPLETED, request_index=index)
        finally:
            watcher.cancel()
            if disconnected.is_set() and not completed and not step.allow_disconnect:
                await self.run.fail(
                    f"request #{index + 1}: the client disconnected before the scripted "
                    "response finished, and the step does not allow it",
                    request_index=index,
                )

    async def _watch_disconnect(
        self, reader: asyncio.StreamReader, disconnected: asyncio.Event, index: int
    ) -> None:
        """Observes the client closing its side while a response is open.

        This is what makes a cancellation test deterministic: the driver can
        wait for `client_disconnected` rather than assume the runtime got
        around to dropping the stream.
        """
        try:
            data = await reader.read(1024)
        except (ConnectionResetError, BrokenPipeError):
            data = b""
        if data == b"":
            disconnected.set()
            await self.run.observe(control.CLIENT_DISCONNECTED, request_index=index)

    # -- the control surface ---------------------------------------------

    async def _control(self, method: str, target: str, writer: asyncio.StreamWriter) -> None:
        split = urlsplit(target)
        path = split.path[len(CONTROL_PREFIX) :]
        query = parse_qs(split.query)

        if method == "GET" and path == "/state":
            await _write_json(writer, 200, self.run.state())
            return
        if method == "GET" and path == "/requests":
            await _write_json(
                writer,
                200,
                {"requests": [request.to_json() for request in self.run.requests]},
            )
            return
        if method == "GET" and path == "/observations":
            await _write_json(
                writer,
                200,
                {
                    "observations": [
                        observation.to_json() for observation in self.run.observations
                    ]
                },
            )
            return
        if method == "GET" and path == "/observations/await":
            await self._await_observation(query, writer)
            return
        if method == "POST" and path.startswith("/gates/") and path.endswith("/release"):
            name = path[len("/gates/") : -len("/release")]
            released = await self.run.release_gate(name)
            if not released:
                await _write_json(
                    writer, 404, {"error": f"the scenario declares no gate {name!r}"}
                )
                return
            await _write_json(writer, 200, {"gate": name, "state": self.run.gates[name]})
            return
        if method == "POST" and path == "/shutdown":
            await _write_json(writer, 200, self.run.report())
            self.run.shutdown_requested.set()
            return
        await _write_json(writer, 404, {"error": f"no control route {method} {path}"})

    async def _await_observation(
        self, query: dict[str, list[str]], writer: asyncio.StreamWriter
    ) -> None:
        kind = _one(query, "kind")
        if kind is None:
            await _write_json(writer, 400, {"error": "kind is required"})
            return
        request_index = _one(query, "requestIndex")
        timeout_ms = int(_one(query, "timeoutMs") or DEFAULT_AWAIT_TIMEOUT_MS)
        observation = await self.run.await_observation(
            kind,
            name=_one(query, "name"),
            request_index=int(request_index) if request_index is not None else None,
            count=int(_one(query, "count") or 1),
            timeout=timeout_ms / 1000,
        )
        if observation is None:
            await _write_json(
                writer,
                504,
                {
                    "error": "the provider never reached that observation",
                    "kind": kind,
                    "timeoutMs": timeout_ms,
                    "state": self.run.state(),
                },
            )
            return
        await _write_json(writer, 200, observation.to_json())


def _one(query: dict[str, list[str]], key: str) -> str | None:
    values = query.get(key)
    return values[0] if values else None


# ---------------------------------------------------------------------------
# Raw HTTP/1.1
# ---------------------------------------------------------------------------


async def _read_head(
    reader: asyncio.StreamReader,
) -> tuple[str, str, dict[str, str]] | None:
    """Reads the request line and headers; `None` on a closed connection."""
    try:
        line = await reader.readline()
    except (ConnectionResetError, BrokenPipeError):
        return None
    if not line:
        return None
    parts = line.decode("latin-1").rstrip("\r\n").split(" ")
    if len(parts) < 2:
        return None
    method, target = parts[0], parts[1]
    headers: dict[str, str] = {}
    while True:
        header = await reader.readline()
        if header in (b"\r\n", b"\n", b""):
            break
        name, _, value = header.decode("latin-1").rstrip("\r\n").partition(":")
        headers[name.strip().lower()] = value.strip()
    return method, target, headers


async def _read_body(reader: asyncio.StreamReader, headers: dict[str, str]) -> bytes:
    length = int(headers.get("content-length", "0") or 0)
    if length <= 0:
        return b""
    return await reader.readexactly(length)


async def _write_head(
    writer: asyncio.StreamWriter, status: int, headers: dict[str, str]
) -> None:
    lines = [f"HTTP/1.1 {status} {_REASONS.get(status, 'OK')}"]
    lines.extend(f"{name}: {value}" for name, value in headers.items())
    lines.append("Cache-Control: no-cache")
    lines.append("Connection: close")
    writer.write(("\r\n".join(lines) + "\r\n\r\n").encode())
    await writer.drain()


async def _write_json(writer: asyncio.StreamWriter, status: int, payload: Any) -> None:
    body = json.dumps(payload).encode()
    await _write_head(
        writer,
        status,
        {"Content-Type": "application/json", "Content-Length": str(len(body))},
    )
    writer.write(body)
    try:
        await writer.drain()
    except (ConnectionResetError, BrokenPipeError):
        pass


_REASONS = {
    200: "OK",
    400: "Bad Request",
    404: "Not Found",
    429: "Too Many Requests",
    500: "Internal Server Error",
    502: "Bad Gateway",
    503: "Service Unavailable",
    504: "Gateway Timeout",
}
