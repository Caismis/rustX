"""A minimal asyncio HTTP client for the emulator's own tests.

The emulator has no runtime dependencies, and its tests keep that property:
the client below speaks exactly the HTTP/1.1 subset the server serves.
"""

from __future__ import annotations

import asyncio
import json
from dataclasses import dataclass
from typing import Any


@dataclass
class Reply:
    status: int
    headers: dict[str, str]
    body: bytes

    @property
    def json(self) -> Any:
        return json.loads(self.body)

    @property
    def text(self) -> str:
        return self.body.decode()


async def request(
    host: str,
    port: int,
    method: str,
    path: str,
    body: bytes | None = None,
    headers: dict[str, str] | None = None,
) -> Reply:
    """Sends one request and reads the whole response (`Connection: close`)."""
    reader, writer = await asyncio.open_connection(host, port)
    try:
        lines = [f"{method} {path} HTTP/1.1", f"Host: {host}:{port}"]
        for name, value in (headers or {}).items():
            lines.append(f"{name}: {value}")
        lines.append(f"Content-Length: {len(body or b'')}")
        writer.write(("\r\n".join(lines) + "\r\n\r\n").encode())
        if body:
            writer.write(body)
        await writer.drain()
        return await _read_reply(reader)
    finally:
        writer.close()


async def open_stream(
    host: str, port: int, path: str, body: bytes
) -> tuple[asyncio.StreamReader, asyncio.StreamWriter]:
    """Opens a streaming request and returns the live connection."""
    reader, writer = await asyncio.open_connection(host, port)
    head = (
        f"POST {path} HTTP/1.1\r\nHost: {host}:{port}\r\n"
        f"Content-Type: application/json\r\nContent-Length: {len(body)}\r\n\r\n"
    )
    writer.write(head.encode() + body)
    await writer.drain()
    return reader, writer


async def _read_reply(reader: asyncio.StreamReader) -> Reply:
    status_line = await reader.readline()
    status = int(status_line.decode().split(" ")[1])
    headers: dict[str, str] = {}
    while True:
        line = await reader.readline()
        if line in (b"\r\n", b"\n", b""):
            break
        name, _, value = line.decode().rstrip("\r\n").partition(":")
        headers[name.strip().lower()] = value.strip()
    body = await reader.read()
    return Reply(status=status, headers=headers, body=body)


def post_json(host: str, port: int, path: str, payload: dict[str, Any]) -> Any:
    """Convenience wrapper used by the synchronous parts of a test."""
    return asyncio.run(
        request(
            host,
            port,
            "POST",
            path,
            json.dumps(payload).encode(),
            {"Content-Type": "application/json"},
        )
    )
