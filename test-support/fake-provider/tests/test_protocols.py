"""Wire-format tests for every protocol codec.

Protocol coverage is claimed only where the encoded bytes are asserted. An
enum entry proves nothing; these tests decode the emitted frames and check
the exact event vocabulary each provider protocol actually uses.
"""

from __future__ import annotations

import json

from fake_provider.protocols import CODECS, codec_for, protocol_of_path
from fake_provider.scenario import (
    ANTHROPIC_MESSAGES,
    OPENAI_CHAT_COMPLETIONS,
    OPENAI_RESPONSES,
    Disconnect,
    Finish,
    Gate,
    Raw,
    Reasoning,
    Text,
    ToolCall,
    Usage,
)


def frames(emits) -> list[str]:
    """The decoded `data:` payloads of the byte emissions."""
    payloads: list[str] = []
    for emit in emits:
        if not isinstance(emit, bytes):
            continue
        for block in emit.decode().split("\n\n"):
            for line in block.splitlines():
                if line.startswith("data: "):
                    payloads.append(line[len("data: ") :])
    return payloads


def events(emits) -> list[dict]:
    return [json.loads(payload) for payload in frames(emits) if payload != "[DONE]"]


def test_every_protocol_has_a_distinct_path():
    paths = {codec.default_path for codec in CODECS.values()}
    assert len(paths) == len(CODECS)
    assert protocol_of_path("/v1/chat/completions") == OPENAI_CHAT_COMPLETIONS
    assert protocol_of_path("/v1/responses") == OPENAI_RESPONSES
    assert protocol_of_path("/v1/messages") == ANTHROPIC_MESSAGES
    assert protocol_of_path("/v1/unknown") is None


# -- OpenAI Chat Completions ------------------------------------------------


def test_chat_text_stream_is_a_chat_completion_chunk_sequence():
    codec = codec_for(OPENAI_CHAT_COMPLETIONS)
    emits = codec.encode([Text("Hello world", pieces=2), Finish("stop"), Usage(12, 3)], "m")
    payloads = frames(emits)
    assert payloads[-1] == "[DONE]"
    decoded = events(emits)
    assert decoded[0]["object"] == "chat.completion.chunk"
    assert decoded[0]["model"] == "m"
    assert decoded[0]["choices"][0]["delta"]["role"] == "assistant"
    assert "".join(
        chunk["choices"][0]["delta"].get("content") or ""
        for chunk in decoded
        if chunk["choices"]
    ) == "Hello world"
    assert decoded[-2]["choices"][0]["finish_reason"] == "stop"
    assert decoded[-1]["usage"] == {
        "prompt_tokens": 12,
        "completion_tokens": 3,
        "total_tokens": 15,
    }


def test_chat_tool_call_streams_identity_then_argument_deltas():
    codec = codec_for(OPENAI_CHAT_COMPLETIONS)
    emits = codec.encode(
        [ToolCall("call-1", "read", '{"path":"a.txt"}', pieces=3), Finish("tool_calls")], "m"
    )
    decoded = events(emits)
    first = decoded[0]["choices"][0]["delta"]["tool_calls"][0]
    assert first["id"] == "call-1"
    assert first["type"] == "function"
    assert first["function"]["name"] == "read"
    arguments = "".join(
        chunk["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"]
        for chunk in decoded
        if chunk["choices"] and "tool_calls" in chunk["choices"][0]["delta"]
    )
    assert arguments == '{"path":"a.txt"}'
    assert decoded[-1]["choices"][0]["finish_reason"] == "tool_calls"


def test_chat_reasoning_uses_the_reasoning_content_extension():
    codec = codec_for(OPENAI_CHAT_COMPLETIONS)
    decoded = events(codec.encode([Reasoning("plan"), Text("go"), Finish("stop")], "m"))
    assert decoded[0]["choices"][0]["delta"]["reasoning_content"] == "plan"


def test_chat_reports_the_requested_tool_names():
    codec = codec_for(OPENAI_CHAT_COMPLETIONS)
    body = {"tools": [{"type": "function", "function": {"name": "read"}}]}
    assert codec.tool_names(body) == ["read"]
    assert codec.tool_names({}) == []


# -- OpenAI Responses -------------------------------------------------------


def test_responses_text_stream_follows_the_documented_lifecycle_exactly():
    """The canonical normal ordering, asserted as a sequence.

    Presence checks would pass on a codec that skipped
    `response.content_part.done`; the whole point of this codec is that it
    emits the protocol, not the subset one parser tolerates.
    """
    codec = codec_for(OPENAI_RESPONSES)
    decoded = events(
        codec.encode([Text("Hello world", pieces=2), Finish("stop"), Usage(10, 5)], "m")
    )
    assert [event["type"] for event in decoded] == [
        "response.created",
        "response.output_item.added",
        "response.content_part.added",
        "response.output_text.delta",
        "response.output_text.delta",
        "response.output_text.done",
        "response.content_part.done",
        "response.output_item.done",
        "response.completed",
    ]
    assert [event["sequence_number"] for event in decoded] == list(range(len(decoded)))

    added = decoded[2]
    assert added["part"] == {"type": "output_text", "text": "", "annotations": []}
    assert (
        "".join(
            event["delta"]
            for event in decoded
            if event["type"] == "response.output_text.delta"
        )
        == "Hello world"
    )
    assert decoded[5]["text"] == "Hello world"
    done_part = decoded[6]
    assert done_part["item_id"] == added["item_id"]
    assert done_part["content_index"] == added["content_index"]
    assert done_part["part"] == {
        "type": "output_text",
        "text": "Hello world",
        "annotations": [],
    }
    item = decoded[7]["item"]
    assert item["status"] == "completed"
    assert item["content"] == [done_part["part"]]

    terminal = decoded[-1]["response"]
    assert terminal["status"] == "completed"
    assert terminal["usage"]["input_tokens"] == 10
    assert terminal["output"][0]["content"][0]["text"] == "Hello world"


def test_responses_reasoning_summary_follows_the_documented_lifecycle_exactly():
    codec = codec_for(OPENAI_RESPONSES)
    decoded = events(codec.encode([Reasoning("plan"), Text("go"), Finish("stop")], "m"))
    assert [event["type"] for event in decoded] == [
        "response.created",
        "response.output_item.added",
        "response.reasoning_summary_part.added",
        "response.reasoning_summary_text.delta",
        "response.reasoning_summary_text.done",
        "response.reasoning_summary_part.done",
        "response.output_item.done",
        "response.output_item.added",
        "response.content_part.added",
        "response.output_text.delta",
        "response.output_text.done",
        "response.content_part.done",
        "response.output_item.done",
        "response.completed",
    ]
    assert [event["sequence_number"] for event in decoded] == list(range(len(decoded)))

    reasoning_added = decoded[1]
    assert reasoning_added["item"]["type"] == "reasoning"
    assert reasoning_added["item"]["summary"] == []
    assert decoded[2]["part"] == {"type": "summary_text", "text": ""}
    assert decoded[2]["summary_index"] == 0
    assert decoded[3]["delta"] == "plan"
    assert decoded[4]["text"] == "plan"
    assert decoded[5]["part"] == {"type": "summary_text", "text": "plan"}
    reasoning_done = decoded[6]["item"]
    assert reasoning_done["status"] == "completed"
    assert reasoning_done["summary"] == [{"type": "summary_text", "text": "plan"}]
    # Every reasoning event names the same item, and the message that follows
    # is a distinct output item at the next index.
    for index in range(2, 6):
        assert decoded[index]["item_id"] == reasoning_added["item"]["id"]
        assert decoded[index]["output_index"] == 0
    assert decoded[7]["output_index"] == 1


def test_responses_function_call_follows_the_documented_lifecycle_exactly():
    codec = codec_for(OPENAI_RESPONSES)
    decoded = events(
        codec.encode(
            [ToolCall("call-9", "read", '{"path":"a"}', pieces=2), Finish("tool_calls")],
            "m",
        )
    )
    assert [event["type"] for event in decoded] == [
        "response.created",
        "response.output_item.added",
        "response.function_call_arguments.delta",
        "response.function_call_arguments.delta",
        "response.function_call_arguments.done",
        "response.output_item.done",
        "response.completed",
    ]
    added = decoded[1]
    assert added["item"]["type"] == "function_call"
    assert added["item"]["call_id"] == "call-9"
    assert added["item"]["arguments"] == ""
    assert (
        "".join(
            event["delta"]
            for event in decoded
            if event["type"] == "response.function_call_arguments.delta"
        )
        == '{"path":"a"}'
    )
    assert decoded[4]["arguments"] == '{"path":"a"}'
    assert decoded[5]["item"]["arguments"] == '{"path":"a"}'
    assert decoded[5]["item"]["status"] == "completed"


def test_responses_maps_a_length_finish_to_an_incomplete_response():
    codec = codec_for(OPENAI_RESPONSES)
    decoded = events(codec.encode([Text("partial"), Finish("length")], "m"))
    assert decoded[-1]["type"] == "response.incomplete"
    assert decoded[-1]["response"]["incomplete_details"]["reason"] == "max_output_tokens"


# -- Anthropic Messages -----------------------------------------------------


def test_anthropic_text_stream_uses_named_sse_events():
    codec = codec_for(ANTHROPIC_MESSAGES)
    emits = codec.encode([Text("Hello world", pieces=2), Finish("stop"), Usage(25, 15)], "m")
    raw = b"".join(emit for emit in emits if isinstance(emit, bytes)).decode()
    named = [line[len("event: ") :] for line in raw.splitlines() if line.startswith("event: ")]
    assert named[0] == "message_start"
    assert named[-1] == "message_stop"
    assert named.count("content_block_delta") == 2
    decoded = events(emits)
    assert decoded[0]["message"]["model"] == "m"
    assert "".join(
        event["delta"]["text"]
        for event in decoded
        if event["type"] == "content_block_delta"
    ) == "Hello world"
    delta = next(event for event in decoded if event["type"] == "message_delta")
    assert delta["delta"]["stop_reason"] == "end_turn"
    assert delta["usage"] == {"input_tokens": 25, "output_tokens": 15}


def test_anthropic_tool_use_streams_input_json_deltas():
    codec = codec_for(ANTHROPIC_MESSAGES)
    decoded = events(
        codec.encode(
            [ToolCall("toolu_1", "read", '{"path":"a"}', pieces=2), Finish("tool_calls")], "m"
        )
    )
    start = next(event for event in decoded if event["type"] == "content_block_start")
    assert start["content_block"] == {
        "type": "tool_use",
        "id": "toolu_1",
        "name": "read",
        "input": {},
    }
    assert "".join(
        event["delta"]["partial_json"]
        for event in decoded
        if event["type"] == "content_block_delta"
    ) == '{"path":"a"}'
    delta = next(event for event in decoded if event["type"] == "message_delta")
    assert delta["delta"]["stop_reason"] == "tool_use"


def test_anthropic_thinking_emits_a_signed_thinking_block():
    codec = codec_for(ANTHROPIC_MESSAGES)
    decoded = events(codec.encode([Reasoning("plan"), Text("go"), Finish("stop")], "m"))
    start = next(event for event in decoded if event["type"] == "content_block_start")
    assert start["content_block"]["type"] == "thinking"
    kinds = [
        event["delta"]["type"] for event in decoded if event["type"] == "content_block_delta"
    ]
    assert kinds[:2] == ["thinking_delta", "signature_delta"]


# -- The raw-wire escape hatch ---------------------------------------------


def test_raw_bytes_and_control_items_pass_through_every_codec():
    for name in CODECS:
        emits = codec_for(name).encode(
            [Raw(b"data: {not json\n\n"), Gate("g"), Disconnect()], "m"
        )
        assert b"data: {not json\n\n" in emits
        assert Gate("g") in emits
        assert Disconnect() in emits
