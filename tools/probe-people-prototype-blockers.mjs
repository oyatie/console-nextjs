import { readFile, stat } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import ts from "typescript";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const files = {
  manifest: path.join(root, ".next/standalone/.next/server/app-paths-manifest.json"),
  people: path.join(root, "src/app/(console)/hr/people/page.tsx"),
  layout: path.join(root, "src/app/(console)/layout.tsx"),
  config: path.join(root, "next.config.ts"),
};
const scope = "Current People prototype blockers only; passing does not prove authentication, authorized SSR, owner reads, or two-site safety";

function source(filename, content) {
  return ts.createSourceFile(filename, content, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
}

function callsNotFound(statement) {
  const direct = ts.isBlock(statement) && statement.statements.length === 1
    ? statement.statements[0] : statement;
  return ts.isExpressionStatement(direct) && ts.isCallExpression(direct.expression)
    && ts.isIdentifier(direct.expression.expression)
    && direct.expression.expression.text === "notFound";
}

function deniesAllProductionRequests(tree) {
  const normalized = (node) => node.getText(tree).replace(/\s+/g, "").replace(/'/g, '"');
  return tree.statements.some((statement) =>
    ts.isFunctionDeclaration(statement) && statement.modifiers?.some((modifier) => modifier.kind === ts.SyntaxKind.DefaultKeyword)
    && statement.body?.statements.some((child) => ts.isIfStatement(child)
      && normalized(child.expression) === 'process.env.NODE_ENV==="production"'
      && callsNotFound(child.thenStatement)));
}

try {
  const [manifest, people, layout, manifestInfo, peopleInfo, layoutInfo, configInfo] = await Promise.all([
    readFile(files.manifest, "utf8"), readFile(files.people, "utf8"), readFile(files.layout, "utf8"),
    stat(files.manifest), stat(files.people), stat(files.layout), stat(files.config),
  ]);
  if (manifestInfo.mtimeMs < Math.max(peopleInfo.mtimeMs, layoutInfo.mtimeMs, configInfo.mtimeMs)) {
    console.log(JSON.stringify({ scope, status: "unreached", reason: "Fresh production build required after relevant source changes" }));
    process.exitCode = 2;
  } else {
    const peopleTree = source(files.people, people);
    const layoutTree = source(files.layout, layout);
    const checks = [
      { id: "people-production-route", passed: "/(console)/hr/people/page" in JSON.parse(manifest) },
      { id: "people-prototype-store-detached", passed: !peopleTree.statements.some((statement) =>
        ts.isImportDeclaration(statement) && ts.isStringLiteral(statement.moduleSpecifier)
        && statement.moduleSpecifier.text === "@/lib/store") },
      { id: "console-unconditional-production-denial-removed", passed: !deniesAllProductionRequests(layoutTree) },
    ];
    console.log(JSON.stringify({ scope, status: checks.every((check) => check.passed) ? "passed" : "failed", checks }));
    if (checks.some((check) => !check.passed)) process.exitCode = 1;
  }
} catch (error) {
  console.log(JSON.stringify({ scope, status: "unreached", reason: error instanceof Error ? error.message : String(error) }));
  process.exitCode = 2;
}
