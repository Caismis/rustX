# A2UI Express parser

The lexer, generated parser, visitor, and errors are from the Apache-2.0 A2UI repository at [2d38217c](https://github.com/a2ui-project/a2ui/tree/2d38217c1635021c9e794afa01f25d8912d87761/typescript/agents/a2ui_agent/src/inference-formats/express). The published Agent SDK entry point is Node-oriented and its current release predates the Express system-function syntax used by `@index()`. The pinned syntax reader runs in the browser with ANTLR and requires no Node shims.

To refresh, copy `visitor.ts`, `errors.ts`, and the three generated TypeScript files from the same upstream revision. Point the `A2uiError` import in `errors.ts` at the public `@a2ui/web_core/errors` module and remove the unused generated `Token` imports and `int` alias. Run the Express tests and regenerate dependency notices.
