# One fixed Workflow

Use the minimal user catalog/settings with this workspace. After reviewing it,
explicitly grant trust, then run `rustx config check` and
`rustx config show --sources`. The registered `read_file` Workflow accepts a
`path`, invokes the native Read Tool, and returns its text. It requires no
Python, MCP, or Subagent. Static checking compiles its fixed graph without
executing the Tool. Ordinary Workflow admission and Tool approval still apply.
