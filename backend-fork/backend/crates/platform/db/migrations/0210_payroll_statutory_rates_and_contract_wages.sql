-- Payroll engine step 1: the statutory rate register (with citations) and the
-- effective-dated contract wage a payslip is actually computed from.
--
-- ASSERTS NO LEGAL CONCLUSION. Every row records which document sets a number,
-- which article, and when that text entered force. Korea controls remain HOLD;
-- only qualified counsel may unhold them.

-- ---------------------------------------------------------------------------
-- 1. The statutory rate register.
--
-- GLOBAL, no org_id, no RLS: statutory figures are identical for every tenant
-- and carry no tenant data; the register exists to make each figure citable,
-- not to make it editable.
--
-- That carve-out is registered where the gate actually reads it —
-- `global_table_allowlist()` in backend/ci/gates/tenant-isolation/src/lib.rs.
-- The tenant-isolation gate parses SQL, not SQL COMMENTS: a `-- console-gate:`
-- annotation here would be decorative and would not classify this table.
--
-- DELIBERATELY NOT OPERATOR-WRITABLE: console_rt gets SELECT and nothing else.
-- `docs/program/console-jurisdiction-register.json` `source_process.change_rule`
-- already requires a new candidate-bound professional review on any source or
-- effective-date change, so a rate an operator could edit at runtime would be a
-- compliance hole wearing the costume of a feature. Rates change by migration,
-- which means by deploy, which means by review.
--
-- The arithmetic still lives in `console_payroll_domain` (pure, unit-tested,
-- no database). This table is the CITATION surface the API serves and a payslip
-- snapshot is built from;
-- `seeded_statutory_rate_register_agrees_with_the_kernel_it_cites` in
-- backend/crates/payroll/rest/tests/payslip_draft_api.rs fails if the two ever
-- disagree about a number, a 공포번호 or a 시행일자. That test now reads this
-- comment back: `the_migration_names_the_test_that_actually_guards_it` fails if
-- the name above goes stale again, which it did once already.
CREATE TABLE payroll_statutory_rates (
    code                   TEXT NOT NULL CHECK (code IN (
                               'NATIONAL_PENSION_EMPLOYEE',
                               'HEALTH_INSURANCE_TOTAL',
                               'LONG_TERM_CARE_TOTAL',
                               'EMPLOYMENT_INSURANCE_EMPLOYEE',
                               'INDUSTRIAL_ACCIDENT_EMPLOYEE',
                               'PENSION_STANDARD_INCOME_BAND',
                               'HEALTH_PREMIUM_BAND',
                               'MINIMUM_WAGE',
                               'SIMPLIFIED_WITHHOLDING_TABLE')),
    effective_from         DATE NOT NULL,
    effective_to_exclusive DATE CHECK (effective_to_exclusive IS NULL OR effective_to_exclusive > effective_from),

    -- Integer rate as num/den. NEVER a decimal literal, never floating point:
    -- 장기요양 is exactly 9,448/71,900 of the 건강보험료액.
    rate_num               BIGINT CHECK (rate_num IS NULL OR rate_num >= 0),
    rate_den               BIGINT CHECK (rate_den IS NULL OR rate_den > 0),
    -- Won bands (기준소득월액 하한/상한, 보험료 하한/상한, 최저임금 시간급/월환산액).
    floor_won              BIGINT CHECK (floor_won IS NULL OR floor_won >= 0),
    cap_won                BIGINT CHECK (cap_won IS NULL OR cap_won >= 0),
    basis                  TEXT NOT NULL CHECK (basis IN (
                               'PENSION_STANDARD_INCOME',
                               'MONTHLY_REMUNERATION',
                               'HEALTH_INSURANCE_PREMIUM',
                               'INDUSTRY_TARIFF',
                               'NOT_APPLICABLE')),
    bearer                 TEXT NOT NULL CHECK (bearer IN ('EMPLOYEE_WHOLE', 'HALF_EACH', 'EMPLOYER_ONLY', 'NOT_APPLICABLE')),

    -- THE CITATION. Every column below is mandatory and non-blank: a figure
    -- without an instrument, an article and an effective date is not usable
    -- evidence for release-gate condition 1.
    instrument_ko          TEXT NOT NULL CHECK (btrim(instrument_ko) <> ''),
    article_ko             TEXT NOT NULL CHECK (btrim(article_ko) <> ''),
    -- 공포번호 (법령) or 발령번호 (고시). The version anchor, together with
    -- enforced_on. A `flSeq` file handle is NEVER an anchor: three different
    -- flSeq values serve byte-identical 별표 2 content.
    promulgation_ko        TEXT NOT NULL CHECK (btrim(promulgation_ko) <> ''),
    enforced_on            DATE NOT NULL,
    source_url             TEXT NOT NULL CHECK (source_url LIKE 'https://www.law.go.kr/%'),
    retrieved_on           DATE NOT NULL,
    -- What is actually established about this row, so a reviewer is never told
    -- more than was verified.
    provenance_ko          TEXT NOT NULL CHECK (btrim(provenance_ko) <> ''),
    -- A row cannot be in force before the document that sets it. Three rows
    -- shipped backdated past their own instrument (건강보험 요율 cited a decree
    -- enforced 2026-02-19 from a 2026-01-01 row; 장기요양 one enforced 2026-05-12;
    -- 별표 2's 2026-04-23 slice one enforced 2026-05-22) — every one of them a
    -- `target=law` latest-promulgated read standing in for the text in force.
    -- The constraint is the fix; the corrected citations below merely satisfy it.
    CONSTRAINT payroll_statutory_rates_not_backdated_before_instrument
        CHECK (effective_from >= enforced_on),
    PRIMARY KEY (code, effective_from)
);

COMMENT ON TABLE payroll_statutory_rates IS
    '급여 법정요율 인용 대장. 계산은 console_payroll_domain이 수행하고, 이 표는 각 수치의 근거 문서를 제공한다. 어떤 법적 결론도 주장하지 않는다.';

INSERT INTO payroll_statutory_rates (
    code, effective_from, effective_to_exclusive, rate_num, rate_den, floor_won, cap_won,
    basis, bearer, instrument_ko, article_ko, promulgation_ko, enforced_on, source_url,
    retrieved_on, provenance_ko
) VALUES
    ('NATIONAL_PENSION_EMPLOYEE', DATE '2026-01-01', DATE '2027-01-01', 475, 10000, NULL, NULL,
     'PENSION_STANDARD_INCOME', 'EMPLOYEE_WHOLE',
     '국민연금법 부칙 <법률 제20903호, 2025. 4. 2.>',
     '제4조제1항제1호 (기준소득월액의 1만분의 475) — 본칙 제88조제3항의 1천분의 65를 2026년에 대해 대체',
     '법률 제20903호', DATE '2026-01-01', 'https://www.law.go.kr/법령/국민연금법', DATE '2026-08-01',
     '레지스터 §1 항목 1에서 부칙 조문 전문을 인용(시행 중인 파일에서 읽음, HIGH). 연간 고시가 아니라 법률 부칙의 2026→2032 법정 스케줄. 10원 미만 절사의 근거는 국민연금법 제117조(단수의 처리)가 국고금관리법을 준용하는 데 있다 — 법령ID 001781, MST 280269, efYd 20260101(법률 제21203호)로 슬라이스를 고정해 조문 전문을 읽었다. 종전의 「규정 미발견에 따른 공개 가정」은 철회. 종전 인용 2026-06-17은 같은 MST의 나중 슬라이스였다: MST 280269는 20260101·20260617 두 시행일을 가지며 efYd를 고정하지 않으면 나중 것이 온다. 응답에 함께 실리는 조문시행일자는 요청한 슬라이스를 되받는 값이므로 별개의 증거가 아니다.'),

    ('HEALTH_INSURANCE_TOTAL', DATE '2026-01-01', DATE '2027-01-01', 719, 10000, NULL, NULL,
     'MONTHLY_REMUNERATION', 'HALF_EACH',
     '국민건강보험법 시행령',
     '제44조제1항 「법 제73조제1항에 따른 직장가입자의 보험료율 … 은 각각 1만분의 719로 한다」 <개정 2025.12.23> — 분담은 법 제76조제1항 「각각 보험료액의 100분의 50씩」',
     '대통령령 제35931호', DATE '2026-01-01', 'https://www.law.go.kr/법령/국민건강보험법시행령', DATE '2026-08-01',
     '요율은 2026-01-01 시행본을 eflaw로 직접 읽음 (법령ID 002813, MST 280453, 대통령령 제35931호) — 제44조제1항의 최종 개정은 2025.12.23이며 제36116호(시행 2026-02-19)는 이 조문을 건드리지 않았다. 종전 인용 「제36116호, 시행 2026-02-19」은 이 행의 effective_from(2026-01-01)보다 7주 늦은 문서였다. 50/50 분담(제76조제1항)과 10원 절사(제107조)는 이 행의 effective_from에 시행 중인 슬라이스에서 읽음 — MST 265877, efYd 20250423, 법률 제20505호. 종전 인용 「법률 제21065호」는 시행 2026-01-02로 이 행보다 하루 늦은 슬라이스였다. 총액은 10원 절사 후 상·하한 고시로 클램프하며, 절반 분담의 반올림 단위는 미해결(Q-HALF-SHARE-ROUNDING-UNIT).'),

    ('LONG_TERM_CARE_TOTAL', DATE '2026-01-01', DATE '2026-11-27', 9448, 71900, NULL, NULL,
     'HEALTH_INSURANCE_PREMIUM', 'HALF_EACH',
     '노인장기요양보험법 시행령',
     '제4조(장기요양보험료율) 「법 제9조제1항에 따른 장기요양보험료율은 100만분의 9,448로 한다」 <개정 2025.12.30> — 법 제9조제1항에 따라 건강보험료액에 「건강보험료율 대비 장기요양보험료율의 비율」을 곱한다',
     '대통령령 제35987호', DATE '2026-01-01', 'https://www.law.go.kr/법령/노인장기요양보험법시행령', DATE '2026-08-01',
     '요율은 2026-01-01 시행본을 eflaw로 직접 읽음 (법령ID 010526, MST 281843, 대통령령 제35987호) — 제4조의 최종 개정은 2025.12.30이다. 종전 인용 「제36325호, 시행 2026-05-12」은 이 행의 effective_from보다 넉 달 늦은 문서였다. 산정기초는 보수월액이 아니라 건강보험료액. 제9조제1항 2026-05-26 시행본에는 「소수점 이하 다섯째자리에서 반올림」 문구가 없으므로 비율은 절사·반올림 없이 정확분수 — 그 문구는 법률 제21690호의 2026-11-27 시행분에서 삽입된다. 절사와 50/50 분담은 국민건강보험법을 직접 인용하지 않고 준용 조문을 인용한다: 노인장기요양보험법 제64조(→ 법 제107조), 제11조(→ 법 제76조제1항). 둘 다 법령ID 010436, MST 281921, efYd 20251230, 법률 제21257호에서 읽음.'),

    ('LONG_TERM_CARE_TOTAL', DATE '2026-11-27', DATE '2027-01-01', 1314, 10000, NULL, NULL,
     'HEALTH_INSURANCE_PREMIUM', 'HALF_EACH',
     '노인장기요양보험법',
     '제9조제1항 (… 비율을 곱하여 산정 <소수점 이하 다섯째자리에서 반올림한다>) → 0.009448/0.0719 = 0.1314',
     '법률 제21690호', DATE '2026-11-27', 'https://www.law.go.kr/법령/노인장기요양보험법', DATE '2026-08-01',
     '동일 공포번호(제21690호)의 두 시행일 슬라이스 중 나중 것. target=law은 이 텍스트를 오늘 이미 반환하므로, 시행일 기준 선택이 없으면 4개월 앞당겨 적용되는 오류가 난다. 절사·분담은 준용 조문(제64조·제11조)을 인용하며, 이 행이 도는 20261127 슬라이스(MST 286217, 법률 제21690호)에서 제11조는 자구까지 동일하고 제64조는 제91조의2·제척기간이 추가되었을 뿐 「제107조 … 단수처리 … 준용한다」는 그대로다.'),

    ('EMPLOYMENT_INSURANCE_EMPLOYEE', DATE '2026-01-01', DATE '2027-01-01', 9, 1000, NULL, NULL,
     'MONTHLY_REMUNERATION', 'EMPLOYEE_WHOLE',
     '고용보험 및 산업재해보상보험의 보험료징수 등에 관한 법률 시행령',
     '제12조제1항제2호 (실업급여 1천분의 18) — 법 제13조제2항에 따라 근로자가 2분의 1 부담',
     '대통령령 제35935호', DATE '2025-12-23',
     'https://www.law.go.kr/법령/고용보험및산업재해보상보험의보험료징수등에관한법률시행령', DATE '2026-08-01',
     '레지스터 §1 항목 5. 근로자 ½ 부담은 2026-08-01 시행본(법률 제19209호, MST 247481, efYd 20240101)에서 제13조제2항 전문을 직접 읽어 확인 — 「고용보험 가입자인 근로자가 부담하여야 하는 고용보험료는 자기의 보수총액에 제14조제1항에 따른 실업급여의 보험료율의 2분의 1을 곱한 금액으로 한다」. 레지스터의 MEDIUM은 해소됨. 단수는 Rounding::Assumed — 국민연금과 달리 절사 근거 조문이 없다. 시행령 제12조제1항제2호는 요율만 정하며 단수를 정하지 않으므로 그것을 절사 근거로 인용하지 않는다. 2026-08-01 재확인: 단수·끝수는 징수법 본문(MST 247481, efYd 20240101)과 시행령(MST 280527, efYd 20251223) 양쪽 모두 0건이고, 국고금은 시행령 제41조의5제2항제1호의 완납증명 예외(「국고금 관리법 시행령」 제31조 관서운영경비) 한 건뿐이라 단수와 무관하다. 절사 근거 없는 공개 가정으로 남는 것은 여기와 산재뿐이다(오차 <10원).'),

    ('INDUSTRIAL_ACCIDENT_EMPLOYEE', DATE '2026-01-01', DATE '2027-01-01', 0, 1, NULL, NULL,
     'INDUSTRY_TARIFF', 'EMPLOYER_ONLY',
     '고용보험 및 산업재해보상보험의 보험료징수 등에 관한 법률',
     -- 조문 전문. 「사업주가 전액 부담한다」는 조문에 없는 표현이라 인용하지 않는다.
     '제13조제5항 「제1항에 따라 사업주가 부담하여야 하는 산재보험료는 그 사업주가 경영하는 사업에 종사하는 근로자의 개인별 보수총액에 다음 각 호에 따른 산재보험료율을 곱한 금액을 합한 금액으로 한다」 — 근로자 부담분을 정한 항이 없다. 요율은 고용노동부고시 제2025-91호 별지, 유효기간 2026-12-31',
     '법률 제19209호', DATE '2024-01-01', 'https://www.law.go.kr/법령/고용보험및산업재해보상보험의보험료징수등에관한법률', DATE '2026-08-01',
     'rate_num=0은 근로자 요율이며(징수법에 근로자 산재 부담 조항 없음) 사업주 부담액이 아니다 — 사업주 총액은 별지 미파싱으로 미상이며 커널은 이를 0이 아닌 NULL로 낸다. 2026-08-01 시행본을 eflaw 2단계로 확정하여 조문 전문을 읽음 (MST 247481, efYd 20240101). 이 법령명한글로 target=eflaw lawSearch를 display=100으로 1·2면 모두 열거하면(totalCnt 174) 정확히 일치하는 행이 56건이고 그중 시행예정은 20261008·20270101 둘뿐이다 — 종전에 적힌 「58개 슬라이스」는 재현되지 않아 실측치로 대체한다(56행 = 서로 다른 시행일자 48개, 서로 다른 MST 38개). 종전 인용 「법률 제21532호, 시행 2026-10-08」은 장래효 슬라이스였고, 종전 article_ko는 인용이 아니라 의역이었다 — 둘 다 정정. 단수는 Rounding::Assumed — 종전에는 국민건강보험법 제107조를 인용했으나 그 조문은 같은 법 「보험료등」에만 미치고 징수법이 이를 준용하는 조항이 없어, 산재보험료에 적용되지 않는 법률을 인용한 것이었다.'),

    ('PENSION_STANDARD_INCOME_BAND', DATE '2025-07-01', DATE '2026-07-01', NULL, NULL, 400000, 6370000,
     'PENSION_STANDARD_INCOME', 'NOT_APPLICABLE',
     '국민연금 기준소득월액 하한액과 상한액',
     '가. 하한액 400천원 / 나. 상한액 6,370천원 · 적용기간 2025년도 7월분부터 2026년도 6월분까지',
     '보건복지부고시 제2025-24호', DATE '2025-07-01',
     'https://www.law.go.kr/LSW/admRulInfoR.do?admRulSeq=2100000254486', DATE '2026-08-01',
     'target=admrul에 &nw=2를 붙여야 연혁 고시가 보인다. 그 없이는 초과분이 조용히 숨겨진다.'),

    ('PENSION_STANDARD_INCOME_BAND', DATE '2026-07-01', DATE '2027-07-01', NULL, NULL, 410000, 6590000,
     'PENSION_STANDARD_INCOME', 'NOT_APPLICABLE',
     '국민연금 기준소득월액 하한액과 상한액',
     '가. 하한액 410천원 / 나. 상한액 6,590천원 · 적용기간 2026년도 7월분부터 2027년도 6월분까지',
     '보건복지부고시 제2026-31호 (발령 2026. 2. 2.)', DATE '2026-07-01',
     'https://www.law.go.kr/LSW/admRulInfoR.do?admRulSeq=2100000274228', DATE '2026-08-01',
     '고시 본문을 credential-free로 읽음. 국민연금법 시행령 제5조제3항의 3월 31일 기한보다 앞서 발령. 이 밴드는 clamp 경계일 뿐이며, 기준소득월액 자체는 제5조제1항의 「천원 미만을 버린 금액」이다 — 밴드 밖이면 제5조제5항에 따라 절사 없이 그 하한액/상한액이 곧 기준소득월액이 된다. 두 경계 모두 제5조제1항 각 호의 「만원 미만은 반올림」으로 만원 배수이므로 절사·클램프 순서는 관측되지 않는다.'),

    ('HEALTH_PREMIUM_BAND', DATE '2026-01-01', DATE '2027-01-01', NULL, NULL, 20160, 9183480,
     'MONTHLY_REMUNERATION', 'NOT_APPLICABLE',
     '월별 건강보험료액의 상한과 하한에 관한 고시',
     '직장가입자의 보수월액보험료 — 상한 9,183,480원 / 하한 20,160원',
     '보건복지부고시 제2025-222호 (발령 2025. 12. 24.)', DATE '2026-01-01',
     'https://www.law.go.kr/LSW/admRulInfoR.do?admRulSeq=2100000270472', DATE '2026-08-01',
     '출처 대장이 한 번도 열거하지 않은 다섯 번째 고시. 하한은 보수월액 280,389원 미만에서 구속된다.'),

    ('MINIMUM_WAGE', DATE '2026-01-01', DATE '2027-01-01', NULL, NULL, 10320, 2156880,
     'MONTHLY_REMUNERATION', 'NOT_APPLICABLE',
     '2026년 적용 최저임금 고시',
     '1. 모든 산업 시간급 10,320원 · 월환산액 2,156,880원(209시간) · 2. 사업의 종류별 구분 없이 모든 사업장에 동일하게 적용 · 3. 적용기간 2026.1.1.~2026.12.31.',
     '고용노동부고시 제2025-47호 (발령 2025. 8. 5.)', DATE '2026-01-01',
     'https://www.law.go.kr/행정규칙/2026년 적용 최저임금 고시', DATE '2026-08-01',
     'floor_won=시간급, cap_won=월환산액. 첨부 PDF를 미매핑 글리프 0개로 추출. 일급 82,560원은 고시에 없는 파생값이므로 저장하지 않는다.'),

    ('SIMPLIFIED_WITHHOLDING_TABLE', DATE '2026-01-02', DATE '2026-02-27', NULL, NULL, NULL, NULL,
     'NOT_APPLICABLE', 'NOT_APPLICABLE',
     '소득세법 시행령 별표 2 (근로소득 간이세액표)',
     '별표 2 — 별표시행일자 2026-01-02, 별표HWP파일명 law0039562025123035947KC_000200E_20260102.hwp (주3 자녀세액공제 1명 12,500원)',
     '대통령령 제35947호', DATE '2026-01-02',
     'https://www.law.go.kr/법령/소득세법시행령', DATE '2026-08-01',
     '미탑재. 2026-02-27 개정과의 사이에서 646×11 구간표는 바이트 동일하고 주3만 바뀐다 — 구간표만 읽으면 자녀 1명당 월 8,330원 과다 원천징수.'),

    ('SIMPLIFIED_WITHHOLDING_TABLE', DATE '2026-02-27', DATE '2026-04-23', NULL, NULL, NULL, NULL,
     'NOT_APPLICABLE', 'NOT_APPLICABLE',
     '소득세법 시행령 별표 2 (근로소득 간이세액표)',
     '별표 2 — 별표시행일자 2026-02-27, 별표HWP파일명 law0039562026022736129KC_000200E_20260227.hwp (주3 자녀세액공제 1명 20,830원)',
     '대통령령 제36129호', DATE '2026-02-27',
     'https://www.law.go.kr/법령/소득세법시행령', DATE '2026-08-01',
     '미탑재. 구간표는 직전 판과 동일, 자녀세액공제만 상향(1명 12,500→20,830, 2명 29,160→45,830).'),

    ('SIMPLIFIED_WITHHOLDING_TABLE', DATE '2026-04-23', DATE '2026-07-01', NULL, NULL, NULL, NULL,
     'NOT_APPLICABLE', 'NOT_APPLICABLE',
     '소득세법 시행령 별표 2 (근로소득 간이세액표)',
     '별표 2 (근로소득 간이세액표, 제189조제1항 관련) — 별표시행일자 2026-04-23, 별표HWP파일명 law0039562026042336276KC_000200E_20260423.hwp',
     '대통령령 제36276호', DATE '2026-04-23',
     'https://www.law.go.kr/법령/소득세법시행령', DATE '2026-08-01',
     '미탑재. 이 슬라이스가 존재하는 것은 별표가 시행령과 별개의 시행일자를 갖기 때문이다. 버전 앵커는 별표HWP파일명이며 flSeq가 아니다 — 파일명의 「36276」이 곧 공포번호다. 2026-04-23 시행본을 eflaw로 확인(공포·시행 같은 날). 종전 인용 「제36343호, 시행 2026-05-22」은 이 슬라이스가 시작된 뒤에 시행된 문서였다.'),

    ('SIMPLIFIED_WITHHOLDING_TABLE', DATE '2026-07-01', DATE '2027-01-01', NULL, NULL, NULL, NULL,
     'NOT_APPLICABLE', 'NOT_APPLICABLE',
     '소득세법 시행령 별표 2 (근로소득 간이세액표)',
     '별표 2 — 별표시행일자 2026-07-01, 별표HWP파일명 law0039562026052236343KC_000200E_20260701.hwp',
     '대통령령 제36343호 (공포 2026. 5. 22.)', DATE '2026-07-01',
     'https://www.law.go.kr/법령/소득세법시행령', DATE '2026-08-01',
     '미탑재 — 커널은 세액을 추정하지 않는다. 646개 구간행과 주3의 자녀세액공제를 함께 파싱해야 하며, 구간표만 읽으면 자녀 1명당 월 8,330원 과다 원천징수가 된다.');

REVOKE ALL ON payroll_statutory_rates FROM PUBLIC;
-- SELECT only, on purpose. See the table comment: a runtime-editable statutory
-- rate would bypass the professional review `change_rule` mandates.
GRANT SELECT ON payroll_statutory_rates TO console_rt;

-- ---------------------------------------------------------------------------
-- 2. The contract wage a payslip is computed from.
--
-- Append-only and effective-dated. The wage in force on date D is the row with
-- the greatest effective_from <= D, so there is no effective_to_exclusive
-- column and no UPDATE grant: history is immutable and a raise is a NEW row.
--
-- This exists because `employee_employment_profiles.base_pay` cannot serve:
-- it is NUMERIC(14,2) (wrong type for won), its period is UNLABELLED (fixtures
-- seed both 50000000 and 101.25), it has no effective dating, and it is
-- write-once. It stays HR-directory display data; payroll never reads it.
CREATE TABLE employee_contract_wages (
    id                     UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id                 UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    employee_id            UUID        NOT NULL,
    effective_from         DATE        NOT NULL,
    wage_kind              TEXT        NOT NULL CHECK (wage_kind IN ('MONTHLY', 'HOURLY')),
    -- BIGINT won. Deliberately not NUMERIC(14,2): money is never a decimal
    -- type on this path, and a sub-won contract wage is not a thing.
    amount_won             BIGINT      NOT NULL CHECK (amount_won > 0),
    -- 월 소정근로시간 (209 for a 주40시간 worker: 주 40시간 + 유급주휴 8시간).
    -- The minimum-wage comparator's denominator. Mandatory, because a
    -- minimum-wage check that cannot be made must not read as a pass.
    monthly_standard_hours INTEGER     NOT NULL CHECK (monthly_standard_hours > 0 AND monthly_standard_hours <= 400),
    source_note            TEXT        NOT NULL DEFAULT '' CHECK (char_length(source_note) <= 500),
    created_by             UUID        NOT NULL,
    created_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, employee_id, effective_from),
    FOREIGN KEY (employee_id, org_id) REFERENCES employees(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (created_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);

CREATE INDEX employee_contract_wages_in_force_idx
    ON employee_contract_wages (org_id, employee_id, effective_from DESC);

ALTER TABLE employee_contract_wages ENABLE ROW LEVEL SECURITY;
ALTER TABLE employee_contract_wages FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON employee_contract_wages
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

CREATE OR REPLACE FUNCTION employee_contract_wages_append_only()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'employee_contract_wages is append-only: % is forbidden (a raise is a new effective-dated row)', TG_OP
        USING ERRCODE = '55000';
END;
$$;
CREATE TRIGGER trg_employee_contract_wages_append_only
    BEFORE UPDATE OR DELETE ON employee_contract_wages
    FOR EACH ROW EXECUTE FUNCTION employee_contract_wages_append_only();

-- No UPDATE, no DELETE: compensation history is evidence.
GRANT SELECT, INSERT ON employee_contract_wages TO console_rt;
