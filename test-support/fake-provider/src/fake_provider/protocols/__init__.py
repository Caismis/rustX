"""Protocol codecs: wire encoding separated from scenario semantics.

The emulator is structured around the *protocol boundaries* rustX supports,
never around provider brand names. A provider brand that reuses one of these
protocols reuses the same codec; there is deliberately no per-brand server.

```text
scenario semantics          protocol codec              wire
-------------------         ---------------------       ----------------------
Text("hello")          ->   openai_chat.encode     ->   data: {...delta...}
                            openai_responses.encode->   data: {"type":"response...
                            anthropic.encode       ->   event: content_block_delta
```
"""

from __future__ import annotations

from fake_provider.protocols.anthropic_messages import AnthropicMessagesCodec
from fake_provider.protocols.base import Emit, ProtocolCodec
from fake_provider.protocols.openai_chat import OpenAiChatCompletionsCodec
from fake_provider.protocols.openai_responses import OpenAiResponsesCodec
from fake_provider.scenario import (
    ANTHROPIC_MESSAGES,
    OPENAI_CHAT_COMPLETIONS,
    OPENAI_RESPONSES,
)

CODECS: dict[str, ProtocolCodec] = {
    OPENAI_CHAT_COMPLETIONS: OpenAiChatCompletionsCodec(),
    OPENAI_RESPONSES: OpenAiResponsesCodec(),
    ANTHROPIC_MESSAGES: AnthropicMessagesCodec(),
}

#: Reverse lookup from the served path to the protocol that owns it, so an
#: unexpected request can still be attributed to a protocol in the record.
PATHS: dict[str, str] = {codec.default_path: name for name, codec in CODECS.items()}


def codec_for(protocol: str) -> ProtocolCodec:
    """The codec of one protocol.

    Raises:
        KeyError: when the scenario names a protocol that does not exist.
    """
    return CODECS[protocol]


def protocol_of_path(path: str) -> str | None:
    """The protocol a request path belongs to, when it is one rustX speaks."""
    return PATHS.get(path)


__all__ = [
    "CODECS",
    "Emit",
    "PATHS",
    "ProtocolCodec",
    "codec_for",
    "protocol_of_path",
]
