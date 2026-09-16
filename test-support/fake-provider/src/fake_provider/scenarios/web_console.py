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
    steps.append(Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", body_contains=("Keep this draft", "user_uploaded_files", "note.txt", ".agents/uploads/")),
                      Stream(Text("Uploaded workspace file received."), Finish())))
    steps.append(Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", body_contains=("Image please",)),
                      Stream(ToolCall("chat-image", "render_image", "{}"), Finish("tool_calls"))))
    return Scenario("web_chat_history", *steps)


SCENARIOS["web_chat_history"] = web_chat_history


def web_commands() -> Scenario:
    """Real native retry consumes the returned boundary input, never old output."""
    def expected(model: str) -> Expect:
        return Expect(protocol=OPENAI_CHAT_COMPLETIONS, model=model,
                      body_contains=("Regenerate my uploaded note", "user_uploaded_files", "note.txt", ".agents/uploads/"),
                      body_excludes=("Original native answer", "Regenerated native answer"))
    return Scenario("web_commands",
                    Step(expected("second-model"), Stream(Text("Original native answer"), Finish())),
                    Step(expected("console-model"), Stream(Text("Regenerated native answer"), Finish())))


SCENARIOS["web_commands"] = web_commands


def web_composer_context() -> Scenario:
    """Native Todo and Goal owners feed the composer docks; one Human input waits in the mailbox.

    The provider only emits model calls. The Todo list, Goal state, continuation
    round admission and the pending inbound row all belong to rustX.
    """
    def expected(text: str) -> Expect:
        return Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", body_contains=(text,))

    def todo(call: str, arguments: dict[str, object]) -> ToolCall:
        return ToolCall(call, "todo", json.dumps(arguments))

    return Scenario(
        "web_composer_context",
        Step(expected("Plan the composer docks"), Stream(
            todo("todo-bind", {"action": "create", "subject": "Bind native Todo"}),
            todo("todo-goal", {"action": "create", "subject": "Render the Goal dock", "blocked_by": [1]}),
            todo("todo-start", {"action": "update", "id": 1, "status": "in_progress", "active_form": "Binding native Todo"}),
            Finish("tool_calls"))),
        Step(expected("Binding native Todo"), Stream(Text("Plan recorded."), Finish())),
        Step(expected("Keep working until the docks are verified"), Stream(ToolCall("goal-create", "create_goal", json.dumps(
            {"objective": "Verify the composer docks", "autonomous_round_budget": 1})), Finish("tool_calls"))),
        Step(expected("Verify the composer docks"), Stream(Text("Goal recorded."), Finish())),
        Step(expected("Continue pursuing the current Goal"), Stream(
            Text("Continuing the Goal."), Gate("goal-round"), Text(" Round finished."), Finish())),
        Step(expected("Queued during the Goal round"), Stream(Text("Queued input handled."), Finish())),
    )


SCENARIOS["web_composer_context"] = web_composer_context
