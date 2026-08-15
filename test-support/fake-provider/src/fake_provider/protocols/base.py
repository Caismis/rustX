"""The protocol codec boundary."""

from __future__ import annotations

import json
from typing import Any, Iterable, Protocol, Union

from fake_provider.scenario import Chunk, Disconnect, Gate

#: One encoded emission. Bytes are written and flushed; `Gate` suspends the
#: response until the control API releases it; `Disconnect` closes the
#: connection abruptly. Keeping the control items *inside* the encoded stream
#: is what makes "gate after chunk 2" mean exactly that on the wire.
Emit = Union[bytes, Gate, Disconnect]


class ProtocolCodec(Protocol):
    """Encodes scenario semantics into one provider wire protocol."""

    #: The protocol identifier, matching rustX's `ModelProtocol` spelling.
    name: str
    #: The request path rustX's adapter for this protocol sends to.
    default_path: str
    #: The response content type of a successful streamed response.
    content_type: str

    def model_of(self, body: dict[str, Any]) -> str | None:
        """The model id carried by a request body."""

    def tool_names(self, body: dict[str, Any]) -> list[str]:
        """The tool names offered to the model by a request body."""

    def encode(self, script: Iterable[Chunk], model: str) -> list[Emit]:
        """Encodes one high-level response script into wire emissions.

        `model` is the model id the request carried, so the scripted
        response echoes the model rustX actually asked for rather than a
        constant the scenario would have to restate.
        """


def sse(data: Any, event: str | None = None) -> bytes:
    """One Server-Sent Event frame."""
    payload = data if isinstance(data, str) else json.dumps(data, separators=(",", ":"))
    prefix = f"event: {event}\n" if event else ""
    return f"{prefix}data: {payload}\n\n".encode()
