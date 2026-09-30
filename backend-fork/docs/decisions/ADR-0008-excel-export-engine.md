---
id: ADR-0008
status: accepted
doc_status: published
date: 2026-06-12
owner: T0.10 spike / platform team
related: []
---

# ADR-0008: Excel Export Engine — umya-spreadsheet

## Context

The forklift FSM maintenance system must generate the Korean daily-status Excel form
(일일업무진행현황) with byte-fidelity: the output must preserve the layout, merged
cells, section headers, title cell, and column widths of the ground-truth template
(`docs/reference/일일업무진행현황_0605.xlsx`).

Plan §2.8 required a viability spike (T0.10) before committing to any engine, because
template fidelity is a hard acceptance criterion — not a best-effort goal.

The spike evaluated **umya-spreadsheet 3.0.0** (latest stable as of 2026-06-12,
verified live against crates.io) against the real template:

- 1 worksheet named `6월05일`
- Dimension A1:AH97
- Exactly 16 merged cell ranges
- Title cell containing `◈ 일일업무 진행 현황 ◈`
- 4 section headers at rows 2, 24, 44, 77

## Decision

**Use umya-spreadsheet 3.0.0** as the Excel export engine for the
`console-platform-excel` platform crate.

## Evidence

Integration test `tests/template_fidelity.rs` in `console-platform-excel` — all 5
assertions **PASS**:

| Assertion group | Result |
|---|---|
| Pre-condition: sheet name `6월05일` | PASS |
| Pre-condition: exactly 16 merged ranges | PASS |
| Pre-condition: title cell contains `◈` | PASS |
| Pre-condition: all 4 section headers present at correct rows | PASS |
| Round-trip: load → fill row 4 → write → re-read → all invariants intact | PASS |

Round-trip sub-checks (all within the single `roundtrip_fill_and_read_back` test):

- Sheet name survives
- Highest row >= 97 (original extent preserved)
- All 16 merged ranges identical before/after (sorted string comparison)
- Title `◈` present after round-trip
- All 4 section headers present at correct rows after round-trip
- Written values readable back: 구분=미, No=99, 사업장=태성이엔지, 호기=#290,
  불량내용=시동안걸림, 작업자=김용현, Warning=Priority#1
- Column A width > 0 (explicit width preserved)

Verified with:
```
cargo build -p console-platform-excel          # clean
cargo test -p console-platform-excel           # 5 passed, 0 failed
cargo clippy --all-targets -p console-platform-excel  # 0 errors, 0 warnings
```

## Consequences

- `console-platform-excel` depends on `umya-spreadsheet = "3.0.0"` (pure Rust, no
  native dependencies).
- Production fill logic (section 1–4 row writers, date/header injection) will be
  implemented in `console-platform-excel` in M4.
- Re-verify umya version on each dependency update (project mandate).
- No contingency path needed: the spike passed all criteria.

## Alternatives Considered

| Alternative | Rejected because |
|---|---|
| rust_xlsxwriter | Write-only; cannot load existing templates |
| Hybrid (umya layout + rust_xlsxwriter fill) | Unnecessary complexity; umya round-trip passed |
| Python openpyxl via subprocess | Cross-language boundary, deployment complexity |
| Contingency (plan §2.8): targeted fix or rebuild | Not needed — spike passed |
