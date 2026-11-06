"""Invoke a Workflow main with JSON inputs in the caller's workspace."""
import contextlib
import json
import sys

payload = json.load(sys.stdin)
namespace = {"__name__": "__main__"}
with contextlib.redirect_stdout(sys.stderr):
    exec(compile(payload["code"], "<workflow-code>", "exec"), namespace)
    main = namespace.get("main")
    if not callable(main):
        raise TypeError("Code must define main")
    result = main(**payload["inputs"])
if not isinstance(result, dict):
    raise TypeError("main must return an object")
sys.stdout.write(json.dumps(result, ensure_ascii=False, allow_nan=False))
