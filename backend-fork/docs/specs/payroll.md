> **EXECUTABLE-CONTRACT / NON-AUTHORITY.** CI-bound gate contract (machine-checked). Cannot authorize product scope, clear HOLDs, release, or production readiness. Current product authority: repository README + [`docs/current/PRODUCT.md`](../current/PRODUCT.md) / ROADMAP / DELIVERY.

# Korean Payroll Kernel Spec (G012)

Status: first regulated-kernel slice. This spec is an implementation contract, not legal/tax advice. Production payroll remains blocked until a licensed 노무사/세무사 validates the worked examples and signs the release gate.

## Goal

Build the Korean payroll foundation as a regulated, source-driven module:

- versioned, effective-dated statutory rate tables for 4대보험, minimum wage, and tax-table provenance;
- deterministic payroll draft math for employee-side deductions only where an official table row is supplied;
- golden-case tests that prove the kernel refuses estimates and is release-gated by professional validation;
- import/export readiness for sensitive payroll fields without exposing them through general HR surfaces.

## Source-of-truth order

1. Korean government/official sources: NPS, NHIS, 고용노동부/최저임금위원회, 국세청, 법제처, 근로복지공단.
2. Versioned internal rate tables with source URL, retrieval date, effective start/end date, and notes.
3. 노무사/세무사-signed golden cases that include input payslip facts, expected deductions, and source table versions.
4. Runtime payroll runs referencing the exact rate-table version and golden-case gate version.

No commercial payroll feed or blog-derived rate is allowed as a calculation source.

## Current official sources checked on 2026-06-27

- 국민연금: NPS explains workplace subscribers split the applicable yearly pension rate 50/50 between employee and employer; 2026 total rate is 9.5%, and NPS lists monthly standard-income caps of 400,000/6,370,000 won for 2025-07-01 through 2026-06-30 and 410,000/6,590,000 won for 2026-07-01 through 2027-06-30. Source: <https://www.nps.or.kr/pnsinfo/ntpsklg/getOHAF0038M0.do>
- 건강보험/장기요양: NHIS 2026 notice lists workplace health-insurance rate 7.19% with employee/employer each bearing 50%, and long-term-care rate 0.9448% applied through the NHIS formula. Source: <https://edi.nhis.or.kr/portal/images/popup/20251204_pop01longdesc.html>
- 최저임금: Minimum Wage Commission table lists 2026 hourly wage 10,320 won, daily 82,560 won, and 209-hour monthly 2,156,880 won. Source: <https://www.minimumwage.go.kr/minWage/policy/decisionMain.do>
- 근로소득 간이세액표: NTS states employers withhold monthly wage income tax using the 근로소득간이세액표 and provides HomeTax download/lookup paths. Source: <https://www.nts.go.kr/nts/cm/cntnts/cntntsView.do?cntntsId=7862&mi=6583>
- 임금명세서: Ministry of Employment and Labor guidance for wage-statement issuance/required fields is part of the pay-statement UX contract. Source: <https://www.moel.go.kr/policy/policydata/view.do?bbs_seq=20211101053>

## Scope in this slice

### Included

- Pure Rust domain crate `console-payroll-domain`.
- Effective-dated 2026 rate records for employee-side deductions:
  - 국민연금 employee share: 4.75% of capped 기준소득월액 in 2026.
  - 건강보험 employee share: 3.595% of 보수월액 in 2026.
  - ~~장기요양 employee share: 0.4724% of 보수월액 in 2026, derived from NHIS total 0.9448% split
    50/50.~~ **Superseded 2026-08-01 — this was wrong about the basis.** 노인장기요양보험법 제9조제1항
    puts 장기요양's basis on the **건강보험료액**, not 보수월액. See "Step 1 of the payroll engine"
    below; at 보수월액 3,000,000 the two models differ by 2원, and with any 경감/면제 by a multiple.
  - 고용보험 실업급여 employee share: modeled as source-required 0.9% row; employer-side employment-stabilization/vocational-training varies by employer and is not guessed in this slice.
  - 산재보험: modeled as employer-only, industry-tariff-required; no employee deduction.
- Income tax handling that requires an NTS tax-table row as input. The kernel must not synthesize tax brackets from memory.
- Local-income-tax handling via the supplied withholding row/golden case, not a hidden approximation.
- Minimum-wage guard data for 2026.
- Release gate that fails unless versioned sources, at least one golden case, and licensed professional validation evidence are present.

### Excluded until the next payroll slice

- Persisted payroll-run tables and payroll REST/UI.
- Full NTS tax-table ingestion/parsing.
- Employer-side full cost calculation for all employment insurance employer subclasses and 산재 industry tariffs.
- Severance, annual-leave payout, weekly-holiday allowance, mid-period hire/termination proration, retroactive settlement, and year-end settlement. These require separate golden cases.

## Data boundaries and permissions

Payroll fields are high-sensitivity and must not be promoted into general HR/user records. Payroll import/export requires payroll-specific permission, masking, dry-run preview, audit log, and passkey confirmation for signing/approval-equivalent actions. Metrics may include counts and gate status only; no raw payroll amounts, resident-registration numbers, bank accounts, phone numbers, or addresses.

## Golden-case contract

Each golden case must include:

- `case_id`, organization/legal entity, pay period, pay date, employee category, and employment status;
- all pay items with 통상임금/taxable/insurance basis flags;
- exact NTS 간이세액표 version/row or official lookup artifact;
- expected employee deductions, employer costs if in scope, net pay, and wage-statement fields;
- source URLs/version identifiers used;
- professional reviewer kind (`labor_attorney`, `tax_accountant`, or equivalent licensed reviewer), reviewed date, and artifact hash.

A payroll calculation may be tested locally before sign-off, but production enablement must fail until the gate is satisfied.

## Release gate

Production payroll calculations are disabled unless all are true:

1. the effective-dated rate table version has at least one official source per statutory item in scope;
2. the current payroll code has at least one matching golden case for the pay type being enabled;
3. every required golden case is marked professionally validated;
4. a 노무사/세무사 validation artifact hash is stored;
5. the payroll run references immutable source and gate version ids;
6. audit, RBAC, and passkey-signing requirements are active for payroll run approval and pay-statement issuance.

## Step 1 of the payroll engine — 4대보험 to the won, over HTTP (added 2026-08-01)

`professionally_validated: false`. Nothing below moves any Korea control off `HOLD`. Every figure is
an agent's read of an official document; each ships with its instrument, article and 시행일자.

### The route

```
POST /api/v1/payroll/employees/{employee_id}/contract-wages     # 근로계약 임금 (append-only)
GET  /api/v1/payroll/employees/{employee_id}/payslip-draft?period=YYYY-MM&pay_date=YYYY-MM-DD
```

Read-only draft, org-wide gated (`PayrollRunManage`), audited. Sources: `employee_contract_wages`
(migration 0210) for the wage in force on the pay date, `employee_attendance_records` (0091) for the
period's real timesheet, `payroll_statutory_rates` (0210) for the citations.

**What the timesheet does.** It **gates**, it does not compute. No figure below is prorated by
attendance — there is no 일할계산 in this slice, and 근로기준법 제56조's 연장·야간·휴일 premiums are
the next increment. What it does carry is a refusal: recorded working days short of the
period's working days — zero 근태기록 is only the extreme case — or a CLOCK_IN count that does not
match its CLOCK_OUT count adds `ATTENDANCE_INCOMPLETE` (with the counts) to `blockers[]` and forces
`issuable: false`, exactly as withholding does. A payslip draft must not read as a full month on a
month nobody evidenced. The expected working days are the period's **weekdays**: 공휴일 and the
contract's own 소정근로일 are not modelled, so the count OVER-states in a month with a holiday. That
is the only direction it may err — it makes the draft refuse a complete timesheet, never accept a
short one.

### The ordered pipeline (`console_payroll_domain::build_statutory_insurance_draft`)

| # | 항목 | 산식 (정수연산, i128) | Rounding, and the instrument that prescribes it |
|---|---|---|---|
| 1 | 기준소득월액 | 소득월액 < 하한액 → 하한액 · > 상한액 → 상한액 · 그 밖에는 `소득월액 - 소득월액 % 1,000` | 국민연금법 시행령 제5조제1항·제5항 (대통령령 제35602호). 밴드 410,000/6,590,000은 보건복지부고시 제2026-31호, 시행 2026-07-01 |
| 2 | 국민연금 (직원) | `trunc10(기준소득월액 × 475/10,000)` | 절사: **국민연금법 제117조(단수의 처리)** → 국고금관리법. MST 280269, **efYd 20260101**, 법률 제21203호 |
| 3 | 건강보험 (총) | `clamp(trunc10(보수월액 × 719/10,000), 20,160, 9,183,480)` | 절사: 법 제107조 → 국고금관리법 제47조제1항 (MST 265877, efYd 20250423, 법률 제20505호). 클램프: 보건복지부고시 제2025-222호 |
| 4 | 건강보험 (직원) | `× 50/100` | **미해결** — 제76조제1항 `100분의 50씩` vs 제107조 끝수. `Q-HALF-SHARE-ROUNDING-UNIT` |
| 5 | 장기요양 (총) | `trunc10(건강보험료액(총) × 9,448/71,900)` | 절사: **노인장기요양보험법 제64조** (「… 제107조 … 단수처리 등에 관하여 준용한다」) → 국민건강보험법 제107조 → 국고금관리법 제47조제1항. MST 281921, efYd 20251230, 법률 제21257호 — `Instrument`가 목적지가 아니라 **준용 조문**을 싣는다 |
| 6 | 장기요양 (직원) | `× 50/100` | 같은 미해결 쟁점. 분담 근거는 **제11조** 준용 (「… 제76조부터 제86조까지 … 준용한다」) → 제76조제1항. 같은 슬라이스 |
| 7 | 고용보험 (직원) | `보수월액 × 9/1,000` | `Rounding::Assumed` — **단수 규정 미발견**, 인용 없음. 징수법(MST 247481, efYd 20240101)·같은 법 시행령(MST 280527, efYd 20251223) 어디에도 단수·끝수 조문이 없고 국고금관리법을 준용하는 조항도 없다. 절사 없이 정확분을 쓰는 공개된 가정(오차 <10원), 확인된 규칙 아님 |
| 8 | 산재보험 | 없음 — 근로자 공제 항목 아님. `basis_won`·`rate_num`·`rate_den`·`total_won` 모두 `null`(0 아님) | 징수법 제13조제5항. 근로자 부담의 부재는 **명시**된다. 단수는 같은 이유로 `Rounding::Assumed` — 인용 없음. 요율은 고용노동부고시 제2025-91호 별지 미파싱이므로 사업주 쪽 기초·요율·부담액은 0이 아니라 **미상** |
| 9 | 소득세 / 지방소득세 | **미산정** | 별표 2 미탑재. `not_computed[]`가 근거 문서를 지목 |

**장기요양의 산정기초는 건강보험료액이지 보수월액이 아니다** (노인장기요양보험법 제9조제1항). 종전
모델 `0.4724% × 보수월액`은 보수월액 3,000,000원에서 14,172원을, 법정 연쇄는 14,170원을 낸다.

From **2026-11-27** the ratio becomes `1,314/10,000` — 제9조제1항의 반올림 문구가 그때 시행된다.
같은 공포번호(제21690호)의 두 시행일 슬라이스이며, `target=law`은 오늘 이미 나중 텍스트를 반환한다.

### The agreement gate

`Rounding::Unresolved{candidates, question_id}` computes every candidate and emits the won **only if
they agree**; otherwise the component is blocked and names its question id. `Q-HALF-SHARE-ROUNDING-UNIT`
is the one live instance, and it reaches **two** components — 건강보험's half and 장기요양's half.

**Measured: it blocks 6,762 / 9,001 (75.1%)** of 보수월액 1,000,000–10,000,000 in 1,000원 steps
(pay date 2026-08-10). Method: run `build_statutory_insurance_draft` at every step and count the
wages where `blocked_by` is set on 건강보험 **or** 장기요양. Executed, not asserted in prose —
`the_agreement_gate_blocks_6_762_of_9_001_sampled_wages` in `console_payroll_domain` recomputes it,
so this paragraph cannot drift from the engine again.

Breakdown: 건강보험 alone 4,500 (50.0%) · 장기요양 alone 4,524 (50.3%) · both 2,262.

> **Correction (2026-08-01).** This paragraph previously read *"blocks 4,500 / 9,001 (50.0%)"*. That
> figure was wrong: 4,500 is 건강보험's half counted **alone**, and 장기요양's half — which is
> blocked independently and on a different 2,262 of the wages — was omitted. The true rate is
> 6,762 / 9,001. The measurement was published as a measurement, so the error is stated here rather
> than quietly overwritten.

That is the honest cost of an unanswered question, and closing it needs NHIS practice or counsel.

### GC-2026-07-KR-MONTHLY-A

근로계약 월 기본급 3,000,000원 · 월 소정근로시간 209h · 시행 2025-03-02. 급여계산기간 2026-07,
소정근로일 23일 완전출근. 지급일 2026-08-10. 무공제사유.

Chosen so every component lands on a 10원 boundary: the case is therefore independent of both
unresolved questions. A golden case whose answer depends on an unanswered question is not one.

| 항목 | 금액 |
|---|---|
| 기본급 / 지급계 | **3,000,000** |
| 국민연금 | **142,500** |
| 건강보험 (총 215,700의 1/2) | **107,850** |
| 장기요양 (기초 215,700 → 총 28,340의 1/2) | **14,170** |
| 고용보험 | **27,000** |
| 산재보험 | 근로자 공제 없음 |
| 근로소득세 / 지방소득세 | 미산정 (NOT_COMPUTED) |
| **4대보험 공제계** | **291,520** |
| **4대보험 공제 후 잔액** | **2,708,480** |
| 차인지급액 | **산출 불가 — 원천징수 미반영**, `issuable: false` |

최저임금 CHECK: 3,000,000 ÷ 209 = 14,354원/h ≥ 10,320원 (고용노동부고시 제2025-47호). PASS.

Executed by: `console_payroll_domain` unit tests (CI step **"Domain crate unit tests"**) and
`backend/crates/payroll/rest/tests/payslip_draft_api.rs` (CI step **"Serialized disposable
PostgreSQL integration targets"**, target `//tools/buck:payroll-rest-payslip-draft-api-pg`).

### Citation corrections made on 2026-08-01 (verified by `target=eflaw` two-step)

Three citations were re-resolved against the slice actually **in force**, not the latest promulgated
text. None changes a won in the golden case; all three were wrong about the version or the wording.

| Instrument | Was | Now | How |
|---|---|---|---|
| 징수법 (산재 근거) | 법률 제21532호, 시행 **2026-10-08** — a FUTURE slice | 법률 **제19209호**, 시행 **2024-01-01** | MST 247481, efYd 20240101. Slice count recounted 2026-08-01 — see the final round below; the two future ones are 20261008 and 20270101 |
| 징수법 제13조제5항 | `(산재보험료는 사업주가 전액 부담한다)` — a **paraphrase presented as a quote** | 조문 전문 인용 | 「사업주가 전액 부담한다」 does not appear in the article; the employer-only conclusion comes from 제13조제5항 having no 근로자 부담 항 |
| ~~지방세법 제103조의13제1항~~ | ~~법률 제21308호, 시행 2026-01-01~~ | ~~법률 제21308호, 시행 2026-07-01~~ | **This row was itself the defect.** It moved a correct anchor onto a later slice; reverted in the final round below |

Also **upgraded**: 고용보험's 근로자 ½ 부담, which the source register held at MEDIUM ("read from a
future file"), is now read verbatim from the in-force 제13조제2항 — `자기의 보수총액에 … 실업급여의
보험료율의 2분의 1을 곱한 금액`.

`seeded_statutory_rate_register_agrees_with_the_kernel_it_cites` now asserts 공포번호 + 시행일자
agreement between the migration and the kernel. Before that assertion existed the drift above was
invisible — only the *numbers* were compared. It was later strengthened three ways (final round): it walks
the kernel's rows by `(code, effective_from)` rather than by one pay date, so both 장기요양 rows are
covered; it requires the cited 시행일자 to be no later than the **row's own `effective_from`**, which
is strictly stronger than the pay-date form it replaced; and it compares `provenance_ko` to
`StatutoryRate::provenance` byte for byte.

### A second round of citation corrections (2026-08-01)

The assertion above compares a citation to the **pay date**, which is why it passed on three rows
that were nonetheless wrong: each cited an instrument enforced **after the row's own
`effective_from`**. A rate cannot be in force before the document that sets it.

| Row | Was | Now | How |
|---|---|---|---|
| `HEALTH_INSURANCE_TOTAL`, from 2026-01-01 | 대통령령 **제36116호**, 시행 2026-02-19 | 대통령령 **제35931호**, 시행 **2026-01-01** | MST 280453; 제44조제1항's last 개정 is 2025.12.23 — 제36116호 never touched it. The earlier note calling 제36116호 "the slice in force" was true of *today* and irrelevant to a row starting 2026-01-01 |
| `LONG_TERM_CARE_TOTAL`, from 2026-01-01 | 대통령령 **제36325호**, 시행 2026-05-12 | 대통령령 **제35987호**, 시행 **2026-01-01** | MST 281843; 제4조's last 개정 is 2025.12.30 |
| `SIMPLIFIED_WITHHOLDING_TABLE`, from 2026-04-23 | 대통령령 **제36343호**, 시행 2026-05-22 | 대통령령 **제36276호**, 시행 **2026-04-23** | 별표HWP파일명 `…36276KC…` carries the 공포번호; eflaw confirms 제36276호 공포·시행 same day |

None of the three moves a won — the numbers were right, the versions were not. The class is now
unreachable: `payroll_statutory_rates` carries
`CONSTRAINT payroll_statutory_rates_not_backdated_before_instrument CHECK (effective_from >=
enforced_on)`, and `a_rate_row_backdated_before_its_own_instrument_is_rejected` plants such a row and
requires the INSERT to fail.

### 국민연금's 10원 절사 — the rule an earlier round recorded as absent

Round 2 emitted 국민연금 unrounded, on the ground that no statutory bridge to 국고금관리법 exists for
국민연금 as 국민건강보험법 제107조 provides for 건강보험. **The bridge exists**: 국민연금법
**제117조(단수의 처리)** — 「이 법에 따른 급여ㆍ연금보험료ㆍ반환금 등을 계산할 때 그 금액에 10원
미만의 단수(端數)가 있으면 「국고금관리법」을 준용하여 계산한다」 (법령ID 001781, MST 280269,
**efYd 20260101**, 법률 제21203호, read via `target=eflaw`). "No rule was located" was a statement
about the search, not about the statute book.

The golden case does not move — 3,000,000 × 475/10,000 = 142,500 is already a 10원 multiple, which is
exactly why a full review missed it. Where it bites: 보수월액 3,001,000 → 142,547 → **142,540**;
기준소득월액 상한 6,590,000 → 313,025 → **313,020**; 하한 410,000 → 19,475 → **19,470**.
`national_pension_truncates_the_10_won_remainder_under_article_117` pins the first and sweeps every
wage to 12,000,000 asserting `% 10 == 0`.

### A third round: citations that did not match what they cited (2026-08-01)

Five defects, none of which moved a won. Every anchor below was re-fetched with `target=eflaw` and a
pinned `efYd`.

| Citation | Was | Now | Why it was wrong |
|---|---|---|---|
| 국민연금법 제117조 (`national_pension_fraction_instrument`) | anchored on the **2026-06-17** slice | **efYd 20260101** | MST 280269 carries **two** 시행일자 slices — 20260101 and 20260617. An unpinned fetch returns the later. 제117조 is present verbatim in the 20260101 slice, so it is in force on the row's own `effective_from` |
| 국민건강보험법 제107조 (`national_treasury_fraction_instrument`) | 법률 제21065호, 시행 **2026-01-02** | 법률 **제20505호**, 시행 **2025-04-23** (MST 265877) | The row it truncates is in force from 2026-01-01, one day *before* the cited slice |
| 국민건강보험법 제76조제1항 (`fifty_fifty_share_instrument`) | 법률 제21065호, 시행 2026-01-02 | 법률 **제20505호**, 시행 **2025-04-23** | Same slice, same defect |
| 국고금관리법 제47조제1항 (`trunc10` doc) | MST 276079, 시행 2026-01-02 | MST **218677**, efYd **20200609** | Same defect in a comment. The text is word-for-word identical in both slices |
| 고용보험 `total_rounding` | `Resolved{ExactWon, 시행령 제12조제1항제2호}` | **`Assumed{ExactWon, …}`** | 제12조제1항제2호 sets a **rate** and prescribes no 단수 rule. `Resolved` claims an instrument settles the rounding; none does |
| 산재 `total_rounding` | `Resolved{ExactWon, 국민건강보험법 제107조}` | **`Assumed{ExactWon, …}`** | 제107조 truncates 「보험료등」 under 국민건강보험법 only; 징수법 provides no 준용 bridge. A statute that does not apply to the row |

`Rounding` gained a third state so the type can say it: `Resolved` (an instrument prescribes the
unit, and is cited) · `Assumed` (**nothing** prescribes it — the unit is a disclosed assumption and
**no instrument is named**) · `Unresolved` (two instruments compete; emit only on agreement).

The negative search `Assumed` records, re-run 2026-08-01 against both in-force texts: 단수/끝수/국고금
occur **zero** times in 징수법 (법령ID 009589, MST 247481, efYd 20240101, 101 조문단위); its 시행령
(법령ID 009842, MST 280527, efYd 20251223, 118 조문단위) has zero 단수 and zero 끝수 and exactly one
국고금 — **제41조의5제2항제1호**, verbatim: 「1. 「국가를 당사자로 하는 계약에 관한 법률」 제2조에 따른
계약. 다만, 「국고금 관리법 시행령」 제31조에 따른 관서운영경비로 그 대가를 지급받는 계약은
제외한다.」 That is 제41조의5(보험료등의 완납증명이 필요한 경우 등) exempting 관서운영경비 contracts
from the 완납증명 duty — 국고금 관리법 **시행령** 제31조, not 국고금관리법 제47조, and nothing about
단수. An earlier revision of this paragraph put the hit in the wrong article and attributed it to the
wrong 호 — 제41조의5제2항**제2호** is the 지방회계법 one, and it contains no 국고금 at all.

**Why the round-2 guards did not catch the last three.** `CHECK (effective_from >= enforced_on)` and
`no_rate_row_is_in_force_before_the_instrument_that_sets_it` both read the **rate** instrument only.
The 단수·분담 instruments have no column in `payroll_statutory_rates` and were never in the kernel
loop, so 제107조 and 제76조제1항 sat a day late on every 2026-01-01 row while both guards stayed green.
The kernel test now walks **every** instrument a row carries — rate, 단수, 분담, 분담단수, clamp.

`the_migration_names_the_test_that_actually_guards_it` closes the last one: the migration comment
named `payroll_statutory_rate_register_matches_kernel`, a test that has never existed. The comment is
now read back by the code it names, and the migration is located by content so the merge-time
renumber does not break it.

### The final round: a correction that was the defect it was closing (2026-08-01)

Six findings, none of which moves a won. Every anchor was re-fetched with `target=eflaw` and a pinned
`efYd`.

| # | Finding | Anchor re-fetched | Outcome |
|---|---|---|---|
| 1 | `local_income_tax_instrument()` was moved to 지방세법 시행 **2026-07-01** and the move recorded above as a *correction* | MST 282559 (법령ID 001649, 공포번호 21308), efYd **20260101** / 20260424 / 20260701 / 20270101 | **Reverted to 2026-01-01.** 제103조의13 is byte-identical across all four, the instrument is returned on **every** draft, and 2026-01-01 is the earliest slice in force on the dates it is served. Digest and the normalization that reproduces it: see `local_income_tax_instrument` |
| 2 | Two hand-maintained copies of the provenance narrative, never compared | — | `seeded_statutory_rate_register_agrees_with_the_kernel_it_cites` now asserts `provenance_ko == StatutoryRate::provenance` byte for byte, over every row keyed by `(code, effective_from)` |
| 3 | The 징수법 시행령 negative search misdescribed its own single hit | MST 280527, efYd **20251223** | The hit is **제41조의5제2항제1호** (「국고금 관리법 시행령」 제31조 관서운영경비, a 완납증명 exemption) — quoted verbatim above, not 제41조 and not the 지방회계법 호 |
| 4 | 장기요양's 절사 named 국민건강보험법 제107조 without the 준용 that reaches it | MST 281921, efYd **20251230**, 법률 제21257호 | New `long_term_care_fraction_instrument()` (제64조) and `long_term_care_half_share_instrument()` (제11조). 제11조 is byte-identical in the 2026-11-27 slice; 제64조 is amended there (제91조의2·제척기간 added) but 「제107조 … 단수처리 … 준용한다」 is untouched, so the quoted fragment holds in both |
| 5 | 「58개 슬라이스」 did not reproduce | `target=eflaw lawSearch`, totalCnt **174**, display=100, pages 1–2 | **56** exact 법령명한글 matches (48 distinct 시행일자, 38 distinct MST), of which 20261008 and 20270101 are 시행예정. The old 58 is retracted in both copies |
| 6 | `조문시행일자` was presented as corroboration independent of the law-level 시행일자 | — | It is not independent: it is the same response echoing the `efYd` that was asked for. Measured on finding 1 — the same article, byte-identical, reports 조문시행일자 20260101 / 20260424 / 20260701 / 20270101 depending only on which slice was requested. Every citation now rests on the pinned `efYd` plus the article text |

**Why neither round-2 guard reached finding 1.** 지방세법 has no `payroll_statutory_rates` row, so
`CHECK (effective_from >= enforced_on)` has nothing to check, and
`no_rate_row_is_in_force_before_the_instrument_that_sets_it` walks the rate table.
`no_instrument_the_draft_emits_post_dates_the_pay_date_it_is_emitted_on` is the widened guard: it
sweeps every 2026 pay date, builds the draft, and checks every instrument in the value returned —
component, share, `not_computed`, 최저임금 — so an instrument with no rate row is covered, and so is
one added tomorrow. Planted back at 2026-07-01 it fails on 지급일 2026-01-02.

### Deferred, explicitly

원천징수 (별표 2 파싱 — 646개 구간행 + 주3 자녀세액공제), 경감/면제 (육아휴직 등 — 이제 표현
가능하나 미구현), 연장·야간·휴일 가산 (근로기준법 제56조 — pay-item 모델 선행 필요), 일할계산,
시급제, 사립학교 교원 50/30/20, 사업주 부담분 전체.

### Open residuals

What this slice does **not** establish, as of the head that ships it. None of these moves a number in
GC-2026-07-KR-MONTHLY-A above. This list lived only in a review transcript until now, which is why it
is here: a residual that exists only in a transcript stops existing when the transcript does, and the
next reader of this engine then has no way to learn what it never settled.

1. **`Q-HALF-SHARE-ROUNDING-UNIT` is unresolved.** 국민건강보험법 제76조제1항's 「100분의 50씩」 and
   제107조's 끝수 처리 do not, between them, say which unit the halving rounds to, and no text read
   settles it — it needs NHIS practice or counsel. Until then the agreement gate refuses to pick a
   default:
   `payroll/domain/src/lib.rs` `the_agreement_gate_blocks_6_762_of_9_001_sampled_wages` pins the
   cost of that refusal at 6,762 blocked of 9,001 sampled wages.

2. **고용보험 and 산재 단수 remain `Rounding::Assumed`.** The 징수법 negative search is now described
   accurately — 단수·끝수 is zero hits in both 본문 and 시행령, and the single 국고금 hit is a 완납증명
   exemption unrelated to 단수 — but a negative search is not a rule. Both stay disclosed assumptions,
   bounded by <10원.

3. **The DB half of the F1 class is uncovered.** `payroll_statutory_rates`' `CHECK (effective_from >=
   enforced_on)` can only see an instrument that has a row in that table. 지방세법 has none, so
   `local_income_tax_instrument` is guarded by the kernel sweep alone. A register row for non-rate
   instruments is the real fix and is not in this diff.

4. **The provenance equality guard reaches the six rate rows only.** Migration 0210 seeds 14 rows;
   `payslip_draft_api.rs` `seeded_statutory_rate_register_agrees_with_the_kernel_it_cites`
   walks `statutory_contribution_rates()`, which is six of them. The other **eight** —
   `PENSION_STANDARD_INCOME_BAND` ×2, `HEALTH_PREMIUM_BAND`, `MINIMUM_WAGE`,
   `SIMPLIFIED_WITHHOLDING_TABLE` ×4 — carry a `provenance_ko` with no kernel counterpart to compare
   against, because `MonthlyBaseLimit`, `MinimumWageRate` and the 별표 2 tuples have no `provenance`
   field at all. Those eight narratives are single-sourced and unguarded.

   The guard itself has been **observed to pass**, not merely believed to: all 8 tests in
   `payslip_draft_api.rs` green against a disposable PostgreSQL 18.4, including this one and the
   golden case over HTTP. It is a `#[sqlx::test]`, so it needs a reachable database and reports
   nothing at all without one — an earlier run of this section had none and the whole target silently
   never executed, which is why this sentence records the observation rather than the expectation.
   What stays narrow is the six-of-fourteen scope above; that is a real gap and this run does not
   close it.

5. **Two guards divide the instruments between them, and a new field would fall in the gap.** Share
   instruments **are** swept: `StatutoryComponent.share_instrument` reaches the draft output, and
   `payroll/domain/src/lib.rs` `no_instrument_the_draft_emits_post_dates_the_pay_date_it_is_emitted_on`
   reads it. What never reaches the output is `Rounding::Resolved`'s instrument and `Clamp`'s; those
   are covered instead by `no_rate_row_is_in_force_before_the_instrument_that_sets_it`, which walks
   all five instruments a row carries. Planting 2026-09-09 on
   `national_treasury_fraction_instrument` fails the row-level guard and passes the sweep, which is
   the division working as designed. Between the two, every instrument the engine can serve is
   reached — but the split is maintained by hand, so a future field on `StatutoryComponent` is
   covered by neither until someone adds it to one.

6. **2026-01-01 produces no draft, and this diff is what introduced it.** The rate table starts
   2026-01-01 but `withholding_table_instrument_on` has no 별표 2 slice before 2026-01-02, so the
   engine refuses that one day. It is not pre-existing: commit `2a65e02f6` contains neither
   `build_statutory_insurance_draft` nor `simplified_withholding_table_instruments` — the whole 별표 2
   table arrives here. It stays unfixed because the behaviour is a refusal by design rather than a
   wrong payslip: a draft request for 지급일 2026-01-01 returns HTTP 422 「missing 간이세액표 별표
   version for 2026-01-01」, not a 500 and not an estimate.

7. **산재 별지 요율표 is still unparsed.** Employer-side basis, rate and amount are served as `null`,
   never as 0 — `industrial_accident_emits_null_basis_and_null_rate_never_a_zero` is what holds that
   distinction.

8. **Korea controls remain HOLD**, `professionally_validated: false`. Nothing in this section, or
   anywhere above it, is a compliance claim.

## G028 production-control contract

This contract keeps payroll useful for import/staging and receipt workflows while preventing a false
"payroll is live" claim.

- **Domain ownership:** payroll calculations live in `console-payroll-domain`; general HR pages may show
  employment/lifecycle facts but must not own wage, bank, resident-registration, tax, insurance, or
  severance amounts.
- **Protected staging:** HR workbooks may preserve payroll/severance fields in the raw import ledger,
  but generic employee import/export can only preview masked values and canonical non-payroll fields.
- **No estimate path:** payroll drafts must require an official NTS withholding row and effective-dated
  rate table record. Missing tables, source URLs, golden cases, or professional validation fail closed.
- **Receipt workflow:** payroll/wage-statement mail may exist as an audited work-mail object only after
  a payroll processor creates the source object under payroll permission; mail is not a calculator.
- **Signing and audit:** payroll run approval, wage-statement issuance, severance/interim-settlement
  approval, and any correction that changes pay require passkey step-up, actor attribution, memo/evidence,
  and audit records that avoid raw sensitive payloads.
- **Release evidence:** production enablement requires `npm run check:payroll`, `npm run
  check:payroll-release-gate`, golden-case artifacts, and licensed labor/tax reviewer evidence. Until
  those artifacts exist, UI must present payroll as controlled staging/readiness, not as a payable run.
