"""The Anthropic Messages wire codec."""

from __future__ import annotations

from typing import Any, Iterable

from fake_provider.protocols.base import Emit, sse
from fake_provider.scenario import (
    Chunk,
    Disconnect,
    Finish,
    Gate,
    Raw,
    Reasoning,
    Text,
    ToolCall,
    Usage,
    split_text,
)

_MESSAGE_ID = "msg_fake_provider"

#: The scenario's protocol-neutral finish vocabulary mapped onto Anthropic
#: stop reasons.
_STOP_REASONS = {
    "stop": "end_turn",
    "tool_calls": "tool_use",
    "length": "max_tokens",
    "content_filter": "refusal",
}


class AnthropicMessagesCodec:
    """Encodes a response script as Anthropic Messages stream events."""

    name = "anthropic_messages"
    default_path = "/v1/messages"
    content_type = "text/event-stream"

    def model_of(self, body: dict[str, Any]) -> str | None:
        model = body.get("model")
        return model if isinstance(model, str) else None

    def tool_names(self, body: dict[str, Any]) -> list[str]:
        names: list[str] = []
        for tool in body.get("tools") or []:
            if isinstance(tool, dict) and isinstance(tool.get("name"), str):
                names.append(tool["name"])
        return names

    def encode(self, script: Iterable[Chunk], model: str) -> list[Emit]:
        script = list(script)
        emits: list[Emit] = []
        usage = next((item for item in script if isinstance(item, Usage)), None)
        input_tokens = usage.input_tokens if usage else 0
        output_tokens = usage.output_tokens if usage else 0
        block_index = -1
        open_block: str | None = None

        def event(name: str, payload: dict[str, Any]) -> bytes:
            return sse({"type": name, **payload}, event=name)

        def close_block() -> None:
            nonlocal open_block
            if open_block is None:
                return
            emits.append(event("content_block_stop", {"index": block_index}))
            open_block = None

        emits.append(
            event(
                "message_start",
                {
                    "message": {
                        "id": _MESSAGE_ID,
                        "type": "message",
                        "role": "assistant",
                        "model": model,
                        "content": [],
                        "stop_reason": None,
                        "stop_sequence": None,
                        "usage": {"input_tokens": input_tokens, "output_tokens": 1},
                    }
                },
            )
        )

        for item in script:
            if isinstance(item, Text):
                if open_block != "text":
                    close_block()
                    block_index += 1
                    open_block = "text"
                    emits.append(
                        event(
                            "content_block_start",
                            {
                                "index": block_index,
                                "content_block": {"type": "text", "text": ""},
                            },
                        )
                    )
                for part in split_text(item.text, item.pieces):
                    emits.append(
                        event(
                            "content_block_delta",
                            {
                                "index": block_index,
                                "delta": {"type": "text_delta", "text": part},
                            },
                        )
                    )
            elif isinstance(item, Reasoning):
                close_block()
                block_index += 1
                open_block = "thinking"
                emits.append(
                    event(
                        "content_block_start",
                        {
                            "index": block_index,
                            "content_block": {
                                "type": "thinking",
                                "thinking": "",
                                "signature": "",
                            },
                        },
                    )
                )
                emits.append(
                    event(
                        "content_block_delta",
                        {
                            "index": block_index,
                            "delta": {"type": "thinking_delta", "thinking": item.text},
                        },
                    )
                )
                emits.append(
                    event(
                        "content_block_delta",
                        {
                            "index": block_index,
                            "delta": {
                                "type": "signature_delta",
                                "signature": "fake-signature",
                            },
                        },
                    )
                )
            elif isinstance(item, ToolCall):
                close_block()
                block_index += 1
                open_block = "tool_use"
                emits.append(
                    event(
                        "content_block_start",
                        {
                            "index": block_index,
                            "content_block": {
                                "type": "tool_use",
                                "id": item.id,
                                "name": item.name,
                                "input": {},
                            },
                        },
                    )
                )
                for part in split_text(item.arguments, item.pieces):
                    emits.append(
                        event(
                            "content_block_delta",
                            {
                                "index": block_index,
                                "delta": {
                                    "type": "input_json_delta",
                                    "partial_json": part,
                                },
                            },
                        )
                    )
            elif isinstance(item, Usage):
                pass
            elif isinstance(item, Finish):
                close_block()
                emits.append(
                    event(
                        "message_delta",
                        {
                            "delta": {
                                "stop_reason": _STOP_REASONS.get(item.reason, item.reason),
                                "stop_sequence": None,
                            },
                            "usage": {
                                "input_tokens": input_tokens,
                                "output_tokens": output_tokens,
                            },
                        },
                    )
                )
                emits.append(event("message_stop", {}))
            elif isinstance(item, Raw):
                emits.append(item.data)
            elif isinstance(item, (Gate, Disconnect)):
                emits.append(item)
            else:  # pragma: no cover - the union is closed
                raise TypeError(f"unsupported script item {item!r}")

        return emits
