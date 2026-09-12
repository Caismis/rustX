# Local-runtime workspace

These are the project instructions inherited by the main Agent. Subagents
receive this chain only when their `agents_md.inherit` policy enables it; their
explicit `.agents/agents/<name>/AGENTS.md` files are resolved and frozen by
the parent runtime.
