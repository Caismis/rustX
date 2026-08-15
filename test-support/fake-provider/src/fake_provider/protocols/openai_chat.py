"""The OpenAI Chat Completions wire codec."""

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

_ID = "chatcmpl-fake-provider"


class OpenAiChatCompletionsCodec:
    """Encodes a response script as Chat Completions stream chunks."""

    name = "openai_chat_completions"
    default_path = "/v1/chat/completions"
    content_type = "text/event-stream"

    def model_of(self, body: dict[str, Any]) -> str | None:
        model = body.get("model")
        return model if isinstance(model, str) else None

    def tool_names(self, body: dict[str, Any]) -> list[str]:
        names: list[str] = []
        for tool in body.get("tools") or []:
            function = tool.get("function") if isinstance(tool, dict) else None
            if isinstance(function, dict) and isinstance(function.get("name"), str):
                names.append(function["name"])
        return names

    def encode(self, script: Iterable[Chunk], model: str) -> list[Emit]:
        emits: list[Emit] = []
        started = False
        tool_index = -1
        open_tool: str | None = None
        terminated = False

        def chunk(delta: dict[str, Any], finish: str | None = None) -> bytes:
            nonlocal started
            if not started and "role" not in delta:
                delta = {"role": "assistant", **delta}
            started = True
            return sse(
                {
                    "id": _ID,
                    "object": "chat.completion.chunk",
                    "created": 0,
                    "model": model,
                    "choices": [
                        {"index": 0, "delta": delta, "finish_reason": finish}
                    ],
                }
            )

        for item in script:
            if isinstance(item, Text):
                emits.extend(chunk({"content": part}) for part in split_text(item.text, item.pieces))
            elif isinstance(item, Reasoning):
                emits.append(chunk({"content": "", "reasoning_content": item.text}))
            elif isinstance(item, ToolCall):
                if open_tool != item.id:
                    tool_index += 1
                    open_tool = item.id
                    parts = split_text(item.arguments, item.pieces)
                    emits.append(
                        chunk(
                            {
                                "content": None,
                                "tool_calls": [
                                    {
                                        "index": tool_index,
                                        "id": item.id,
                                        "type": "function",
                                        "function": {
                                            "name": item.name,
                                            "arguments": parts[0],
                                        },
                                    }
                                ],
                            }
                        )
                    )
                    rest = parts[1:]
                else:
                    rest = split_text(item.arguments, item.pieces)
                emits.extend(
                    chunk(
                        {
                            "tool_calls": [
                                {
                                    "index": tool_index,
                                    "function": {"arguments": part},
                                }
                            ]
                        }
                    )
                    for part in rest
                )
            elif isinstance(item, Finish):
                emits.append(chunk({}, finish=item.reason))
                terminated = True
            elif isinstance(item, Usage):
                emits.append(
                    sse(
                        {
                            "id": _ID,
                            "object": "chat.completion.chunk",
                            "created": 0,
                            "model": model,
                            "choices": [],
                            "usage": {
                                "prompt_tokens": item.input_tokens,
                                "completion_tokens": item.output_tokens,
                                "total_tokens": item.input_tokens + item.output_tokens,
                            },
                        }
                    )
                )
            elif isinstance(item, Raw):
                emits.append(item.data)
            elif isinstance(item, (Gate, Disconnect)):
                emits.append(item)
            else:  # pragma: no cover - the union is closed
                raise TypeError(f"unsupported script item {item!r}")

        if terminated:
            emits.append(sse("[DONE]"))
        return emits
