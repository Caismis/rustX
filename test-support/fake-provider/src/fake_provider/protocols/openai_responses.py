"""The OpenAI Responses wire codec."""

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

_RESPONSE_ID = "resp_fake_provider"

#: Responses reports `incomplete` with a reason rather than a finish reason,
#: so the scenario's finish vocabulary maps onto the two terminal shapes.
_INCOMPLETE_REASONS = {"length": "max_output_tokens", "content_filter": "content_filter"}


class OpenAiResponsesCodec:
    """Encodes a response script as Responses streaming events.

    The emitted sequence is the **normal documented lifecycle**, not the
    subset any particular parser happens to accept. A codec that emitted only
    what rustX currently consumes would model the parser rather than the
    provider, and a gap in the parser would then be invisible here.

    A deliberately malformed or truncated sequence is expressed with `Raw`
    inside a `Stream`, or with a whole `RawResponse`; those bytes bypass this
    codec entirely.
    """

    name = "openai_responses"
    default_path = "/v1/responses"
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
        sequence = 0
        output_index = 0
        output: list[dict[str, Any]] = []
        # Responses reports usage on its terminal event rather than as a
        # separate frame, so a script that lists `Usage` after `Finish`
        # (the Chat Completions ordering) still reports it. Scenario
        # semantics stay protocol-neutral; only the encoding differs.
        usage: dict[str, Any] | None = next(
            (
                {
                    "input_tokens": item.input_tokens,
                    "output_tokens": item.output_tokens,
                    "total_tokens": item.input_tokens + item.output_tokens,
                }
                for item in script
                if isinstance(item, Usage)
            ),
            None,
        )
        # An open text item is kept open until something else starts, so
        # consecutive `Text` items stream into one assistant message exactly
        # as a real provider emits them.
        open_text: dict[str, Any] | None = None

        def event(payload: dict[str, Any]) -> bytes:
            nonlocal sequence
            payload = {**payload, "sequence_number": sequence}
            sequence += 1
            return sse(payload)

        def close_text() -> None:
            """Closes an open assistant message with the documented lifecycle.

            `output_text.done` -> `content_part.done` -> `output_item.done`,
            in that order. The middle event is part of the normal protocol,
            not an optional extra: emitting only the subset a particular
            parser happens to tolerate would make this codec a model of the
            parser rather than of the provider.
            """
            nonlocal open_text, output_index
            if open_text is None:
                return
            item_id = open_text["id"]
            text = open_text["text"]
            part = {"type": "output_text", "text": text, "annotations": []}
            emits.append(
                event(
                    {
                        "type": "response.output_text.done",
                        "item_id": item_id,
                        "output_index": open_text["index"],
                        "content_index": 0,
                        "text": text,
                        "annotations": [],
                    }
                )
            )
            emits.append(
                event(
                    {
                        "type": "response.content_part.done",
                        "item_id": item_id,
                        "output_index": open_text["index"],
                        "content_index": 0,
                        "part": part,
                    }
                )
            )
            item = {
                "id": item_id,
                "type": "message",
                "status": "completed",
                "role": "assistant",
                "content": [part],
            }
            emits.append(
                event(
                    {
                        "type": "response.output_item.done",
                        "output_index": open_text["index"],
                        "item": item,
                    }
                )
            )
            output.append(item)
            output_index += 1
            open_text = None

        emits.append(
            event(
                {
                    "type": "response.created",
                    "response": {
                        "id": _RESPONSE_ID,
                        "object": "response",
                        "status": "in_progress",
                        "model": model,
                        "output": [],
                        "usage": None,
                    },
                }
            )
        )

        for item in script:
            if isinstance(item, Text):
                if open_text is None:
                    item_id = f"msg_{output_index}"
                    open_text = {"id": item_id, "index": output_index, "text": ""}
                    emits.append(
                        event(
                            {
                                "type": "response.output_item.added",
                                "output_index": output_index,
                                "item": {
                                    "id": item_id,
                                    "type": "message",
                                    "status": "in_progress",
                                    "role": "assistant",
                                    "content": [],
                                },
                            }
                        )
                    )
                    emits.append(
                        event(
                            {
                                "type": "response.content_part.added",
                                "item_id": item_id,
                                "output_index": output_index,
                                "content_index": 0,
                                "part": {"type": "output_text", "text": "", "annotations": []},
                            }
                        )
                    )
                for part in split_text(item.text, item.pieces):
                    open_text["text"] += part
                    emits.append(
                        event(
                            {
                                "type": "response.output_text.delta",
                                "item_id": open_text["id"],
                                "output_index": open_text["index"],
                                "content_index": 0,
                                "delta": part,
                            }
                        )
                    )
            elif isinstance(item, Reasoning):
                close_text()
                item_id = f"rs_{output_index}"
                emits.append(
                    event(
                        {
                            "type": "response.output_item.added",
                            "output_index": output_index,
                            "item": {
                                "id": item_id,
                                "type": "reasoning",
                                "status": "in_progress",
                                "summary": [],
                            },
                        }
                    )
                )
                # The documented reasoning-summary lifecycle, in full:
                # part.added -> text.delta -> text.done -> part.done, then
                # the item's own done event.
                summary_part = {"type": "summary_text", "text": item.text}
                emits.append(
                    event(
                        {
                            "type": "response.reasoning_summary_part.added",
                            "item_id": item_id,
                            "output_index": output_index,
                            "summary_index": 0,
                            "part": {"type": "summary_text", "text": ""},
                        }
                    )
                )
                emits.append(
                    event(
                        {
                            "type": "response.reasoning_summary_text.delta",
                            "item_id": item_id,
                            "output_index": output_index,
                            "summary_index": 0,
                            "delta": item.text,
                        }
                    )
                )
                emits.append(
                    event(
                        {
                            "type": "response.reasoning_summary_text.done",
                            "item_id": item_id,
                            "output_index": output_index,
                            "summary_index": 0,
                            "text": item.text,
                        }
                    )
                )
                emits.append(
                    event(
                        {
                            "type": "response.reasoning_summary_part.done",
                            "item_id": item_id,
                            "output_index": output_index,
                            "summary_index": 0,
                            "part": summary_part,
                        }
                    )
                )
                done = {
                    "id": item_id,
                    "type": "reasoning",
                    "status": "completed",
                    "summary": [summary_part],
                    "encrypted_content": None,
                }
                emits.append(
                    event(
                        {
                            "type": "response.output_item.done",
                            "output_index": output_index,
                            "item": done,
                        }
                    )
                )
                output.append(done)
                output_index += 1
            elif isinstance(item, ToolCall):
                close_text()
                item_id = f"fc_{output_index}"
                parts = split_text(item.arguments, item.pieces)
                emits.append(
                    event(
                        {
                            "type": "response.output_item.added",
                            "output_index": output_index,
                            "item": {
                                "id": item_id,
                                "type": "function_call",
                                "status": "in_progress",
                                "call_id": item.id,
                                "name": item.name,
                                "arguments": "",
                            },
                        }
                    )
                )
                for part in parts:
                    emits.append(
                        event(
                            {
                                "type": "response.function_call_arguments.delta",
                                "item_id": item_id,
                                "output_index": output_index,
                                "delta": part,
                            }
                        )
                    )
                emits.append(
                    event(
                        {
                            "type": "response.function_call_arguments.done",
                            "item_id": item_id,
                            "output_index": output_index,
                            "arguments": item.arguments,
                        }
                    )
                )
                done = {
                    "id": item_id,
                    "type": "function_call",
                    "status": "completed",
                    "call_id": item.id,
                    "name": item.name,
                    "arguments": item.arguments,
                }
                emits.append(
                    event(
                        {
                            "type": "response.output_item.done",
                            "output_index": output_index,
                            "item": done,
                        }
                    )
                )
                output.append(done)
                output_index += 1
            elif isinstance(item, Usage):
                pass
            elif isinstance(item, Finish):
                close_text()
                reason = _INCOMPLETE_REASONS.get(item.reason)
                response: dict[str, Any] = {
                    "id": _RESPONSE_ID,
                    "object": "response",
                    "status": "incomplete" if reason else "completed",
                    "model": model,
                    "output": list(output),
                    "usage": usage,
                }
                if reason:
                    response["incomplete_details"] = {"reason": reason}
                emits.append(
                    event(
                        {
                            "type": "response.incomplete" if reason else "response.completed",
                            "response": response,
                        }
                    )
                )
            elif isinstance(item, Raw):
                emits.append(item.data)
            elif isinstance(item, (Gate, Disconnect)):
                emits.append(item)
            else:  # pragma: no cover - the union is closed
                raise TypeError(f"unsupported script item {item!r}")

        return emits
