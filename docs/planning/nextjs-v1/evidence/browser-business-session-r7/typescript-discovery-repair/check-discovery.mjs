import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import assert from 'node:assert/strict';
import crypto from 'node:crypto';
import { createRequire } from 'node:module';

const root = process.cwd();
const ts = createRequire(path.join(root, 'package.json'))('typescript');
const out = path.dirname(new URL(import.meta.url).pathname);
const baseline = JSON.parse(fs.readFileSync(path.join(out, 'baseline-tsconfig.json'), 'utf8'));
const candidate = JSON.parse(fs.readFileSync('tsconfig.json', 'utf8'));
assert.deepEqual(candidate.compilerOptions, baseline.compilerOptions);
assert.deepEqual(candidate.include, baseline.include);
assert.deepEqual(candidate.exclude, baseline.exclude.toSpliced(baseline.exclude.length - 1, 0, 'docs/planning/nextjs-v1/evidence'));
const parse = config => ts.parseJsonConfigFileContent(config, ts.sys, root, undefined, path.join(root, 'tsconfig.json'));
const old = parse(baseline), next = parse(candidate);
assert.equal(old.errors.length, 0);
assert.equal(next.errors.length, 0);
const relative = files => files.map(f => path.relative(root, f)).sort();
const before = relative(old.fileNames), after = relative(next.fileNames);
const difference = (left, right) => {
  const set = new Set(right);
  return left.filter(f => !set.has(f));
};
const removed = difference(before, after), added = difference(after, before);
assert.equal(removed.length, 3);
assert.ok(removed.every(f => f.startsWith('docs/planning/nextjs-v1/evidence/')));
assert.equal(added.length, 0);
for (const prefix of ['src/', 'tests/', 'e2e/', '.next/types/', '.next/dev/types/', '.next-e2e/types/', '.next-e2e/dev/types/']) {
  assert.deepEqual(after.filter(f => f.startsWith(prefix)), before.filter(f => f.startsWith(prefix)));
}
const oldProgram = ts.createProgram({ rootNames: old.fileNames, options: old.options });
const nextProgram = ts.createProgram({ rootNames: next.fileNames, options: next.options });
const programBefore = relative(oldProgram.getSourceFiles().map(f => f.fileName));
const programAfter = relative(nextProgram.getSourceFiles().map(f => f.fileName));
const programRemoved = difference(programBefore, programAfter);
assert.deepEqual(programRemoved, removed);
assert.deepEqual(difference(programAfter, programBefore), []);
const diagnostics = ts.getPreEmitDiagnostics(nextProgram);
assert.equal(diagnostics.length, 0, ts.formatDiagnosticsWithColorAndContext(diagnostics, {
  getCurrentDirectory: () => root, getCanonicalFileName: f => f, getNewLine: () => '\n',
}));
const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'pr16-ts-import-semantics-'));
let importProof;
try {
  fs.mkdirSync(path.join(temp, 'src'));
  fs.mkdirSync(path.join(temp, 'evidence'));
  fs.writeFileSync(path.join(temp, 'src', 'index.ts'), "import { value } from '../evidence/shared'; export { value };\n");
  fs.writeFileSync(path.join(temp, 'evidence', 'shared.ts'), "export const value: number = 'intentional compiler probe';\n");
  const config = { compilerOptions: baseline.compilerOptions, include: ['src/**/*.ts'], exclude: ['evidence'] };
  const configPath = path.join(temp, 'tsconfig.json');
  fs.writeFileSync(configPath, JSON.stringify(config));
  const scoped = ts.parseJsonConfigFileContent(config, ts.sys, temp, undefined, configPath);
  assert.equal(scoped.fileNames.length, 1);
  const imported = ts.createProgram({ rootNames: scoped.fileNames, options: scoped.options });
  const source = imported.getSourceFile(path.join(temp, 'evidence', 'shared.ts'));
  assert.ok(source);
  const errors = ts.getPreEmitDiagnostics(imported).filter(d => d.file?.fileName === source.fileName);
  assert.ok(errors.some(d => d.code === 2322));
  importProof = { excluded_root_not_discovered: true, explicit_import_enters_program: true, imported_type_error_still_reported: 2322 };
} finally {
  fs.rmSync(temp, { recursive: true, force: true });
}
const report = {
  scope: 'Compiler discovery only; no business/runtime tests.', typescript_version: ts.version,
  baseline_roots: before.length, candidate_roots: after.length, removed, added,
  program_before_files: programBefore.length, program_after_files: programAfter.length, program_removed: programRemoved,
  all_non_evidence_roots_and_imported_dependencies_retained: true,
  compiler_options_and_includes_unchanged: true, candidate_diagnostics: diagnostics.length,
  import_semantics: importProof,
  tsconfig_sha256: crypto.createHash('sha256').update(fs.readFileSync('tsconfig.json')).digest('hex'),
  before, after, programBefore, programAfter,
};
fs.writeFileSync(path.join(out, 'discovery-and-import-semantics.json'), JSON.stringify(report, null, 2) + '\n');
console.log(JSON.stringify({ ...report, before: undefined, after: undefined, programBefore: undefined, programAfter: undefined }));
