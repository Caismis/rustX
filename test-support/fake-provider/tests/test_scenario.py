"""The scenario model: expectations, ordering, and the registry."""

from __future__ import annotations

import asyncio

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


async def _run_states() -> None:
    """A matched request is progression; only a settled response is success."""
    scenario = Scenario(
        "one-step",
        Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS), Stream(Text("x"), Finish())),
    )
    run = ScenarioRun(scenario)
    assert not run.ok
    assert run.step_states == ["pending"]
    assert run.report()["unsettledSteps"] == [{"index": 0, "state": "pending"}]

    run.match_step(0)
    assert run.steps_matched == 1
    assert run.steps_settled == 0
    assert not run.ok, "a matched request is not a completed scenario"
    assert run.report()["unsettledSteps"] == [{"index": 0, "state": "matched"}]

    await run.settle_step(0, "script_complete")
    assert run.steps_settled == 1
    assert run.all_settled
    assert run.ok
    assert run.report()["unsettledSteps"] == []


def test_a_matched_step_is_not_a_settled_step():
    asyncio.run(_run_states())


def one_step_run(name: str = "one-step") -> ScenarioRun:
    return ScenarioRun(
        Scenario(
            name,
            Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS), Stream(Text("x"), Finish())),
        )
    )


async def _failed_is_terminal() -> None:
    """A failed step cannot settle — proven by attempting it."""
    run = one_step_run()
    run.fail_step(0)
    assert run.step_states == ["failed"]

    with pytest.raises(RuntimeError) as rejected:
        await run.settle_step(0, "script_complete")
    message = str(rejected.value)
    assert "step 0" in message
    assert "'failed'" in message and "'settled'" in message

    # The rejection changed nothing: not the state, not the report, and not
    # the observation stream a driver waits on.
    assert run.step_states == ["failed"]
    assert not run.ok
    assert run.report()["unsettledSteps"] == [{"index": 0, "state": "failed"}]
    assert [observation.kind for observation in run.observations] == []


def test_a_failed_step_can_never_settle():
    asyncio.run(_failed_is_terminal())


async def _settled_is_terminal() -> None:
    """Every transition out of `settled` is refused."""
    run = one_step_run()
    run.match_step(0)
    await run.settle_step(0, "script_complete")
    assert run.step_states == ["settled"]
    settlements = len(run.observations)

    with pytest.raises(RuntimeError):
        await run.settle_step(0, "script_complete")
    with pytest.raises(RuntimeError):
        run.match_step(0)
    with pytest.raises(RuntimeError):
        run.fail_step(0)

    assert run.step_states == ["settled"]
    assert run.ok, "a rejected transition cannot unsettle a settled step"
    assert len(run.observations) == settlements, "no second response_completed"


def test_a_settled_step_is_terminal():
    asyncio.run(_settled_is_terminal())


async def _settlement_requires_a_match() -> None:
    """Settling is only ever the completion of a matched request."""
    run = one_step_run()
    with pytest.raises(RuntimeError) as rejected:
        await run.settle_step(0, "script_complete")
    assert "'pending'" in str(rejected.value)
    assert run.step_states == ["pending"]
    assert [observation.kind for observation in run.observations] == []


def test_a_pending_step_cannot_settle():
    asyncio.run(_settlement_requires_a_match())


def test_matching_is_one_shot():
    run = one_step_run()
    run.match_step(0)
    with pytest.raises(RuntimeError) as rejected:
        run.match_step(0)
    assert "'matched'" in str(rejected.value)
    assert run.step_states == ["matched"]


def test_both_failure_paths_stay_legal():
    """`fail_step` is legal from either non-terminal state.

    These are the emulator's two real failure call sites: a request that did
    not match its step, and a response that could not finish.
    """
    unmatched = one_step_run()
    unmatched.fail_step(0)
    assert unmatched.step_states == ["failed"]

    abandoned = one_step_run()
    abandoned.match_step(0)
    abandoned.fail_step(0)
    assert abandoned.step_states == ["failed"]


def test_an_out_of_range_step_index_is_rejected():
    """A negative index must not silently mutate a different step."""
    run = ScenarioRun(
        Scenario(
            "two-step",
            Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS), Stream(Text("x"), Finish())),
            Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS), Stream(Text("y"), Finish())),
        )
    )
    for index in (-1, 2):
        with pytest.raises(IndexError):
            run.match_step(index)
    assert run.step_states == ["pending", "pending"]


def test_every_registered_scenario_builds():
    for name in SCENARIOS:
        scenario = build(name)
        assert scenario.name == name
        assert scenario.steps
        declared_gates(scenario)


def test_an_unknown_scenario_name_fails_loudly():
    with pytest.raises(SystemExit):
        build("no-such-scenario")
