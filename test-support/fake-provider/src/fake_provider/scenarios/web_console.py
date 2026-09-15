"""Bounded browser integration script. Gates control provider emission only.

No rustX operation, interaction lifetime, or Tool behavior is simulated here.
"""
import json
from fake_provider.scenario import (
    OPENAI_CHAT_COMPLETIONS, Expect, Finish, Gate, Scenario, Step, Stream, Text, ToolCall,
)


def web_console_dogfood() -> Scenario:
    def expected(prompt: str) -> Expect:
        return Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", body_contains=(prompt,))

    questions = json.dumps({"questions": [{
        "header": "Direction", "question": "Keep runtime authority in rustX?",
        "options": [{"label": "Keep native (Recommended)", "description": "Use rustX owners."},
                    {"label": "Inspect first", "description": "Read the protocol."}],
        "multi_select": False,
    }]})
    return Scenario(
        "web_console_dogfood",
        Step(expected("Long action in A"), Stream(Text("A is running."), Gate("finish-a"), Text(" A finished."), Finish())),
        Step(expected("Use B while A runs"), Stream(Text("B stayed responsive."), Finish())),
        Step(expected("Approval please"), Stream(ToolCall("console-bash", "bash", json.dumps({
            "command": "printf console-approved; printf x >> console-effect", "execution_mode": "foreground",
        })), Finish("tool_calls"))),
        Step(expected("console-approved"), Stream(Text("Approval completed."), Finish())),
        Step(expected("Questionnaire please"), Stream(ToolCall("console-question", "ask_user", questions), Finish("tool_calls"))),
        Step(expected("Keep native"), Stream(Text("Questionnaire completed."), Finish())),
        Step(expected("Publish while detached"), Stream(Text("Preparing a question."), Gate("publish-question"),
             ToolCall("console-detached-question", "ask_user", questions), Finish("tool_calls"))),
        Step(expected("Keep native"), Stream(Text("Detached question completed."), Finish())),
    )


SCENARIOS = {"web_console_dogfood": web_console_dogfood}


def web_chat_history() -> Scenario:
    """Thirty-four accepted turns cross the native 64-entry bootstrap boundary."""
    steps = [Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", body_contains=(f"History {i}",)),
                  Stream(Text(f"Answer {i}"), Finish())) for i in range(34)]
    steps.append(Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", body_contains=("Rich reply",)),
                      Stream(Text("## Rich reply\n\n| Key | Value |\n| --- | --- |\n| native | history |\n\n```rust\nfn main() {}"),
                             Gate("settle-chat"), Text("\n```\n\n- **Settled**\n\n$x^2$"), Finish())))
    steps.append(Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", body_contains=("Image please",)),
                      Stream(ToolCall("chat-image", "render_image", "{}"), Finish("tool_calls"))))
    return Scenario("web_chat_history", *steps)


SCENARIOS["web_chat_history"] = web_chat_history
