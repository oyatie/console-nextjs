#!/usr/bin/env node
/**
 * Cursor-native lane receipt validator.
 *
 * Mirrors the skim-proof BUILD_SCHEMA / REVIEW_SCHEMA fields from
 * `scripts/console/workflows/lane-fanout.js`. Exit nonzero = not done.
 *
 * Usage:
 *   node scripts/cursor/validate-lane-receipt.mjs scripts/console/fixtures/<id>.json
 *   node scripts/cursor/validate-lane-receipt.mjs --schema build|critic <path>
 */
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

const N_A_ENFORCEMENT = /^n\/a\s*-\s*adds no enforcement\b/i
const N_A_PERIPHERALS = /^n\/a\s*-\s*nothing described this behaviour\b/i

function die(msg, code = 2) {
  console.error(`validate-lane-receipt: ${msg}`)
  process.exit(code)
}

function isNonEmptyString(v) {
  return typeof v === 'string' && v.trim().length > 0
}

function isStringArray(v) {
  return Array.isArray(v) && v.every((x) => typeof x === 'string')
}

function nonEmptyCommands(commands) {
  if (!Array.isArray(commands) || commands.length === 0) return false
  return commands.every((c) => typeof c === 'string' && c.trim().length > 0)
}

function validateBuild(r) {
  const errors = []
  const required = [
    'status',
    'summary',
    'filesChanged',
    'redBaseline',
    'verification',
    'contractBreaches',
    'enforcementPlacement',
    'peripheralsUpdated',
  ]
  for (const k of required) {
    if (!(k in r)) errors.push(`missing required field: ${k}`)
  }
  if (r.status !== undefined && !['done', 'partial', 'blocked'].includes(r.status)) {
    errors.push(`status must be done|partial|blocked, got ${JSON.stringify(r.status)}`)
  }
  for (const k of ['summary', 'redBaseline', 'verification', 'contractBreaches', 'enforcementPlacement', 'peripheralsUpdated']) {
    if (k in r && !isNonEmptyString(r[k])) errors.push(`${k} must be a non-empty string`)
  }
  if ('filesChanged' in r && !isStringArray(r.filesChanged)) {
    errors.push('filesChanged must be an array of strings')
  }
  if (isNonEmptyString(r.enforcementPlacement) && !N_A_ENFORCEMENT.test(r.enforcementPlacement)) {
    const t = r.enforcementPlacement.toLowerCase()
    if (!t.includes('where') && !t.includes('sequence') && !t.includes('subject')) {
      errors.push(
        'enforcementPlacement must answer WHERE/subject existence (or exact n/a - adds no enforcement)',
      )
    }
  }
  if (isNonEmptyString(r.peripheralsUpdated) && !N_A_PERIPHERALS.test(r.peripheralsUpdated)) {
    // free-form ok if not the n/a escape; empty already caught
  }
  if (r.status === 'done') {
    if (!('commands' in r) || !nonEmptyCommands(r.commands)) {
      errors.push('status=done requires commands: non-empty string[] (empty string entries forbidden)')
    }
    if (!isNonEmptyString(r.headSha)) {
      errors.push('status=done requires headSha bound to the reviewed tip')
    }
  }
  if ('commands' in r && Array.isArray(r.commands) && r.commands.some((c) => typeof c === 'string' && c.trim() === '')) {
    errors.push('commands must not contain empty strings (commandsRun:[""] is a false green)')
  }
  return errors
}

function validateCritic(r) {
  const errors = []
  if (!['APPROVE', 'BLOCK', 'accept', 'reject', 'accept_with_findings'].includes(r.verdict)) {
    errors.push(`verdict must be APPROVE|BLOCK (or accept|reject|accept_with_findings), got ${JSON.stringify(r.verdict)}`)
  }
  if (!Array.isArray(r.findings)) {
    errors.push('findings must be an array')
    return errors
  }
  r.findings.forEach((f, i) => {
    for (const k of ['severity', 'claim', 'failureScenario', 'location', 'provenByExecution', 'ownerLease']) {
      if (!(k in f)) errors.push(`findings[${i}] missing ${k}`)
    }
    if (f.severity !== undefined && !['blocker', 'major', 'minor'].includes(f.severity)) {
      errors.push(`findings[${i}].severity must be blocker|major|minor`)
    }
    if (typeof f.provenByExecution !== 'boolean') {
      errors.push(`findings[${i}].provenByExecution must be boolean`)
    }
    if (typeof f.ownerLease !== 'boolean') {
      errors.push(`findings[${i}].ownerLease must be boolean`)
    }
  })

  const blocking = r.findings.filter((f) => {
    if (!f || f.ownerLease === true) return false
    if (f.severity === 'blocker') return true
    return f.severity === 'major' && f.provenByExecution === true
  })
  const approveLike = ['APPROVE', 'accept'].includes(r.verdict)
  if (approveLike && blocking.length) {
    errors.push(
      `verdict ${r.verdict} conflicts with ${blocking.length} blocking finding(s) (blocker, or major+provenByExecution)`,
    )
  }
  if (r.verdict === 'BLOCK' && blocking.length === 0 && r.findings.length === 0) {
    errors.push('verdict BLOCK with empty findings — cite at least one finding')
  }
  return errors
}

function detectKind(r, forced) {
  if (forced) return forced
  if (r && typeof r === 'object' && 'verdict' in r && 'findings' in r) return 'critic'
  return 'build'
}

function main() {
  const args = process.argv.slice(2)
  let schema = null
  const paths = []
  for (let i = 0; i < args.length; i++) {
    if (args[i] === '--schema') {
      schema = args[++i]
      if (!['build', 'critic'].includes(schema)) die(`--schema must be build|critic`)
    } else if (args[i] === '-h' || args[i] === '--help') {
      console.log(`Usage: node scripts/cursor/validate-lane-receipt.mjs [--schema build|critic] <receipt.json>`)
      process.exit(0)
    } else {
      paths.push(args[i])
    }
  }
  if (paths.length !== 1) die('expected exactly one receipt path')

  const path = resolve(paths[0])
  let raw
  try {
    raw = readFileSync(path, 'utf8')
  } catch (e) {
    die(`cannot read ${path}: ${e.message}`, 1)
  }
  let receipt
  try {
    receipt = JSON.parse(raw)
  } catch (e) {
    die(`invalid JSON in ${path}: ${e.message}`, 1)
  }
  if (!receipt || typeof receipt !== 'object' || Array.isArray(receipt)) {
    die('receipt must be a JSON object', 1)
  }

  const kind = detectKind(receipt, schema)
  const errors = kind === 'critic' ? validateCritic(receipt) : validateBuild(receipt)
  if (errors.length) {
    console.error(`INVALID ${kind} receipt: ${path}`)
    for (const e of errors) console.error(`  - ${e}`)
    process.exit(1)
  }
  console.log(`OK ${kind} receipt: ${path}`)
}

main()
