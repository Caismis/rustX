"""The provider selector accepts concurrency without weakening its gates."""
import json

import pytest

from fake_provider.scenario import Gate, HttpError, RecordedRequest
from fake_provider.scenarios.asynchronous_compaction import asynchronous_compaction


def request(model, body=""):
    return RecordedRequest(index=0, method="POST", path="/v1/chat/completions",
        protocol="openai_chat_completions", model=model, headers={}, credential_headers=[],
        body_text=body, json={"model": model, "messages": [{"content": body}]})


def test_concurrent_lanes_select_independent_gates_and_one_native_fallback():
    scenario = asynchronous_compaction()
    history = [request("chat-model")] * 14
    native = request("summary-model")
    step, final = scenario.select([*history, native])
    assert not final and isinstance(step.respond, HttpError)
    assert step.expect.json_subset["response_format"]["type"] == "json_schema"
    primary = request("chat-model")
    prompt_first, final = scenario.select([*history, native, native])
    assert not final and prompt_first.respond.script[0] == Gate("async-summary")
    primary_after_prompt, final = scenario.select([*history, native, native, primary])
    assert not final and primary_after_prompt.respond.script[0] == Gate("async-primary")
    first_primary, final = scenario.select([*history, native, primary])
    assert not final and first_primary.respond.script[0] == Gate("async-primary")
    prompt, final = scenario.select([*history, native, primary, native])
    assert not final and prompt.respond.script[0] == Gate("async-summary")
    assert '"response_format"' in prompt.expect.body_excludes
    second = request("second-model", "async: cancelled later user tail")
    second_step, final = scenario.select([*history, native, primary, native, second])
    assert not final and second_step.allow_disconnect
    assert second_step.respond.script[0] == Gate("async-next-primary")
    final_request = request("second-model", "async: cancelled later user tail async: fresh final user tail")
    final_step, final = scenario.select([*history, native, primary, native, second, final_request])
    assert final and "chunked-call-001" in final_step.expect.body_excludes


def test_selector_rejects_unbounded_or_early_summary_work():
    scenario = asynchronous_compaction()
    with pytest.raises(ValueError, match="before sufficient"):
        scenario.select([request("chat-model"), request("summary-model")])
    with pytest.raises(ValueError, match="request bound"):
        scenario.select([request("chat-model")] * 321)
