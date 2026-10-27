import { relative } from 'node:path';
import ts from 'typescript';

const config = ts.getParsedCommandLineOfConfigFile('tsconfig.json', {}, { ...ts.sys, onUnRecoverableConfigFileDiagnostic: () => {} });
const program = ts.createProgram(config.fileNames, config.options);
const checker = program.getTypeChecker();

const neverCheck = (clause) =>
  clause.statements.length === 1 &&
  ts.isExpressionStatement(clause.statements[0]) &&
  ts.isSatisfiesExpression(clause.statements[0].expression) &&
  clause.statements[0].expression.type.kind === ts.SyntaxKind.NeverKeyword;

let found = 0;
for (const source of program.getSourceFiles()) {
  if (source.isDeclarationFile || !config.fileNames.includes(source.fileName)) continue;
  const report = (node, text) => {
    const { line } = source.getLineAndCharacterOfPosition(node.getStart());
    console.error(`${relative('.', source.fileName)}:${line + 1}: ${text}`);
    found++;
  };
  const visit = (node) => {
    if (ts.isSwitchStatement(node)) {
      const type = checker.getTypeAtLocation(node.expression);
      if (type.isUnion() && type.types.every((t) => t.isLiteral() || t.flags & ts.TypeFlags.BooleanLiteral)) {
        const clauses = node.caseBlock.clauses;
        const fallback = clauses.find((c) => ts.isDefaultClause(c));
        if (fallback && !neverCheck(fallback)) report(fallback, 'default hides new union members. Write a case per member and end with `default: x satisfies never`.');
        const cased = new Set(clauses.filter(ts.isCaseClause).map((c) => checker.typeToString(checker.getTypeAtLocation(c.expression))));
        const missing = type.types.map((t) => checker.typeToString(t)).filter((name) => !cased.has(name));
        if (missing.length > 0) report(node, `switch misses ${missing.join(', ')}.`);
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(source);
}
process.exit(found ? 1 : 0);
