// Add missing ADR index rows to docs/decisions/README.md. Additive only.
//
// WHY THIS IS NARROWER THAN IT FIRST LOOKS
//
// The first version of this script regenerated the whole table from frontmatter and
// would have destroyed information that exists nowhere else. The status cell is not
// the frontmatter status: it is the frontmatter status plus an authored qualifier —
// "accepted, amended", "accepted, fixture-only", "accepted target only",
// "accepted, amended, reconciliation required". Those qualifiers carry scope limits a
// reader needs ("no live enforcement switch", "no live institution access") and there
// is no field to derive them from. check-adrs.mjs tolerates them deliberately: it
// matches with statusCell.startsWith(status).
//
// The scope sentence is likewise authored judgement, not a restatement of the title.
//
// So the rule this script follows: **generate what is derivable, never write over what
// is authored.** A missing row is derivable — id, link and initial status all come from
// the file. An existing row is not, and is left exactly as it stands.
//
// That keeps the failure this exists to prevent — a row missing for ADR-0034 while the
// README simultaneously asserted "no ADR-0034 was written", true when written and
// falsified by writing the file — without introducing a worse one.
//
// Usage:
//   node scripts/console/generate-adr-index.mjs           # report missing rows, exit 1
//   node scripts/console/generate-adr-index.mjs --write   # insert them in id order
//
// check-adrs.mjs remains the gate. This only removes the manual step.

import { readFileSync, writeFileSync, readdirSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const DECISIONS = join(ROOT, 'docs', 'decisions');
const README = join(DECISIONS, 'README.md');
const TABLE_HEADER = '| ID | Status | Decision and scope |';

function frontmatterStatus(text, filename) {
  if (!text.startsWith('---\n')) throw new Error(`${filename}: no frontmatter`);
  const end = text.indexOf('\n---', 4);
  if (end === -1) throw new Error(`${filename}: unterminated frontmatter`);
  const block = text.slice(4, end);
  const id = block.match(/^id:\s*(ADR-\d{4})\s*$/m)?.[1];
  const status = block.match(/^status:\s*(\S+)\s*$/m)?.[1];
  if (!id || !status) throw new Error(`${filename}: frontmatter missing id or status`);
  return { id, status };
}

const adrs = readdirSync(DECISIONS)
  .filter((f) => /^ADR-\d{4}-.*\.md$/.test(f))
  .map((filename) => ({ filename, ...frontmatterStatus(readFileSync(join(DECISIONS, filename), 'utf8'), filename) }))
  .sort((a, b) => a.id.localeCompare(b.id));

const readmeText = readFileSync(README, 'utf8');
const lines = readmeText.split('\n');

const start = lines.findIndex((l) => l.trim() === TABLE_HEADER);
if (start === -1) throw new Error(`${README}: index table header not found`);
let end = start + 1;
while (end < lines.length && lines[end].trimStart().startsWith('|')) end += 1;

const tableLines = lines.slice(start, end);
const indexed = new Set();
for (const line of tableLines) {
  const id = line.match(/\b(ADR-\d{4})\b/)?.[1];
  if (id) indexed.add(id);
}

const missing = adrs.filter((a) => !indexed.has(a.id));
if (missing.length === 0) {
  console.log(`ADR index has a row for every ADR: ${adrs.length} indexed.`);
  process.exit(0);
}

if (!process.argv.includes('--write')) {
  console.error(`ADR index is missing ${missing.length} row(s): ${missing.map((m) => m.id).join(', ')}`);
  console.error('Run: node scripts/console/generate-adr-index.mjs --write, then write each scope sentence.');
  process.exit(1);
}

// Insert each missing row in id order, so the table stays sorted without reordering
// rows that already exist — reordering would produce a diff nobody asked for.
const next = [...tableLines];
for (const adr of missing) {
  const row = `| [${adr.id}](${adr.filename}) | ${adr.status} | **SCOPE SENTENCE NOT WRITTEN** — one sentence: what ${adr.id} decides and its scope |`;
  let at = next.length;
  for (let i = 2; i < next.length; i += 1) {
    const id = next[i].match(/\b(ADR-\d{4})\b/)?.[1];
    if (id && id.localeCompare(adr.id) > 0) { at = i; break; }
  }
  next.splice(at, 0, row);
}

writeFileSync(README, [...lines.slice(0, start), ...next, ...lines.slice(end)].join('\n'), 'utf8');
console.log(`Inserted ${missing.length} row(s): ${missing.map((m) => m.id).join(', ')}`);
console.log('Each carries a placeholder scope sentence. Write them — the gate does not check prose.');
