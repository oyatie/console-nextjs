# 2026-08-01 — a 급여명세서 draft that refuses to look finished

A statutory-insurance draft served over HTTP for a Korean monthly payroll period. At 보수월액
3,000,000: 국민연금 142,500 · 건강보험 107,850 · 장기요양 14,170 on basis 215,700 · 고용보험 27,000 →
공제계 291,520, 잔액 2,708,480.

**The property that matters is what it refuses to do.** 산재 is employer-only and serves all-`null`
rather than a zero, because an ungrounded zero reads as a settled figure. Withholding is an explicit
`NOT_COMPUTED` blocker with `netPayWon: null` and `issuable: false`. The draft will not present
itself as a payslip, and that is pinned by test rather than by convention.

The 2원 that justifies the whole approach: the old rate table computed 장기요양 as
`4_724 ppm × 보수월액` → 14,172. The statutory chain — 노인장기요양보험법 제9조제1항, basis = 건강보험료액,
via 제64조 준용 to 국민건강보험법 제107조 and 국고금관리법 제47조제1항 — gives **14,170**. A rate table that
is approximately right produces a payslip that is exactly wrong.

Every rate binds to the instrument that sets it, fetched at a pinned 시행일자. Two guards keep that
honest, and they divide the surface between them: a per-row check that no rate is in force before
the instrument that sets it, and a sweep over every 2026 pay date asserting that no instrument the
draft emits post-dates the pay date it is emitted on. The sweep needs no enumeration — it walks
whatever the draft actually returns — and it is what caught a wrong-slice anchor that had been
introduced and documented as a correction.

`docs/specs/payroll.md` carries an **Open residuals** section: eight items this slice does not
establish, including the unresolved 제76조제1항-versus-제107조 halving unit that blocks 6,762 of 9,001
sampled wages rather than picking a default, and 고용보험/산재 단수 remaining disclosed assumptions
bounded by <10원 because a negative search is not a rule.

Korea controls remain `HOLD` and `professionally_validated` stays `false`. Every capability, evidence
contract, jurisdiction binding, review disposition, and exposure state remains `HOLD`. Nothing here
is a compliance claim.
