"""The server: ordered progression, gates, disconnects, and the report."""

from __future__ import annotations

import asyncio
import json

import conftest
from fake_provider import control
from fake_provider.control import ScenarioRun
from fake_provider.scenario import (
    OPENAI_CHAT_COMPLETIONS,
    Disconnect,
    Expect,
    Finish,
    Gate,
    HttpError,
    RawResponse,
    Raw,
    Scenario,
    Step,
    Stream,
    Text,
)
from fake_provider.server import ProviderServer

CHAT = "/v1/chat/completions"


class Harness:
    """A running scenario on an ephemeral loopback port."""

    def __init__(
        self, run: ScenarioRun, server: ProviderServer, host: str, port: int
    ) -> None:
        self.run = run
        self.server = server
        self.host = host
        self.port = port

    async def post(self, path: str, payload: dict) -> conftest.Reply:
        return await conftest.request(
            self.host,
            self.port,
            "POST",
            path,
            json.dumps(payload).encode(),
            {"Content-Type": "application/json", "Authorization": "Bearer secret"},
        )

    async def control(self, method: str, path: str) -> conftest.Reply:
        return await conftest.request(self.host, self.port, method, f"/__control{path}")


async def serve(scenario: Scenario) -> Harness:
    run = ScenarioRun(scenario)
    server = ProviderServer(run)
    host, port = await server.start("127.0.0.1", 0)
    return Harness(run, server, host, port)


def chat_step(**expect) -> Step:
    return Step(
        Expect(protocol=OPENAI_CHAT_COMPLETIONS, **expect),
        Stream(Text("ok"), Finish("stop")),
    )


# -- ordered progression ---------------------------------------------------


def test_steps_are_consumed_in_order_and_the_scenario_completes():
    async def scenario() -> None:
        harness = await serve(
            Scenario(
                "ordered",
                chat_step(body_contains=("first",)),
                chat_step(body_contains=("second",)),
            )
        )
        first = await harness.post(CHAT, {"model": "m", "messages": ["first"]})
        assert first.status == 200
        assert "data: [DONE]" in first.text
        second = await harness.post(CHAT, {"model": "m", "messages": ["second"]})
        assert second.status == 200
        state = (await harness.control("GET", "/state")).json
        assert state["stepsMatched"] == 2
        assert state["stepsSettled"] == 2
        assert state["stepStates"] == ["settled", "settled"]
        assert state["requestCount"] == 2
        assert state["ok"] is True
        assert harness.run.report()["unsettledSteps"] == []
        # The completion observation is published only once every scripted
        # response has actually finished.
        assert [
            observation.kind for observation in harness.run.observations
        ].count(control.SCENARIO_COMPLETED) == 1

    asyncio.run(scenario())


def test_a_request_out_of_order_fails_the_scenario():
    async def scenario() -> None:
        harness = await serve(
            Scenario(
                "ordered",
                chat_step(body_contains=("first",)),
                chat_step(body_contains=("second",)),
            )
        )
        reply = await harness.post(CHAT, {"model": "m", "messages": ["second"]})
        assert reply.status == 500
        assert reply.json["error"]["type"] == "scenario_error"
        assert harness.run.ok is False
        assert any("'first'" in failure["detail"] for failure in harness.run.failures)

    asyncio.run(scenario())


def test_an_extra_request_fails_by_default():
    async def scenario() -> None:
        harness = await serve(Scenario("one", chat_step()))
        assert (await harness.post(CHAT, {"model": "m"})).status == 200
        extra = await harness.post(CHAT, {"model": "m"})
        assert extra.status == 500
        assert harness.run.ok is False
        assert "unexpected provider request #2" in harness.run.failures[0]["detail"]

    asyncio.run(scenario())


def test_an_unrequested_step_fails_the_report():
    async def scenario() -> None:
        harness = await serve(Scenario("two", chat_step(), chat_step()))
        await harness.post(CHAT, {"model": "m"})
        report = harness.run.report()
        assert report["ok"] is False
        assert report["unsettledSteps"] == [{"index": 1, "state": "pending"}]
        assert control.SCENARIO_COMPLETED not in [
            observation.kind for observation in harness.run.observations
        ]

    asyncio.run(scenario())


def test_a_step_still_holding_at_a_gate_is_matched_but_not_settled():
    """The regression for the completion-accounting defect.

    The final expected request arrives and matches, its response stops at an
    unreleased gate, and shutdown is called while it is still held. Matching
    the request is not finishing the response, so the report must not claim
    success.
    """

    async def scenario() -> None:
        harness = await serve(
            Scenario(
                "gated-shutdown",
                Step(
                    Expect(protocol=OPENAI_CHAT_COMPLETIONS),
                    Stream(Text("before"), Gate("g"), Text("after"), Finish("stop")),
                ),
            )
        )
        reader, writer = await conftest.open_stream(
            harness.host, harness.port, CHAT, b'{"model":"m"}'
        )
        try:
            reached = await harness.control(
                "GET", "/observations/await?kind=gate_reached&name=g&timeoutMs=5000"
            )
            assert reached.status == 200

            # The request matched — progression — but the response is still
            # suspended, so the step has not settled.
            state = (await harness.control("GET", "/state")).json
            assert state["requestCount"] == 1
            assert state["stepsMatched"] == 1
            assert state["stepsSettled"] == 0
            assert state["stepStates"] == ["matched"]
            assert state["complete"] is False
            assert state["ok"] is False

            report = (await harness.control("POST", "/shutdown")).json
            assert report["ok"] is False, report
            assert report["unsettledSteps"] == [{"index": 0, "state": "matched"}]
            assert control.SCENARIO_COMPLETED not in [
                observation["kind"] for observation in report["observations"]
            ]
            assert control.RESPONSE_COMPLETED not in [
                observation["kind"] for observation in report["observations"]
            ]
        finally:
            writer.close()
            del reader

    asyncio.run(scenario())


def test_shutdown_completes_even_while_a_response_is_gated():
    """A suspended response must never hold the process open."""

    async def scenario() -> None:
        harness = await serve(
            Scenario(
                "gated-shutdown",
                Step(
                    Expect(protocol=OPENAI_CHAT_COMPLETIONS),
                    Stream(Text("before"), Gate("g"), Finish("stop")),
                ),
            )
        )
        _, writer = await conftest.open_stream(
            harness.host, harness.port, CHAT, b'{"model":"m"}'
        )
        try:
            await harness.control("GET", "/observations/await?kind=gate_reached&name=g")
            await harness.control("POST", "/shutdown")
            await asyncio.wait_for(harness.server.serve_until_shutdown(), 5)
            assert harness.run.ok is False
        finally:
            writer.close()

    asyncio.run(scenario())


def test_a_model_mismatch_is_reported_with_the_failing_field():
    async def scenario() -> None:
        harness = await serve(Scenario("model", chat_step(model="expected")))
        reply = await harness.post(CHAT, {"model": "actual"})
        assert reply.status == 500
        assert any("model" in failure for failure in reply.json["error"]["failures"])

    asyncio.run(scenario())


# -- request observation ---------------------------------------------------


def test_requests_are_recorded_in_arrival_order_without_credential_values():
    async def scenario() -> None:
        harness = await serve(Scenario("record", chat_step(), chat_step()))
        await harness.post(CHAT, {"model": "m", "n": 1})
        await harness.post(CHAT, {"model": "m", "n": 2})
        recorded = (await harness.control("GET", "/requests")).json["requests"]
        assert [entry["index"] for entry in recorded] == [0, 1]
        assert [entry["body"]["n"] for entry in recorded] == [1, 2]
        assert recorded[0]["protocol"] == OPENAI_CHAT_COMPLETIONS
        assert recorded[0]["model"] == "m"
        assert recorded[0]["credentialHeaders"] == ["authorization"]
        assert "secret" not in json.dumps(recorded)

    asyncio.run(scenario())


# -- gates and disconnects -------------------------------------------------


def test_a_gate_suspends_the_stream_until_the_control_api_releases_it():
    async def scenario() -> None:
        harness = await serve(
            Scenario(
                "gated",
                Step(
                    Expect(protocol=OPENAI_CHAT_COMPLETIONS),
                    Stream(Text("before"), Gate("g"), Text("after"), Finish("stop")),
                ),
            )
        )
        reader, writer = await conftest.open_stream(
            harness.host, harness.port, CHAT, b'{"model":"m"}'
        )
        try:
            reached = await harness.control(
                "GET", "/observations/await?kind=gate_reached&name=g&timeoutMs=5000"
            )
            assert reached.status == 200
            assert reached.json["name"] == "g"
            state = (await harness.control("GET", "/state")).json
            assert state["gates"] == {"g": control.GATE_REACHED_STATE}

            # Everything before the gate is already on the wire; nothing after
            # it can be, which is exactly what makes the ordering provable.
            prefix = await asyncio.wait_for(reader.read(4096), 2)
            assert b"before" in prefix
            assert b"after" not in prefix

            released = await harness.control("POST", "/gates/g/release")
            assert released.json["state"] == control.GATE_RELEASED
            rest = await asyncio.wait_for(reader.read(), 5)
            assert b"after" in rest
            assert harness.run.ok is True
        finally:
            writer.close()

    asyncio.run(scenario())


def test_releasing_an_undeclared_gate_is_an_explicit_error():
    async def scenario() -> None:
        harness = await serve(Scenario("no-gates", chat_step()))
        reply = await harness.control("POST", "/gates/absent/release")
        assert reply.status == 404

    asyncio.run(scenario())


def test_a_client_disconnect_is_observable_and_fails_an_unprepared_step():
    async def scenario() -> None:
        harness = await serve(
            Scenario(
                "abandoned",
                Step(
                    Expect(protocol=OPENAI_CHAT_COMPLETIONS),
                    Stream(Text("before"), Gate("g"), Text("after"), Finish("stop")),
                ),
            )
        )
        reader, writer = await conftest.open_stream(
            harness.host, harness.port, CHAT, b'{"model":"m"}'
        )
        await harness.control("GET", "/observations/await?kind=gate_reached&name=g")
        writer.close()
        observed = await harness.control(
            "GET", "/observations/await?kind=client_disconnected&timeoutMs=5000"
        )
        assert observed.status == 200
        await harness.control("POST", "/gates/g/release")
        failure = await harness.control(
            "GET", "/observations/await?kind=assertion_failed&timeoutMs=5000"
        )
        assert failure.status == 200
        assert harness.run.ok is False
        del reader

    asyncio.run(scenario())


def test_an_allowed_disconnect_keeps_the_scenario_green():
    async def scenario() -> None:
        harness = await serve(
            Scenario(
                "cancellable",
                Step(
                    Expect(protocol=OPENAI_CHAT_COMPLETIONS),
                    Stream(Text("before"), Gate("g"), Text("after"), Finish("stop")),
                    allow_disconnect=True,
                ),
            )
        )
        _, writer = await conftest.open_stream(
            harness.host, harness.port, CHAT, b'{"model":"m"}'
        )
        await harness.control("GET", "/observations/await?kind=gate_reached&name=g")
        writer.close()
        await harness.control("GET", "/observations/await?kind=client_disconnected")
        await harness.control("POST", "/gates/g/release")
        # The release lets the response task finish. An abandoned stream is
        # this step's intended terminal state, so it settles rather than
        # failing, and the scenario is genuinely complete.
        settled = await harness.control(
            "GET", "/observations/await?kind=response_completed&timeoutMs=5000"
        )
        assert settled.status == 200
        assert settled.json["detail"]["terminal"] in (
            control.TERMINAL_CLIENT_DISCONNECT,
            control.TERMINAL_SCRIPT_COMPLETE,
        )
        assert harness.run.failures == []
        assert harness.run.ok is True

    asyncio.run(scenario())


def test_awaiting_an_observation_that_never_happens_times_out_explicitly():
    async def scenario() -> None:
        harness = await serve(Scenario("quiet", chat_step()))
        reply = await harness.control(
            "GET", "/observations/await?kind=gate_reached&timeoutMs=50"
        )
        assert reply.status == 504
        assert reply.json["state"]["requestCount"] == 0

    asyncio.run(scenario())


# -- response shapes -------------------------------------------------------


def test_terminal_reasons_are_recorded_per_response_kind():
    """Each response kind settles for its own documented reason."""

    async def scenario() -> None:
        harness = await serve(
            Scenario(
                "terminals",
                chat_step(),
                Step(
                    Expect(protocol=OPENAI_CHAT_COMPLETIONS),
                    HttpError(500, {"error": {"message": "boom"}}),
                ),
                Step(
                    Expect(protocol=OPENAI_CHAT_COMPLETIONS),
                    Stream(Text("cut"), Disconnect(), Text("never"), Finish("stop")),
                ),
                Step(
                    Expect(protocol=OPENAI_CHAT_COMPLETIONS),
                    RawResponse(
                        headers={"Content-Type": "text/event-stream"},
                        script=(Raw(b"data: raw\n\n"),),
                    ),
                ),
            )
        )
        for _ in range(4):
            try:
                await harness.post(CHAT, {"model": "m"})
            except (ConnectionResetError, asyncio.IncompleteReadError):
                pass  # the scripted disconnect tears its own connection down
        terminals = [
            observation.detail["terminal"]
            for observation in harness.run.observations
            if observation.kind == control.RESPONSE_COMPLETED
        ]
        assert terminals == [
            control.TERMINAL_SCRIPT_COMPLETE,
            control.TERMINAL_HTTP_ERROR,
            control.TERMINAL_SCRIPTED_DISCONNECT,
            control.TERMINAL_SCRIPT_COMPLETE,
        ]
        assert harness.run.ok is True

    asyncio.run(scenario())


def test_an_http_error_step_serves_the_scripted_provider_error():
    async def scenario() -> None:
        harness = await serve(
            Scenario(
                "failing",
                Step(
                    Expect(protocol=OPENAI_CHAT_COMPLETIONS),
                    HttpError(429, {"error": {"message": "slow down"}}),
                ),
            )
        )
        reply = await harness.post(CHAT, {"model": "m"})
        assert reply.status == 429
        assert reply.json["error"]["message"] == "slow down"
        assert harness.run.ok is True

    asyncio.run(scenario())


def test_a_raw_response_writes_bytes_verbatim():
    async def scenario() -> None:
        harness = await serve(
            Scenario(
                "raw",
                Step(
                    Expect(protocol=OPENAI_CHAT_COMPLETIONS),
                    RawResponse(
                        status=200,
                        headers={"Content-Type": "text/event-stream"},
                        script=(Raw(b"data: {this is not json}\n\n"),),
                    ),
                ),
            )
        )
        reply = await harness.post(CHAT, {"model": "m"})
        assert reply.text == "data: {this is not json}\n\n"
        assert harness.run.ok is True

    asyncio.run(scenario())


# -- the control surface ---------------------------------------------------


def test_shutdown_returns_the_report_and_stops_the_server():
    async def scenario() -> None:
        harness = await serve(Scenario("one", chat_step()))
        await harness.post(CHAT, {"model": "m"})
        reply = await harness.control("POST", "/shutdown")
        assert reply.json["ok"] is True
        assert reply.json["stepsSettled"] == 1
        assert harness.run.shutdown_requested.is_set()

    asyncio.run(scenario())


def test_an_unknown_control_route_is_rejected():
    async def scenario() -> None:
        harness = await serve(Scenario("one", chat_step()))
        assert (await harness.control("GET", "/nope")).status == 404

    asyncio.run(scenario())
