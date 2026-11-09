"""Shared App Server callbacks through the production provider boundary."""
import hashlib
import json
from fake_provider.scenario import Expect, Finish, Gate, Scenario, Step, Stream, Text, ToolCall, OPENAI_CHAT_COMPLETIONS


def tool_name(namespace: str) -> str:
    identity = json.dumps([namespace, "lookup"], separators=(",", ":")).encode()
    return "host_" + hashlib.sha256(identity).hexdigest()[:56]


def host_broadcast() -> Scenario:
    return Scenario(
        "app_server_host_broadcast",
        Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="chat-model", body_contains=("shared host request",), tools_include=(tool_name("business"), tool_name("inventory"))),
             Stream(Gate("host-call"), ToolCall("host-call-1", tool_name("business"), '{"key":"invoice"}'), Finish("tool_calls"))),
        Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="chat-model", body_contains=("host-call-1",), body_excludes=("late-result",)),
             Stream(Text("host continuation complete"), Finish())),
    )


def shared_control() -> Scenario:
    return Scenario(
        "app_server_shared_control",
        Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="chat-model", body_contains=("start-one",)),
             Stream(Gate("active-request"), Text("initial reply"), Finish())),
        Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="chat-model", body_contains=("start-two", "steer-after-queue"), body_excludes=("queued-next",)),
             Stream(Gate("steered-request"), Text("steered done"), Finish())),
        Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="chat-model", body_contains=("queued-next",)),
             Stream(Gate("queued-request"), Text("queued done"), Finish())),
    )

def host_crash() -> Scenario:
    return Scenario(
        "app_server_host_crash",
        Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="chat-model", body_contains=("crash-host",), tools_include=(tool_name("business"),)),
             Stream(ToolCall("uncertain-host-call",tool_name("business"),'{"key":"invoice"}'),Finish("tool_calls"))),
    )

SCENARIOS = {"app_server_host_broadcast": host_broadcast, "app_server_shared_control": shared_control, "app_server_host_crash":host_crash}


def measure_no_host() -> Scenario:
    return Scenario("app_server_measure_no_host", *[
        Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="chat-model", body_contains=("measure-input",)), Stream(Text("measured"), Finish()))
        for _ in range(20)
    ])


def measure_host() -> Scenario:
    steps = []
    for index in range(20):
        call_id = f"measure-call-{index}"
        steps.extend([
            Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="chat-model", body_contains=("measure-input",), tools_include=(tool_name("business"),)),
                 Stream(ToolCall(call_id, tool_name("business"), '{"key":"invoice"}'), Finish("tool_calls"))),
            Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="chat-model", body_contains=(call_id,)), Stream(Text("measured"), Finish())),
        ])
    return Scenario("app_server_measure_host", *steps)


SCENARIOS.update({"app_server_measure_no_host":measure_no_host,"app_server_measure_host":measure_host})
