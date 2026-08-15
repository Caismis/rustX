"""The scenario model: expectations, ordering, and the registry."""

from __future__ import annotations

import pytest

from fake_provider.control import ScenarioRun, declared_gates
from fake_provider.protocols import codec_for
from fake_provider.scenario import (
    OPENAI_CHAT_COMPLETIONS,
    Expect,
    Finish,
    Gate,
    RecordedRequest,
    Scenario,
    Step,
    Stream,
    Text,
    sanitize_headers,
    split_text,
)
from fake_provider.scenarios import SCENARIOS, build


def recorded(body: dict, path: str = "/v1/chat/completions") -> RecordedRequest:
    return RecordedRequest(
        index=0,
        method="POST",
        path=path,
        protocol=OPENAI_CHAT_COMPLETIONS,
        model=body.get("model"),
        headers={"content-type": "application/json"},
        credential_headers=["authorization"],
        body_text=str(body),
        json=body,
    )


CODEC = codec_for(OPENAI_CHAT_COMPLETIONS)


def test_a_matching_request_has_no_failures():
    expect = Expect(
        protocol=OPENAI_CHAT_COMPLETIONS,
        model="m",
        json_subset={"stream": True},
        tools_include=("read",),
    )
    body = {
        "model": "m",
        "stream": True,
        "temperature": 0.5,
        "tools": [{"type": "function", "function": {"name": "read"}}],
    }
    assert expect.failures(recorded(body), CODEC) == []


def test_every_mismatch_is_reported_explicitly():
    expect = Expect(
        protocol=OPENAI_CHAT_COMPLETIONS,
        model="m",
        path="/v1/chat/completions",
        json_subset={"stream": True},
        tools_include=("read",),
        body_contains=("needle",),
    )
    failures = expect.failures(recorded({"model": "other", "stream": False}), CODEC)
    assert any("model" in failure for failure in failures)
    assert any("stream" in failure for failure in failures)
    assert any("tools" in failure for failure in failures)
    assert any("needle" in failure for failure in failures)


def test_subset_matching_is_open_on_objects_and_closed_on_arrays():
    expect = Expect(protocol=OPENAI_CHAT_COMPLETIONS, json_subset={"a": {"b": 1}})
    assert expect.failures(recorded({"a": {"b": 1, "c": 2}}), CODEC) == []
    expect = Expect(protocol=OPENAI_CHAT_COMPLETIONS, json_subset={"a": [1]})
    assert expect.failures(recorded({"a": [1, 2]}), CODEC) != []


def test_exact_matching_rejects_unexpected_top_level_keys():
    expect = Expect(protocol=OPENAI_CHAT_COMPLETIONS, json_exact={"model": "m"})
    assert expect.failures(recorded({"model": "m"}), CODEC) == []
    failures = expect.failures(recorded({"model": "m", "stream": True}), CODEC)
    assert any("unexpected top-level keys" in failure for failure in failures)


def test_a_summary_request_is_identified_by_the_absence_of_tools():
    expect = Expect(protocol=OPENAI_CHAT_COMPLETIONS, no_tools=True)
    assert expect.failures(recorded({"model": "m"}), CODEC) == []
    body = {"model": "m", "tools": [{"type": "function", "function": {"name": "read"}}]}
    assert expect.failures(recorded(body), CODEC) != []


def test_a_wrong_path_fails_even_when_the_body_matches():
    expect = Expect(protocol=OPENAI_CHAT_COMPLETIONS)
    failures = expect.failures(recorded({"model": "m"}, path="/v1/responses"), CODEC)
    assert any("path" in failure for failure in failures)


def test_credentials_are_recorded_by_name_only():
    recorded_headers, credentials = sanitize_headers(
        {
            "authorization": "Bearer super-secret",
            "x-api-key": "another-secret",
            "content-type": "application/json",
        }
    )
    assert recorded_headers == {"content-type": "application/json"}
    assert credentials == ["authorization", "x-api-key"]
    assert "super-secret" not in str(recorded_headers) + str(credentials)


def test_split_text_is_deterministic():
    assert split_text("abcdef", 3) == ["ab", "cd", "ef"]
    assert split_text("abc", 1) == ["abc"]
    assert split_text("", 4) == [""]


def test_duplicate_gate_names_are_rejected_at_load():
    scenario = Scenario(
        "duplicate",
        Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS), Stream(Gate("g"), Finish())),
        Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS), Stream(Gate("g"), Finish())),
    )
    with pytest.raises(ValueError):
        declared_gates(scenario)


def test_a_fresh_run_is_not_ok_until_every_step_is_consumed():
    scenario = Scenario(
        "one-step",
        Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS), Stream(Text("x"), Finish())),
    )
    run = ScenarioRun(scenario)
    assert not run.ok
    assert run.report()["unconsumedSteps"] == [0]
    run.steps_consumed = 1
    assert run.ok
    assert run.report()["unconsumedSteps"] == []


def test_every_registered_scenario_builds():
    for name in SCENARIOS:
        scenario = build(name)
        assert scenario.name == name
        assert scenario.steps
        declared_gates(scenario)


def test_an_unknown_scenario_name_fails_loudly():
    with pytest.raises(SystemExit):
        build("no-such-scenario")
