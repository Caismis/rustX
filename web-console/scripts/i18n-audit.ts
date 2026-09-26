import ts from 'typescript';

/** A bounded syntax check of presentation positions. It parses TSX, including
 * multiline expressions, but deliberately does not infer arbitrary data flow. */
const copyProps = /^(?:alt|aria-label|aria-description|aria-valuetext|title|label|placeholder|description|hint|tooltip|emptyLabel|confirm|confirmLabel|confirmText|cancelLabel|subtitle|emptyText|triggerLabel|closeLabel|copy|copied|collapse|collapseAria|expandAria|noOutput|noExitCode|running|failed|done|copyLabel|copiedLabel|footnotes|note|copyValue|copyJson|copyPath|copyPretty|copyCompact|collapseNode|expandNode)$/;
export interface CopyLiteral { start: number; end: number; text: string; line: number; kind: 'text' | 'attribute' | 'expression' | 'identity' }
// Exact language-independent tokens; never wildcard files or English phrases.
const tokens = new Set(['rustX', 'rX', 'JSON', 'JSON-RPC', 'HTTP', 'HTTPS', 'URL', 'ID', 'API', 'UTF-8', 'UTF-16', 'TOML', 'MCP', 'PNG', 'SVG', 'HTML', 'PDF', 'English', '中文', 'true', 'false', 'null', 'undefined', 'Symbol', 'Function', 'function()']);
export function findCopy(file: string, source: string): CopyLiteral[] {
  const ast = ts.createSourceFile(file, source, ts.ScriptTarget.Latest, true);
  const found = new Map<number, CopyLiteral>();
  function report(node: ts.Node, text: string, kind: CopyLiteral['kind']) {
    text = text.replace(/\s+/g, ' ').trim();
    if (!/\p{L}/u.test(text.replace(/\{\w+\}/g, '')) || tokens.has(text) || /^(?:common|sidebar|workspace|settings|agent|tools|interactions|commands|trajectory|inspector|artifacts):[a-zA-Z][a-zA-Z0-9._ -]*$/.test(text)) return;
    // One adjacent exemption with a written reason, attached to one literal.
    const prefix = source.slice(node.getFullStart(), node.getStart(ast));
    if (/\/\* i18n-raw: [^*\n]+ \*\//.test(prefix)) return;
    found.set(node.getStart(ast), { start: node.getStart(ast), end: node.end, text, kind, line: ast.getLineAndCharacterOfPosition(node.getStart(ast)).line + 1 });
  }
  function expression(node: ts.Node, kind: CopyLiteral['kind'] = 'expression') {
    if (ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node)) report(node, node.text, kind);
    else if (ts.isTemplateExpression(node)) {
      report(node, node.head.text + node.templateSpans.map((span, i) => `{p${i}}` + span.literal.text).join(''), kind);
      for (const span of node.templateSpans) expression(span.expression);
    }
    else if (ts.isConditionalExpression(node)) { expression(node.whenTrue); expression(node.whenFalse); }
    else if (ts.isBinaryExpression(node)) {
      if (node.operatorToken.kind === ts.SyntaxKind.AmpersandAmpersandToken) expression(node.right);
      else if ([ts.SyntaxKind.BarBarToken, ts.SyntaxKind.QuestionQuestionToken, ts.SyntaxKind.PlusToken].includes(node.operatorToken.kind)) { expression(node.left); expression(node.right); }
    } else if (ts.isParenthesizedExpression(node) || ts.isAsExpression(node) || ts.isSatisfiesExpression(node)) expression(node.expression);
    else if (ts.isArrowFunction(node) || ts.isFunctionExpression(node)) {
      if (!ts.isBlock(node.body)) expression(node.body);
      else {
        const returns = (child: ts.Node) => {
          if (ts.isReturnStatement(child) && child.expression) expression(child.expression);
          else if (!ts.isFunctionLike(child)) ts.forEachChild(child, returns);
        };
        ts.forEachChild(node.body, returns);
      }
    }
  }
  // Choice and its Enum form wrapper own [semantic value, visible label] tuples.
  // Follow only inline syntax (including spreads/const assertions), not data flow.
  function optionLabels(node: ts.Node) {
    if (ts.isArrayLiteralExpression(node)) {
      for (const option of node.elements) {
        let tuple: ts.Node = option;
        while (ts.isAsExpression(tuple) || ts.isSatisfiesExpression(tuple) || ts.isParenthesizedExpression(tuple)) tuple = tuple.expression;
        if (ts.isArrayLiteralExpression(tuple) && tuple.elements[1]) expression(tuple.elements[1]);
        else if (ts.isSpreadElement(tuple)) optionLabels(tuple.expression);
      }
    } else if (ts.isConditionalExpression(node)) { optionLabels(node.whenTrue); optionLabels(node.whenFalse); }
    else if (ts.isAsExpression(node) || ts.isSatisfiesExpression(node) || ts.isParenthesizedExpression(node)) optionLabels(node.expression);
  }
  function visit(node: ts.Node) {
    if (ts.isJsxAttribute(node) && node.name.getText(ast) === 'options'
      && node.initializer && ts.isJsxExpression(node.initializer) && node.initializer.expression
      && ['Choice', 'Enum'].includes(node.parent.parent.tagName.getText(ast))) optionLabels(node.initializer.expression);
    if (ts.isJsxElement(node) && node.openingElement.tagName.getText(ast) === 'option'
      && !node.openingElement.attributes.properties.some(prop => ts.isJsxAttribute(prop) && prop.name.getText(ast) === 'value')) {
      report(node.openingElement, 'Option needs an explicit stable value independent of its translated label', 'identity');
    }
    if (ts.isJsxText(node)) report(node, node.text, 'text');
    if (ts.isJsxAttribute(node) && copyProps.test(node.name.getText(ast)) && node.initializer) {
      if (ts.isStringLiteral(node.initializer)) expression(node.initializer, 'attribute');
      else if (ts.isJsxExpression(node.initializer) && node.initializer.expression) expression(node.initializer.expression);
    }
    if (ts.isJsxExpression(node) && !ts.isJsxAttribute(node.parent) && node.expression) expression(node.expression);
    if (ts.isPropertyAssignment(node) && copyProps.test(node.name.getText(ast).replace(/['"]/g, ''))) expression(node.initializer);
    if (ts.isBindingElement(node) && node.initializer && copyProps.test(node.name.getText(ast))) expression(node.initializer);
    ts.forEachChild(node, visit);
  }
  visit(ast);
  return [...found.values()].sort((a,b) => a.start-b.start);
}
