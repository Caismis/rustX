# Local-runtime workspace

These are the project instructions inherited by the main Agent. Subagents
receive this chain only when their `agentsMd.inherit` policy enables it; their
explicit `.agents/subagents/<name>/AGENTS.md` files are resolved and frozen by
the parent runtime.
