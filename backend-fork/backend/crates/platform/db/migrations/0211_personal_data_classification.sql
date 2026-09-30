-- Field-level personal-data classification, and the one obligation that is
-- derivable from it today.
--
-- WHAT THIS MIGRATION ASSERTS, AND WHAT IT DOES NOT. It records, in the
-- database catalog, which columns hold which category of personal data under
-- Korean law, and it adds two read-only functions that DERIVE the 접속기록
-- retention floor from that record. It asserts NOTHING about whether this
-- product meets any statutory obligation. Every Korea compliance control in
-- docs/program/console-jurisdiction-register.json remains on HOLD; only a
-- qualified Korea legal or compliance authority may change that, and building a
-- technical control is not the same act as asserting compliance. This migration
-- deletes nothing, retains nothing automatically, and schedules no destruction.
--
-- WHY THE CATALOG AND NOT A REGISTRY TABLE. Drift has two directions:
--
--   * classification points at a column that does not exist. Postgres closes
--     this itself -- `COMMENT ON COLUMN t.c` where `c` is absent raises
--     `ERROR: column "c" of relation "t" does not exist` and this migration
--     aborts. No registry table can carry a foreign key into `pg_attribute`, so
--     a stale registry row is silent forever.
--   * column exists, classification absent. That is what
--     `console-gate-personal-data-classification` fails on, per table, against
--     a shrink-only baseline.
--
-- Both halves are mechanical. Neither is a convention anybody has to remember.
-- The catalog is also queryable at runtime, which a checked-in manifest is not,
-- and the derivation below is a runtime consumer.
--
-- THE CLASSIFICATION RULE APPLIED HERE, stated so a reviewer can disagree with
-- it in one place rather than 667:
--
--   A. In a table whose ROW IS A NATURAL PERSON, every column is at least
--      `personal`. 개인정보 보호법 제2조제1호나목 makes information 개인정보
--      when it identifies a specific individual in ready combination with other
--      information; a surrogate key, a timestamp or a status flag sitting in a
--      row beside that individual's name is exactly that combination. This is
--      deliberately over-inclusive: for erasure, DSR export and breach scoping
--      the dangerous error is under-inclusion.
--   B. In a table whose row is a thing or an event, only columns ABOUT a
--      natural person are `personal`; the rest are `none`.
--   C. Operator-typed free text and unbounded JSONB are `undeclared` -- an
--      admission that the content is not known, never `none`. `undeclared` is
--      treated as possibly-personal by every derivation.
--
-- ONE TOKEN IS DELIBERATELY UNUSED. No column here is classified `credit`.
-- Whether an HR and payroll console falls within 신용정보의 이용 및 보호에 관한
-- 법률 제2조제7호 is genuinely open: 제2조제1호의5 가목 covers 소득의 총액 및
-- 납세실적, but 제2조제1호 qualifies all of it as 상거래 information, 제20조의2
-- 제1항 expressly excludes 고용관계, and 제2조제7호 carries an unresolved
-- 대통령령으로 정하는 자 extension. Assigning that token is a legal act. It is
-- a question for counsel and it is not answered here.
--
-- SOURCES. Every citation in this file was verified against the official
-- legislation portal (law.go.kr) on 2026-08-01 -- 법령 via lawService.do, and
-- 「개인정보의 안전성 확보조치 기준」 as an 행정규칙 (--admrul), which is where
-- 고시 live and where a `target=law` query returns nothing. No community
-- compilation was used as authority. No raw API response is committed, here or
-- anywhere in this change.

-- users
COMMENT ON COLUMN users.created_at IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN users.display_name IS 'pd:personal — 직접 식별자 - 이름';
COMMENT ON COLUMN users.employee_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN users.id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN users.is_active IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN users.is_org_lead IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN users.org_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN users.phone IS 'pd:personal — 직접 식별자 - 연락처';
COMMENT ON COLUMN users.roles IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN users.team IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';

-- employees
COMMENT ON COLUMN employees.company IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employees.created_at IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employees.employee_number IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employees.employment_status IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employees.exit_date IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employees.hire_date IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employees.home_branch_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employees.id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employees.identity_name_only_merge IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employees.identity_resolution_confidence IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employees.identity_resolution_strategy IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employees.identity_review_required IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employees.job IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employees.leave_accrued IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employees.leave_remaining IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employees.leave_used IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employees.name IS 'pd:personal — 직접 식별자 - 이름';
COMMENT ON COLUMN employees.org_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employees.org_unit IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employees.position IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employees.raw_row IS 'pd:unique-id/rrn,sensitive/health,undeclared — 0066 헤더가 명시: 주민등록번호와 장애 항목이 평문 보존됨. 원본 JSONB는 알 수 없는/제한/빈 셀 값도 보존하므로 undeclared이다. hr.rs is_restricted_employee_import_header가 주민/장애를 restricted로 분류하나 정책은 retain_raw_mask_preview - 마스킹은 미리보기 투영에만 적용';
COMMENT ON COLUMN employees.source_filename IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employees.source_key IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employees.source_metadata IS 'pd:personal — 확인 결과 반입 출처 정보만 보유 - filename/sheet/row/header_row. 제한 항목 값 없음';
COMMENT ON COLUMN employees.source_row IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employees.source_sheet IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employees.updated_at IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employees.worksite_address IS 'pd:personal — 근무지 주소';
COMMENT ON COLUMN employees.worksite_name IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';

-- data_import_rows
COMMENT ON COLUMN data_import_rows.canonical_row IS 'pd:personal — 허용목록 기반 정규화 필드 - 이름 포함';
COMMENT ON COLUMN data_import_rows.created_at IS 'pd:personal — HR 직원 반입 경로가 이 표에 사람 행을 기록함 - hr.rs:2512, 8791';
COMMENT ON COLUMN data_import_rows.id IS 'pd:personal — HR 직원 반입 경로가 이 표에 사람 행을 기록함 - hr.rs:2512, 8791';
COMMENT ON COLUMN data_import_rows.org_id IS 'pd:personal — HR 직원 반입 경로가 이 표에 사람 행을 기록함 - hr.rs:2512, 8791';
COMMENT ON COLUMN data_import_rows.raw_row IS 'pd:unique-id/rrn,sensitive/health,undeclared — employees.raw_row와 동일한 워크북 원본 행; 알 수 없는/제한/빈 셀 값도 보존하는 비한정 JSONB';
COMMENT ON COLUMN data_import_rows.row_status IS 'pd:personal — HR 직원 반입 경로가 이 표에 사람 행을 기록함 - hr.rs:2512, 8791';
COMMENT ON COLUMN data_import_rows.run_id IS 'pd:personal — HR 직원 반입 경로가 이 표에 사람 행을 기록함 - hr.rs:2512, 8791';
COMMENT ON COLUMN data_import_rows.source_key IS 'pd:personal — HR 직원 반입 경로가 이 표에 사람 행을 기록함 - hr.rs:2512, 8791';
COMMENT ON COLUMN data_import_rows.source_row IS 'pd:personal — HR 직원 반입 경로가 이 표에 사람 행을 기록함 - hr.rs:2512, 8791';
COMMENT ON COLUMN data_import_rows.source_sheet IS 'pd:personal — HR 직원 반입 경로가 이 표에 사람 행을 기록함 - hr.rs:2512, 8791';
COMMENT ON COLUMN data_import_rows.validation IS 'pd:personal,undeclared — 검증 오류가 셀 값을 인용할 수 있음';

-- employee_employment_profiles
COMMENT ON COLUMN employee_employment_profiles.base_pay IS 'pd:personal — 보수. 신정법 제2조제1호의5 가목의 소득 해당 여부는 미해결 - credit 토큰은 이 증분에서 사용하지 않음';
COMMENT ON COLUMN employee_employment_profiles.created_at IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employee_employment_profiles.created_by IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employee_employment_profiles.currency IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employee_employment_profiles.employee_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employee_employment_profiles.employment_type IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employee_employment_profiles.idempotency_key IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employee_employment_profiles.org_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employee_employment_profiles.phone_e164 IS 'pd:personal — 직접 식별자 - 연락처';
COMMENT ON COLUMN employee_employment_profiles.request_hash IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';

-- employee_exit_settlement_packages
COMMENT ON COLUMN employee_exit_settlement_packages.approval_payload IS 'pd:personal,undeclared — 결재 페이로드에 자유입력 포함';
COMMENT ON COLUMN employee_exit_settlement_packages.average_daily_wage_milliwon IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employee_exit_settlement_packages.average_wage_calendar_days IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employee_exit_settlement_packages.average_wage_period_end IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employee_exit_settlement_packages.average_wage_period_start IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employee_exit_settlement_packages.average_wage_total_won IS 'pd:personal — 임금. credit 여부 미해결';
COMMENT ON COLUMN employee_exit_settlement_packages.certification_artifact IS 'pd:personal,undeclared — 증명 산출물 - 내용 비한정';
COMMENT ON COLUMN employee_exit_settlement_packages.certification_status IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employee_exit_settlement_packages.certified_package_digest IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employee_exit_settlement_packages.created_at IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employee_exit_settlement_packages.employee_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employee_exit_settlement_packages.exit_case_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employee_exit_settlement_packages.generated_at IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employee_exit_settlement_packages.id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employee_exit_settlement_packages.insurance_loss_payload IS 'pd:personal,undeclared — 확인 결과 성명/사번/소속/퇴사일 및 운영자 자유입력 reported_reason 보유. 주민등록번호는 현재 미포함';
COMMENT ON COLUMN employee_exit_settlement_packages.missing_source_fields IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employee_exit_settlement_packages.monthly_ordinary_wage_won IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employee_exit_settlement_packages.ordinary_daily_wage_won IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employee_exit_settlement_packages.org_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employee_exit_settlement_packages.service_days IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employee_exit_settlement_packages.severance_pay_won IS 'pd:personal — 퇴직금. credit 여부 미해결';
COMMENT ON COLUMN employee_exit_settlement_packages.status IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employee_exit_settlement_packages.statutory_basis IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employee_exit_settlement_packages.statutory_daily_wage_milliwon IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employee_exit_settlement_packages.submitted_at IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employee_exit_settlement_packages.submitted_by IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN employee_exit_settlement_packages.updated_at IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';

-- recruit_applicants
COMMENT ON COLUMN recruit_applicants.applicant_no IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN recruit_applicants.assessed_at IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN recruit_applicants.assessed_by IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN recruit_applicants.assessment_score IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN recruit_applicants.created_at IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN recruit_applicants.created_by IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN recruit_applicants.doc_requested IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN recruit_applicants.hired_employee_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN recruit_applicants.hold IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN recruit_applicants.id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN recruit_applicants.name IS 'pd:personal — 직접 식별자 - 이름';
COMMENT ON COLUMN recruit_applicants.org_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN recruit_applicants.posting_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN recruit_applicants.profile IS 'pd:personal,undeclared — 지원자 프로필 JSONB - 비한정. 민감정보 포함 가능';
COMMENT ON COLUMN recruit_applicants.reject_note IS 'pd:personal,undeclared — 운영자 자유입력';
COMMENT ON COLUMN recruit_applicants.reject_reason IS 'pd:personal,undeclared — 운영자 자유입력';
COMMENT ON COLUMN recruit_applicants.rejected_at IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN recruit_applicants.source_document IS 'pd:personal,undeclared — 원본 문서 참조 - 내용 비한정';
COMMENT ON COLUMN recruit_applicants.stage IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN recruit_applicants.updated_at IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';

-- attendance_direct_import_events
COMMENT ON COLUMN attendance_direct_import_events.branch_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN attendance_direct_import_events.branch_name IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN attendance_direct_import_events.check_in_at IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN attendance_direct_import_events.check_out_at IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN attendance_direct_import_events.created_at IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN attendance_direct_import_events.employee_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN attendance_direct_import_events.employee_name IS 'pd:personal — 직접 식별자 - 이름';
COMMENT ON COLUMN attendance_direct_import_events.employee_number IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN attendance_direct_import_events.fact_key IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN attendance_direct_import_events.id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN attendance_direct_import_events.import_row_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN attendance_direct_import_events.minutes_worked IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN attendance_direct_import_events.org_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN attendance_direct_import_events.run_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN attendance_direct_import_events.source_key IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN attendance_direct_import_events.source_row IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN attendance_direct_import_events.source_sha256 IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN attendance_direct_import_events.source_sheet IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN attendance_direct_import_events.work_date IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';

-- attendance_substitutions
COMMENT ON COLUMN attendance_substitutions.approval_ref IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN attendance_substitutions.branch_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN attendance_substitutions.cancel_reason IS 'pd:personal,undeclared — 운영자 자유입력';
COMMENT ON COLUMN attendance_substitutions.contract_ref IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN attendance_substitutions.cover_date IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN attendance_substitutions.covered_employee_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN attendance_substitutions.created_at IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN attendance_substitutions.created_by IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN attendance_substitutions.exception_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN attendance_substitutions.from_minutes IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN attendance_substitutions.id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN attendance_substitutions.idempotency_key IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN attendance_substitutions.org_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN attendance_substitutions.reason_detail IS 'pd:personal,undeclared — 운영자 자유입력';
COMMENT ON COLUMN attendance_substitutions.reason_kind IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN attendance_substitutions.request_fingerprint IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN attendance_substitutions.role IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN attendance_substitutions.site IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN attendance_substitutions.status IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN attendance_substitutions.to_minutes IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN attendance_substitutions.worker_employee_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN attendance_substitutions.worker_name IS 'pd:personal — 직접 식별자 - 이름';
COMMENT ON COLUMN attendance_substitutions.worker_rate IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN attendance_substitutions.worker_type IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';

-- payroll_draft_lines
COMMENT ON COLUMN payroll_draft_lines.attendance_event_count IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN payroll_draft_lines.attendance_source_row_count IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN payroll_draft_lines.blockers IS 'pd:personal,undeclared — 차단 사유 JSONB - 비한정';
COMMENT ON COLUMN payroll_draft_lines.calculation_status IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN payroll_draft_lines.created_at IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN payroll_draft_lines.employee_company IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN payroll_draft_lines.employee_display_name IS 'pd:personal — 직접 식별자 - 이름';
COMMENT ON COLUMN payroll_draft_lines.employee_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN payroll_draft_lines.employee_source_key IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN payroll_draft_lines.gross_pay_source_present IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN payroll_draft_lines.holiday_hours IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN payroll_draft_lines.id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN payroll_draft_lines.leave_remaining IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN payroll_draft_lines.leave_used IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN payroll_draft_lines.net_pay_source_present IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN payroll_draft_lines.night_hours IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN payroll_draft_lines.nts_tax_row_status IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN payroll_draft_lines.org_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN payroll_draft_lines.overtime_hours IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN payroll_draft_lines.payroll_source_row_count IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN payroll_draft_lines.regular_hours IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN payroll_draft_lines.run_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN payroll_draft_lines.source_data_import_row_ids IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN payroll_draft_lines.updated_at IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN payroll_draft_lines.work_days IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';

-- payroll_run_exceptions
COMMENT ON COLUMN payroll_run_exceptions.amount_delta_won IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN payroll_run_exceptions.carried_from_run_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN payroll_run_exceptions.created_at IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN payroll_run_exceptions.detail IS 'pd:personal,undeclared — 자유입력';
COMMENT ON COLUMN payroll_run_exceptions.employee_display_name IS 'pd:personal — 직접 식별자 - 이름';
COMMENT ON COLUMN payroll_run_exceptions.employee_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN payroll_run_exceptions.id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN payroll_run_exceptions.kind IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN payroll_run_exceptions.line_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN payroll_run_exceptions.linked_refs IS 'pd:personal,undeclared — 참조 JSONB - 비한정';
COMMENT ON COLUMN payroll_run_exceptions.org_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN payroll_run_exceptions.resolved_at IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN payroll_run_exceptions.resolved_by IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN payroll_run_exceptions.resolved_reason IS 'pd:personal,undeclared — 운영자 자유입력';
COMMENT ON COLUMN payroll_run_exceptions.run_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN payroll_run_exceptions.severity IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN payroll_run_exceptions.status IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN payroll_run_exceptions.summary_ko IS 'pd:personal,undeclared — 자유입력 요약';
COMMENT ON COLUMN payroll_run_exceptions.updated_at IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';

-- registered_devices
COMMENT ON COLUMN registered_devices.app_version IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN registered_devices.created_at IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN registered_devices.device_hash IS 'pd:personal — 이용자 단말 식별자';
COMMENT ON COLUMN registered_devices.id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN registered_devices.last_registered_at IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN registered_devices.org_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN registered_devices.platform IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN registered_devices.push_token IS 'pd:personal — 이용자 단말 식별자';
COMMENT ON COLUMN registered_devices.updated_at IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN registered_devices.user_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';

-- offline_sync_requests
COMMENT ON COLUMN offline_sync_requests.branch_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN offline_sync_requests.client_created_at IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN offline_sync_requests.completed_at IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN offline_sync_requests.device_hash IS 'pd:personal — 이용자 단말 식별자';
COMMENT ON COLUMN offline_sync_requests.http_status IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN offline_sync_requests.id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN offline_sync_requests.operation_type IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN offline_sync_requests.org_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN offline_sync_requests.payload_hash IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN offline_sync_requests.received_at IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN offline_sync_requests.request_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN offline_sync_requests.request_payload IS 'pd:personal,undeclared — 임의 동기화 페이로드 - 비한정';
COMMENT ON COLUMN offline_sync_requests.response_body IS 'pd:personal,undeclared — 임의 응답 본문 - 비한정';
COMMENT ON COLUMN offline_sync_requests.status IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN offline_sync_requests.sync_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN offline_sync_requests.user_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN offline_sync_requests.work_order_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';

-- location_pings
COMMENT ON COLUMN location_pings.accuracy_m IS 'pd:personal — 개인위치정보. 위치정보법 제2조제2호. 파기 시점은 위치정보법 제23조제1항 즉시이나 제16조제2항 확인자료는 제외 - 이 통제는 파기를 수행하지 않음';
COMMENT ON COLUMN location_pings.branch_id IS 'pd:personal — 개인위치정보. 위치정보법 제2조제2호. 파기 시점은 위치정보법 제23조제1항 즉시이나 제16조제2항 확인자료는 제외 - 이 통제는 파기를 수행하지 않음';
COMMENT ON COLUMN location_pings.id IS 'pd:personal — 개인위치정보. 위치정보법 제2조제2호. 파기 시점은 위치정보법 제23조제1항 즉시이나 제16조제2항 확인자료는 제외 - 이 통제는 파기를 수행하지 않음';
COMMENT ON COLUMN location_pings.latitude IS 'pd:personal — 개인위치정보 좌표';
COMMENT ON COLUMN location_pings.longitude IS 'pd:personal — 개인위치정보 좌표';
COMMENT ON COLUMN location_pings.on_duty IS 'pd:personal — 개인위치정보. 위치정보법 제2조제2호. 파기 시점은 위치정보법 제23조제1항 즉시이나 제16조제2항 확인자료는 제외 - 이 통제는 파기를 수행하지 않음';
COMMENT ON COLUMN location_pings.org_id IS 'pd:personal — 개인위치정보. 위치정보법 제2조제2호. 파기 시점은 위치정보법 제23조제1항 즉시이나 제16조제2항 확인자료는 제외 - 이 통제는 파기를 수행하지 않음';
COMMENT ON COLUMN location_pings.received_at IS 'pd:personal — 개인위치정보. 위치정보법 제2조제2호. 파기 시점은 위치정보법 제23조제1항 즉시이나 제16조제2항 확인자료는 제외 - 이 통제는 파기를 수행하지 않음';
COMMENT ON COLUMN location_pings.recorded_at IS 'pd:personal — 개인위치정보. 위치정보법 제2조제2호. 파기 시점은 위치정보법 제23조제1항 즉시이나 제16조제2항 확인자료는 제외 - 이 통제는 파기를 수행하지 않음';
COMMENT ON COLUMN location_pings.user_id IS 'pd:personal — 개인위치정보. 위치정보법 제2조제2호. 파기 시점은 위치정보법 제23조제1항 즉시이나 제16조제2항 확인자료는 제외 - 이 통제는 파기를 수행하지 않음';

-- location_consents
COMMENT ON COLUMN location_consents.branch_id IS 'pd:personal — 정보주체별 위치정보 동의 상태';
COMMENT ON COLUMN location_consents.created_at IS 'pd:personal — 정보주체별 위치정보 동의 상태';
COMMENT ON COLUMN location_consents.granted_at IS 'pd:personal — 정보주체별 위치정보 동의 상태';
COMMENT ON COLUMN location_consents.id IS 'pd:personal — 정보주체별 위치정보 동의 상태';
COMMENT ON COLUMN location_consents.org_id IS 'pd:personal — 정보주체별 위치정보 동의 상태';
COMMENT ON COLUMN location_consents.resumed_at IS 'pd:personal — 정보주체별 위치정보 동의 상태';
COMMENT ON COLUMN location_consents.status IS 'pd:personal — 정보주체별 위치정보 동의 상태';
COMMENT ON COLUMN location_consents.suspended_at IS 'pd:personal — 정보주체별 위치정보 동의 상태';
COMMENT ON COLUMN location_consents.updated_at IS 'pd:personal — 정보주체별 위치정보 동의 상태';
COMMENT ON COLUMN location_consents.user_id IS 'pd:personal — 정보주체별 위치정보 동의 상태';
COMMENT ON COLUMN location_consents.withdrawn_at IS 'pd:personal — 정보주체별 위치정보 동의 상태';

-- location_consent_ledger
COMMENT ON COLUMN location_consent_ledger.action IS 'pd:personal — 정보주체별 위치정보 동의 전이 원장';
COMMENT ON COLUMN location_consent_ledger.actor IS 'pd:personal — 정보주체별 위치정보 동의 전이 원장';
COMMENT ON COLUMN location_consent_ledger.branch_id IS 'pd:personal — 정보주체별 위치정보 동의 전이 원장';
COMMENT ON COLUMN location_consent_ledger.consent_id IS 'pd:personal — 정보주체별 위치정보 동의 전이 원장';
COMMENT ON COLUMN location_consent_ledger.created_at IS 'pd:personal — 정보주체별 위치정보 동의 전이 원장';
COMMENT ON COLUMN location_consent_ledger.from_status IS 'pd:personal — 정보주체별 위치정보 동의 전이 원장';
COMMENT ON COLUMN location_consent_ledger.id IS 'pd:personal — 정보주체별 위치정보 동의 전이 원장';
COMMENT ON COLUMN location_consent_ledger.occurred_at IS 'pd:personal — 정보주체별 위치정보 동의 전이 원장';
COMMENT ON COLUMN location_consent_ledger.org_id IS 'pd:personal — 정보주체별 위치정보 동의 전이 원장';
COMMENT ON COLUMN location_consent_ledger.to_status IS 'pd:personal — 정보주체별 위치정보 동의 전이 원장';
COMMENT ON COLUMN location_consent_ledger.user_id IS 'pd:personal — 정보주체별 위치정보 동의 전이 원장';

-- customer_inquiries
COMMENT ON COLUMN customer_inquiries.created_at IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN customer_inquiries.id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN customer_inquiries.listing_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN customer_inquiries.location IS 'pd:personal — 문의자 소재 정보';
COMMENT ON COLUMN customer_inquiries.message IS 'pd:personal,undeclared — 문의자 자유입력 - 비한정';
COMMENT ON COLUMN customer_inquiries.name IS 'pd:personal — 직접 식별자 - 이름';
COMMENT ON COLUMN customer_inquiries.org_id IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN customer_inquiries.phone IS 'pd:personal — 직접 식별자 - 연락처';
COMMENT ON COLUMN customer_inquiries.status IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN customer_inquiries.topic IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';
COMMENT ON COLUMN customer_inquiries.updated_at IS 'pd:personal — row is a natural person; 개보법 제2조제1호나목 결합 식별';

-- todos
COMMENT ON COLUMN todos.body IS 'pd:personal,undeclared — 운영자 자유입력';
COMMENT ON COLUMN todos.created_at IS 'pd:personal — owner_user_id 소유 행';
COMMENT ON COLUMN todos.done IS 'pd:personal — owner_user_id 소유 행';
COMMENT ON COLUMN todos.done_at IS 'pd:personal — owner_user_id 소유 행';
COMMENT ON COLUMN todos.id IS 'pd:personal — owner_user_id 소유 행';
COMMENT ON COLUMN todos.links IS 'pd:personal,undeclared — 링크 JSONB - 비한정';
COMMENT ON COLUMN todos.org_id IS 'pd:personal — owner_user_id 소유 행';
COMMENT ON COLUMN todos.owner_user_id IS 'pd:personal — owner_user_id 소유 행';
COMMENT ON COLUMN todos.scopes IS 'pd:personal,undeclared — 사용자 제공 kind/id/label을 보존하는 스코프 JSONB';
COMMENT ON COLUMN todos.updated_at IS 'pd:personal — owner_user_id 소유 행';

-- notifications
COMMENT ON COLUMN notifications.body IS 'pd:personal,undeclared — 알림 본문 - 비한정';
COMMENT ON COLUMN notifications.category IS 'pd:personal — recipient_user_id 수신 행';
COMMENT ON COLUMN notifications.created_at IS 'pd:personal — recipient_user_id 수신 행';
COMMENT ON COLUMN notifications.dedup_key IS 'pd:personal — recipient_user_id 수신 행';
COMMENT ON COLUMN notifications.id IS 'pd:personal — recipient_user_id 수신 행';
COMMENT ON COLUMN notifications.kind IS 'pd:personal — recipient_user_id 수신 행';
COMMENT ON COLUMN notifications.link IS 'pd:personal,undeclared — 링크 - 비한정';
COMMENT ON COLUMN notifications.org_id IS 'pd:personal — recipient_user_id 수신 행';
COMMENT ON COLUMN notifications.read_at IS 'pd:personal — recipient_user_id 수신 행';
COMMENT ON COLUMN notifications.recipient_user_id IS 'pd:personal — recipient_user_id 수신 행';
COMMENT ON COLUMN notifications.resolved_at IS 'pd:personal — recipient_user_id 수신 행';
COMMENT ON COLUMN notifications.resolved_by IS 'pd:personal — recipient_user_id 수신 행';
COMMENT ON COLUMN notifications.unread IS 'pd:personal — recipient_user_id 수신 행';

-- email_accounts
COMMENT ON COLUMN email_accounts.backfill_window_days IS 'pd:personal — 계정 보유 자연인에 관한 행';
COMMENT ON COLUMN email_accounts.branch_id IS 'pd:personal — 계정 보유 자연인에 관한 행';
COMMENT ON COLUMN email_accounts.claim_token IS 'pd:personal — 계정 보유 자연인에 관한 행';
COMMENT ON COLUMN email_accounts.claimed_until IS 'pd:personal — 계정 보유 자연인에 관한 행';
COMMENT ON COLUMN email_accounts.consecutive_auth_failures IS 'pd:personal — 계정 보유 자연인에 관한 행';
COMMENT ON COLUMN email_accounts.created_at IS 'pd:personal — 계정 보유 자연인에 관한 행';
COMMENT ON COLUMN email_accounts.created_by IS 'pd:personal — 계정 보유 자연인에 관한 행';
COMMENT ON COLUMN email_accounts.dek_nonce IS 'pd:personal — 계정 보유 자연인에 관한 행';
COMMENT ON COLUMN email_accounts.dek_wrapped IS 'pd:personal — 계정 보유 자연인에 관한 행';
COMMENT ON COLUMN email_accounts.display_name IS 'pd:personal — 직접 식별자 - 이름';
COMMENT ON COLUMN email_accounts.email_address IS 'pd:personal — 직접 식별자';
COMMENT ON COLUMN email_accounts.from_name IS 'pd:personal — 직접 식별자 - 이름';
COMMENT ON COLUMN email_accounts.id IS 'pd:personal — 계정 보유 자연인에 관한 행';
COMMENT ON COLUMN email_accounts.imap_dek_nonce IS 'pd:personal — 계정 보유 자연인에 관한 행';
COMMENT ON COLUMN email_accounts.imap_dek_wrapped IS 'pd:personal — 계정 보유 자연인에 관한 행';
COMMENT ON COLUMN email_accounts.imap_host IS 'pd:personal — 계정 보유 자연인에 관한 행';
COMMENT ON COLUMN email_accounts.imap_password_ct IS 'pd:personal — 계정 자격증명 - 암호문';
COMMENT ON COLUMN email_accounts.imap_password_nonce IS 'pd:personal — 계정 보유 자연인에 관한 행';
COMMENT ON COLUMN email_accounts.imap_port IS 'pd:personal — 계정 보유 자연인에 관한 행';
COMMENT ON COLUMN email_accounts.imap_security IS 'pd:personal — 계정 보유 자연인에 관한 행';
COMMENT ON COLUMN email_accounts.imap_username IS 'pd:personal — 계정 자격증명 식별자';
COMMENT ON COLUMN email_accounts.key_version IS 'pd:personal — 계정 보유 자연인에 관한 행';
COMMENT ON COLUMN email_accounts.last_sync_at IS 'pd:personal — 계정 보유 자연인에 관한 행';
COMMENT ON COLUMN email_accounts.last_sync_error IS 'pd:personal — 계정 보유 자연인에 관한 행';
COMMENT ON COLUMN email_accounts.org_id IS 'pd:personal — 계정 보유 자연인에 관한 행';
COMMENT ON COLUMN email_accounts.smtp_host IS 'pd:personal — 계정 보유 자연인에 관한 행';
COMMENT ON COLUMN email_accounts.smtp_password_ct IS 'pd:personal — 계정 자격증명 - 암호문';
COMMENT ON COLUMN email_accounts.smtp_password_nonce IS 'pd:personal — 계정 보유 자연인에 관한 행';
COMMENT ON COLUMN email_accounts.smtp_port IS 'pd:personal — 계정 보유 자연인에 관한 행';
COMMENT ON COLUMN email_accounts.smtp_security IS 'pd:personal — 계정 보유 자연인에 관한 행';
COMMENT ON COLUMN email_accounts.smtp_username IS 'pd:personal — 계정 자격증명 식별자';
COMMENT ON COLUMN email_accounts.status IS 'pd:personal — 계정 보유 자연인에 관한 행';
COMMENT ON COLUMN email_accounts.sync_cadence_secs IS 'pd:personal — 계정 보유 자연인에 관한 행';
COMMENT ON COLUMN email_accounts.sync_status IS 'pd:personal — 계정 보유 자연인에 관한 행';
COMMENT ON COLUMN email_accounts.updated_at IS 'pd:personal — 계정 보유 자연인에 관한 행';

-- email_messages
COMMENT ON COLUMN email_messages.account_id IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_messages.answered IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_messages.bcc_addresses IS 'pd:personal — 직접 식별자';
COMMENT ON COLUMN email_messages.body_html IS 'pd:personal,undeclared — 비한정 본문. 고유식별정보나 민감정보를 담을 수 있으나 선언되지 않음';
COMMENT ON COLUMN email_messages.body_text IS 'pd:personal,undeclared — 비한정 본문. 고유식별정보나 민감정보를 담을 수 있으나 선언되지 않음';
COMMENT ON COLUMN email_messages.cc_addresses IS 'pd:personal — 직접 식별자';
COMMENT ON COLUMN email_messages.created_at IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_messages.direction IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_messages.draft IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_messages.flagged IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_messages.folder_id IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_messages.from_address IS 'pd:personal — 직접 식별자';
COMMENT ON COLUMN email_messages.from_name IS 'pd:personal — 직접 식별자 - 이름';
COMMENT ON COLUMN email_messages.has_attachments IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_messages.id IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_messages.imap_uid IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_messages.imap_uid_validity IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_messages.in_reply_to IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_messages.message_id IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_messages.org_id IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_messages.received_at IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_messages.references_ids IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_messages.search_vector IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_messages.seen IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_messages.send_error IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_messages.send_status IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_messages.sent_at IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_messages.snippet IS 'pd:personal,undeclared — 사람이 작성한 비한정 내용';
COMMENT ON COLUMN email_messages.subject IS 'pd:personal,undeclared — 사람이 작성한 비한정 내용';
COMMENT ON COLUMN email_messages.thread_id IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_messages.to_addresses IS 'pd:personal — 직접 식별자';
COMMENT ON COLUMN email_messages.updated_at IS 'pd:personal — 송수신 자연인에 관한 행';

-- email_threads
COMMENT ON COLUMN email_threads.account_id IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_threads.assigned_user_id IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_threads.created_at IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_threads.has_attachments IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_threads.id IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_threads.is_flagged IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_threads.last_message_at IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_threads.linked_customer_id IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_threads.linked_work_order_id IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_threads.message_count IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_threads.normalized_subject IS 'pd:personal,undeclared — 사람이 작성한 비한정 내용';
COMMENT ON COLUMN email_threads.org_id IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_threads.subject IS 'pd:personal,undeclared — 사람이 작성한 비한정 내용';
COMMENT ON COLUMN email_threads.unread_count IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_threads.updated_at IS 'pd:personal — 송수신 자연인에 관한 행';

-- email_attachments
COMMENT ON COLUMN email_attachments.content_id IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_attachments.content_type IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_attachments.created_at IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_attachments.filename IS 'pd:personal,undeclared — 사용자 지정 파일명 - 비한정';
COMMENT ON COLUMN email_attachments.id IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_attachments.is_inline IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_attachments.message_id IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_attachments.org_id IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_attachments.s3_key IS 'pd:personal,undeclared — 첨부 원본 위치 - 내용 비한정';
COMMENT ON COLUMN email_attachments.size_bytes IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_attachments.sort_order IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN email_attachments.upload_state IS 'pd:personal — 송수신 자연인에 관한 행';

-- mailboxes
COMMENT ON COLUMN mailboxes.created_at IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN mailboxes.created_by IS 'pd:personal — 행위자 참조';
COMMENT ON COLUMN mailboxes.display_name IS 'pd:personal — 직접 식별자 - 이름';
COMMENT ON COLUMN mailboxes.domain_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN mailboxes.id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN mailboxes.legal_hold IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN mailboxes.local_part IS 'pd:personal — 직접 식별자';
COMMENT ON COLUMN mailboxes.mailbox_kind IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN mailboxes.org_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN mailboxes.owner_user_id IS 'pd:personal — 정보주체 참조';
COMMENT ON COLUMN mailboxes.quota_bytes IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN mailboxes.retention_policy_key IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN mailboxes.status IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN mailboxes.updated_at IS 'pd:none — structural or non-personal attribute of a non-person row';

-- mailbox_messages
COMMENT ON COLUMN mailbox_messages.answered IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN mailbox_messages.created_at IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN mailbox_messages.direction IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN mailbox_messages.domain_id IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN mailbox_messages.draft IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN mailbox_messages.flagged IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN mailbox_messages.folder_role IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN mailbox_messages.has_attachments IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN mailbox_messages.id IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN mailbox_messages.in_reply_to IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN mailbox_messages.mailbox_id IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN mailbox_messages.normalized_subject IS 'pd:personal,undeclared — 사람이 작성한 비한정 내용';
COMMENT ON COLUMN mailbox_messages.org_id IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN mailbox_messages.raw_object_key IS 'pd:personal,undeclared — 원본 RFC822 위치 - 내용 비한정';
COMMENT ON COLUMN mailbox_messages.raw_size_bytes IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN mailbox_messages.received_at IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN mailbox_messages.references_ids IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN mailbox_messages.rfc_message_id IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN mailbox_messages.seen IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN mailbox_messages.sensitivity IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN mailbox_messages.sent_at IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN mailbox_messages.updated_at IS 'pd:personal — 송수신 자연인에 관한 행';

-- mailbox_deliveries
COMMENT ON COLUMN mailbox_deliveries.accepted_at IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN mailbox_deliveries.attempt_count IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN mailbox_deliveries.completed_at IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN mailbox_deliveries.created_at IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN mailbox_deliveries.direction IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN mailbox_deliveries.envelope_from IS 'pd:personal — 직접 식별자';
COMMENT ON COLUMN mailbox_deliveries.error_payload IS 'pd:personal,undeclared — 오류 페이로드 - 비한정';
COMMENT ON COLUMN mailbox_deliveries.id IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN mailbox_deliveries.locked_by IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN mailbox_deliveries.locked_until IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN mailbox_deliveries.mailbox_message_id IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN mailbox_deliveries.next_attempt_at IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN mailbox_deliveries.org_id IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN mailbox_deliveries.queue_key IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN mailbox_deliveries.recipient_domain IS 'pd:personal — 직접 식별자 구성요소';
COMMENT ON COLUMN mailbox_deliveries.recipient_local_part IS 'pd:personal — 직접 식별자';
COMMENT ON COLUMN mailbox_deliveries.rejection_reason IS 'pd:personal,undeclared — 거부 사유 - 비한정';
COMMENT ON COLUMN mailbox_deliveries.remote_addr_hash IS 'pd:personal — 접속지 정보 파생값';
COMMENT ON COLUMN mailbox_deliveries.status IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN mailbox_deliveries.updated_at IS 'pd:personal — 송수신 자연인에 관한 행';

-- messenger_messages
COMMENT ON COLUMN messenger_messages.body IS 'pd:personal,undeclared — 사람이 작성한 비한정 내용';
COMMENT ON COLUMN messenger_messages.branch_id IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN messenger_messages.created_at IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN messenger_messages.id IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN messenger_messages.org_id IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN messenger_messages.quoted_message_id IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN messenger_messages.search_vector IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN messenger_messages.sender_id IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN messenger_messages.sent_at IS 'pd:personal — 송수신 자연인에 관한 행';
COMMENT ON COLUMN messenger_messages.thread_id IS 'pd:personal — 송수신 자연인에 관한 행';

-- notices
COMMENT ON COLUMN notices.audience_scope IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN notices.author_user_id IS 'pd:personal — 작성자 참조';
COMMENT ON COLUMN notices.body IS 'pd:undeclared — 운영자 자유입력 - 사람 이름을 담을 수 있음';
COMMENT ON COLUMN notices.category IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN notices.code IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN notices.created_at IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN notices.id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN notices.org_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN notices.published_at IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN notices.status IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN notices.title IS 'pd:undeclared — 운영자 자유입력 - 사람 이름을 담을 수 있음';
COMMENT ON COLUMN notices.updated_at IS 'pd:none — structural or non-personal attribute of a non-person row';

-- support_tickets
COMMENT ON COLUMN support_tickets.assignee_user_id IS 'pd:personal — 담당자 참조';
COMMENT ON COLUMN support_tickets.body IS 'pd:personal,undeclared — 요청자 자유입력 - 비한정';
COMMENT ON COLUMN support_tickets.branch_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN support_tickets.category IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN support_tickets.closed_at IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN support_tickets.created_at IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN support_tickets.customer_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN support_tickets.due_at IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN support_tickets.id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN support_tickets.org_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN support_tickets.origin IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN support_tickets.priority IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN support_tickets.requester_contact IS 'pd:personal — 0022 헤더가 PII로 표시한 요청자 연락처';
COMMENT ON COLUMN support_tickets.requester_name IS 'pd:personal — 0022 헤더가 PII로 표시한 요청자 정보';
COMMENT ON COLUMN support_tickets.requester_user_id IS 'pd:personal — 정보주체 참조';
COMMENT ON COLUMN support_tickets.resolved_at IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN support_tickets.site_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN support_tickets.status IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN support_tickets.title IS 'pd:undeclared — 요청자 자유입력';
COMMENT ON COLUMN support_tickets.updated_at IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN support_tickets.work_order_id IS 'pd:none — structural or non-personal attribute of a non-person row';

-- support_ticket_comments
COMMENT ON COLUMN support_ticket_comments.author_user_id IS 'pd:personal — 작성자 참조';
COMMENT ON COLUMN support_ticket_comments.body IS 'pd:personal,undeclared — 사람이 작성한 비한정 내용';
COMMENT ON COLUMN support_ticket_comments.created_at IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN support_ticket_comments.id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN support_ticket_comments.is_internal_note IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN support_ticket_comments.org_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN support_ticket_comments.ticket_id IS 'pd:none — structural or non-personal attribute of a non-person row';

-- outsource_vendors
COMMENT ON COLUMN outsource_vendors.branch_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN outsource_vendors.contact IS 'pd:personal — 담당자 연락처';
COMMENT ON COLUMN outsource_vendors.created_at IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN outsource_vendors.id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN outsource_vendors.name IS 'pd:personal — 개인사업자인 경우 개인정보';
COMMENT ON COLUMN outsource_vendors.org_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN outsource_vendors.updated_at IS 'pd:none — structural or non-personal attribute of a non-person row';

-- registry_customers
COMMENT ON COLUMN registry_customers.branch_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_customers.created_at IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_customers.id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_customers.name IS 'pd:personal — 개인사업자인 경우 개인정보';
COMMENT ON COLUMN registry_customers.org_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_customers.updated_at IS 'pd:none — structural or non-personal attribute of a non-person row';

-- registry_sites
COMMENT ON COLUMN registry_sites.address IS 'pd:personal — 담당자와 결합하여 개인의 근무 장소를 특정함';
COMMENT ON COLUMN registry_sites.branch_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_sites.city IS 'pd:personal — 주소 구성요소';
COMMENT ON COLUMN registry_sites.contact_email IS 'pd:personal — 직접 식별자 - 연락처';
COMMENT ON COLUMN registry_sites.contact_name IS 'pd:personal — 직접 식별자 - 이름';
COMMENT ON COLUMN registry_sites.contact_phone IS 'pd:personal — 직접 식별자 - 연락처';
COMMENT ON COLUMN registry_sites.created_at IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_sites.customer_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_sites.geofence_radius_m IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_sites.id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_sites.latitude IS 'pd:personal — 현장 좌표 - 담당자와 결합 시 개인 소재 특정';
COMMENT ON COLUMN registry_sites.longitude IS 'pd:personal — 현장 좌표 - 담당자와 결합 시 개인 소재 특정';
COMMENT ON COLUMN registry_sites.name IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_sites.org_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_sites.postal_code IS 'pd:personal — 주소 구성요소';
COMMENT ON COLUMN registry_sites.province IS 'pd:personal — 주소 구성요소';
COMMENT ON COLUMN registry_sites.updated_at IS 'pd:none — structural or non-personal attribute of a non-person row';

-- registry_equipment
COMMENT ON COLUMN registry_equipment.acquisition_cost_won IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_equipment.acquisition_date IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_equipment.asset_owner IS 'pd:personal — 개인인 경우 개인정보';
COMMENT ON COLUMN registry_equipment.asset_registered_on IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_equipment.branch_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_equipment.created_at IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_equipment.customer_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_equipment.equipment_no IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_equipment.hours IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_equipment.id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_equipment.insured IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_equipment.insured_party IS 'pd:personal — 개인인 경우 개인정보';
COMMENT ON COLUMN registry_equipment.insurer IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_equipment.kind_code IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_equipment.maker IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_equipment.management_no IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_equipment.manager_name IS 'pd:personal — 직접 식별자 - 이름';
COMMENT ON COLUMN registry_equipment.manufacturer_code IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_equipment.model IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_equipment.note IS 'pd:undeclared — 운영자 자유입력 - 사람 이름을 담을 수 있음';
COMMENT ON COLUMN registry_equipment.operation_shift IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_equipment.org_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_equipment.placement_location IS 'pd:personal — 관리자와 결합 시 개인 소재 특정';
COMMENT ON COLUMN registry_equipment.placement_no IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_equipment.policy_holder IS 'pd:personal — 개인인 경우 개인정보';
COMMENT ON COLUMN registry_equipment.power_code IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_equipment.power_label IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_equipment.rental_fee IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_equipment.rental_started_on IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_equipment.residual_value IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_equipment.site_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_equipment.source_row IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_equipment.source_sheet IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_equipment.specification IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_equipment.status IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_equipment.ton_milli IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_equipment.ton_text IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_equipment.updated_at IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_equipment.vehicle_registration_no IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_equipment.vehicle_value IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_equipment.vin IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN registry_equipment.year IS 'pd:none — structural or non-personal attribute of a non-person row';

-- equipment_3r_rental_cases
COMMENT ON COLUMN equipment_3r_rental_cases.approval_decision IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN equipment_3r_rental_cases.approval_reason IS 'pd:undeclared — 운영자 자유입력';
COMMENT ON COLUMN equipment_3r_rental_cases.approved_at IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN equipment_3r_rental_cases.approved_by IS 'pd:personal — 행위자 참조';
COMMENT ON COLUMN equipment_3r_rental_cases.branch_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN equipment_3r_rental_cases.carrier_name IS 'pd:personal — 직접 식별자 - 이름';
COMMENT ON COLUMN equipment_3r_rental_cases.created_at IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN equipment_3r_rental_cases.created_by IS 'pd:personal — 행위자 참조';
COMMENT ON COLUMN equipment_3r_rental_cases.currency_code IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN equipment_3r_rental_cases.customer_name IS 'pd:personal — 직접 식별자 - 이름';
COMMENT ON COLUMN equipment_3r_rental_cases.dispatched_at IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN equipment_3r_rental_cases.duration_months IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN equipment_3r_rental_cases.handed_over_at IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN equipment_3r_rental_cases.handover_evidence_object_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN equipment_3r_rental_cases.id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN equipment_3r_rental_cases.idempotency_key IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN equipment_3r_rental_cases.monthly_rate_minor IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN equipment_3r_rental_cases.org_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN equipment_3r_rental_cases.recipient_name IS 'pd:personal — 직접 식별자 - 이름';
COMMENT ON COLUMN equipment_3r_rental_cases.request_fingerprint IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN equipment_3r_rental_cases.returned_at IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN equipment_3r_rental_cases.site_reference IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN equipment_3r_rental_cases.status IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN equipment_3r_rental_cases.unit_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN equipment_3r_rental_cases.updated_at IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN equipment_3r_rental_cases.vehicle_reference IS 'pd:none — structural or non-personal attribute of a non-person row';

-- equipment_3r_dispositions
COMMENT ON COLUMN equipment_3r_dispositions.assessment_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN equipment_3r_dispositions.branch_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN equipment_3r_dispositions.buyer_name IS 'pd:personal — 직접 식별자 - 이름';
COMMENT ON COLUMN equipment_3r_dispositions.case_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN equipment_3r_dispositions.completed_at IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN equipment_3r_dispositions.completed_by IS 'pd:personal — 행위자 참조';
COMMENT ON COLUMN equipment_3r_dispositions.cost_minor IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN equipment_3r_dispositions.created_at IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN equipment_3r_dispositions.id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN equipment_3r_dispositions.kind IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN equipment_3r_dispositions.org_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN equipment_3r_dispositions.sale_amount_minor IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN equipment_3r_dispositions.status IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN equipment_3r_dispositions.unit_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN equipment_3r_dispositions.updated_at IS 'pd:none — structural or non-personal attribute of a non-person row';

-- p1_dispatches
COMMENT ON COLUMN p1_dispatches.accept_window_ends_at IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN p1_dispatches.accept_window_started_at IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN p1_dispatches.auto_assigned_mechanic_id IS 'pd:personal — 배정 기사 참조';
COMMENT ON COLUMN p1_dispatches.branch_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN p1_dispatches.created_at IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN p1_dispatches.created_by IS 'pd:personal — 행위자 참조';
COMMENT ON COLUMN p1_dispatches.id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN p1_dispatches.incident_latitude IS 'pd:personal — 배정 기사와 결합 시 개인 소재 특정';
COMMENT ON COLUMN p1_dispatches.incident_longitude IS 'pd:personal — 배정 기사와 결합 시 개인 소재 특정';
COMMENT ON COLUMN p1_dispatches.include_region IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN p1_dispatches.manager_force_pending_at IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN p1_dispatches.manual_call_cleared_at IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN p1_dispatches.manual_call_required_at IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN p1_dispatches.org_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN p1_dispatches.status IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN p1_dispatches.updated_at IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN p1_dispatches.work_order_id IS 'pd:none — structural or non-personal attribute of a non-person row';

-- docs_evidence_custody_events
COMMENT ON COLUMN docs_evidence_custody_events.actor_user_id IS 'pd:personal — 행위자 참조';
COMMENT ON COLUMN docs_evidence_custody_events.audit_event_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN docs_evidence_custody_events.created_at IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN docs_evidence_custody_events.event_digest_sha256 IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN docs_evidence_custody_events.evidence_object_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN docs_evidence_custody_events.from_custodian IS 'pd:personal,undeclared — 보관자 - 자연인일 수 있고 임의 JSON 객체를 보존';
COMMENT ON COLUMN docs_evidence_custody_events.id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN docs_evidence_custody_events.location_label IS 'pd:personal — 보관자와 결합 시 개인 소재 특정';
COMMENT ON COLUMN docs_evidence_custody_events.occurred_at IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN docs_evidence_custody_events.org_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN docs_evidence_custody_events.previous_event_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN docs_evidence_custody_events.reason IS 'pd:undeclared — 운영자 자유입력';
COMMENT ON COLUMN docs_evidence_custody_events.source_ref IS 'pd:undeclared — 출처 참조 - 비한정';
COMMENT ON COLUMN docs_evidence_custody_events.stage IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN docs_evidence_custody_events.to_custodian IS 'pd:personal,undeclared — 보관자 - 자연인일 수 있고 임의 JSON 객체를 보존';

-- work_diary_drafts
COMMENT ON COLUMN work_diary_drafts.body IS 'pd:personal,undeclared — 작업 일지 본문 - 근로자 이름을 담음';
COMMENT ON COLUMN work_diary_drafts.branch_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN work_diary_drafts.confirmed_at IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN work_diary_drafts.confirmed_by IS 'pd:personal — 행위자 참조';
COMMENT ON COLUMN work_diary_drafts.created_at IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN work_diary_drafts.diary_date IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN work_diary_drafts.edited_at IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN work_diary_drafts.edited_by IS 'pd:personal — 행위자 참조';
COMMENT ON COLUMN work_diary_drafts.generated_at IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN work_diary_drafts.generated_by IS 'pd:personal — 행위자 참조';
COMMENT ON COLUMN work_diary_drafts.id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN work_diary_drafts.org_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN work_diary_drafts.scope_key IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN work_diary_drafts.status IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN work_diary_drafts.updated_at IS 'pd:none — structural or non-personal attribute of a non-person row';

-- audit_events
COMMENT ON COLUMN audit_events.action IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN audit_events.actor IS 'pd:personal — 행위자 참조';
COMMENT ON COLUMN audit_events.after_snap IS 'pd:undeclared — 임의 행 스냅샷. 분류된 어떤 열의 값도 담을 수 있으며 employees.raw_row를 포함할 수 있음 - 선언되지 않음';
COMMENT ON COLUMN audit_events.anomaly IS 'pd:undeclared — 이상 징후 페이로드 - 비한정';
COMMENT ON COLUMN audit_events.auth_method IS 'pd:personal — 행위자 인증 수단';
COMMENT ON COLUMN audit_events.before_snap IS 'pd:undeclared — 임의 행 스냅샷. 분류된 어떤 열의 값도 담을 수 있으며 employees.raw_row를 포함할 수 있음 - 선언되지 않음';
COMMENT ON COLUMN audit_events.branch_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN audit_events.classification_badges IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN audit_events.created_at IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN audit_events.device IS 'pd:personal — 이용자 단말 정보';
COMMENT ON COLUMN audit_events.id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN audit_events.ip IS 'pd:personal — 접속지 정보. 0149가 감사 맥락으로 추가';
COMMENT ON COLUMN audit_events.occurred_at IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN audit_events.org_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN audit_events.reason IS 'pd:undeclared — 운영자 자유입력';
COMMENT ON COLUMN audit_events.span_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN audit_events.target_id IS 'pd:undeclared — 다형 참조 - 정보주체를 가리킬 수 있음';
COMMENT ON COLUMN audit_events.target_type IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN audit_events.trace_id IS 'pd:none — structural or non-personal attribute of a non-person row';
COMMENT ON COLUMN audit_events.user_agent IS 'pd:personal — 이용자 단말 정보';

-- ---------------------------------------------------------------------------
-- THE DERIVED CONTROL: the 접속기록 retention floor, read out of the catalog.
-- ---------------------------------------------------------------------------
--
-- Instrument: 「개인정보의 안전성 확보조치 기준」, 개인정보보호위원회 고시
-- 제2026-9호, 발령ㆍ시행 2026-07-01, 행정규칙일련번호 2100000281400,
-- 행정규칙ID 73493. Retrieved 2026-08-01 from the official legislation portal
-- as an 행정규칙 (--admrul); a `target=law` query for a 고시 returns nothing,
-- which means wrong target, not absent.
--
-- 제8조제1항, verbatim in the operative part:
--   개인정보처리자는 개인정보처리시스템에 접속한 자(다만, 정보주체는 제외한다)의
--   접속기록을 1년 이상 보관ㆍ관리하여야 한다. 다만, 다음 각 호의 어느 하나에
--   해당하는 경우에는 2년 이상 보관ㆍ관리하여야 한다.
--     2. 고유식별정보 또는 민감정보를 처리하는 개인정보처리시스템에 해당하는 경우
--
-- WHY THIS OBLIGATION AND NOT ANOTHER. It is the only one in the verified set
-- that is fully determined by column class with no open legal question standing
-- in front of it. The encryption duty is not (고시 제7조제2항 and 제7조제3항
-- split on 이용자 versus 이용자가 아닌 정보주체, and whether this console's
-- subjects are 정보통신서비스 이용자 is unresolved). Breach scoping is not
-- (개보법 제34조제2항, 신설 2026.3.10 시행 2026-09-11, is scoped by 개인정보의
-- 유형 as fixed by 대통령령, and the 시행령 in force references only 제34조
-- 제1항). The 개인신용정보 clock is not (see the header). And this one is purely
-- additive: it retains more and deletes nothing, so it cannot destroy data
-- while the erasure-versus-PITR question in ADR-0037 is open.
--
-- THE CORRECTION THIS ENCODES. 제8조제1항제2호 reads 고유식별정보 **또는**
-- 민감정보. A derivation that fires the two-year floor only off 고유식별정보
-- UNDER-RETAINS for a system that holds 민감정보 and no 고유식별정보. The
-- 민감정보 limb is therefore load-bearing, and a test asserts it fires on its
-- own.
--
-- WHAT THIS CANNOT SEE, named so no later reader assumes coverage:
--   * 제8조제1항제1호 fires on 5만명 이상의 정보주체 -- row cardinality, not
--     column class. Not expressible here.
--   * 제8조제1항제3호 fires on 기간통신사업자 registration status -- an
--     organisation property, not column class. Not expressible here.
-- Both belong at system or tenant level and are outside this control by
-- construction.
--
-- SOUNDNESS UNDER A PARTIAL CLASSIFICATION. With a non-empty baseline this is a
-- LOWER BOUND: it can under-report, never over-report. Under-retention is the
-- legally dangerous direction and two years is the higher tier in this
-- derivation, so once a single 고유식별정보 or 민감정보 column is classified the
-- answer is already at that tier and further classification cannot move it
-- down. That is why a partial
-- rollout is safe for this obligation and would not be safe for a destruction
-- schedule.
--
-- No new table is created, so no org-scoping or FORCE ROW LEVEL SECURITY
-- obligation is incurred. That is a deliberate consequence of choosing the
-- catalog as the store. The catalog is cluster-wide, not tenant-scoped: this
-- reports a property of the SCHEMA, which is identical for every tenant, and it
-- exposes no tenant row data.

-- `pg_catalog` FIRST, and that order is the security property, not style.
-- These are SECURITY DEFINER, so they execute with the definer's rights. With
-- `public` ahead of `pg_catalog`, the unqualified `string_to_array`,
-- `regexp_match` and `unnest` below resolve against `public` first — and
-- anyone who can CREATE in `public` can plant a function of the same name and
-- signature and have it run as the definer. `pg_temp` stays last for the same
-- reason at one further remove.
-- payroll_statutory_rates
-- Global statutory figures and source citations are shared reference data, not
-- rows about a natural person.
COMMENT ON COLUMN payroll_statutory_rates.code IS 'pd:none — global statutory-rate key; no natural-person row';
COMMENT ON COLUMN payroll_statutory_rates.effective_from IS 'pd:none — statutory effective date; no natural-person row';
COMMENT ON COLUMN payroll_statutory_rates.effective_to_exclusive IS 'pd:none — statutory effective-date bound; no natural-person row';
COMMENT ON COLUMN payroll_statutory_rates.rate_num IS 'pd:none — statutory rate numerator; no natural-person row';
COMMENT ON COLUMN payroll_statutory_rates.rate_den IS 'pd:none — statutory rate denominator; no natural-person row';
COMMENT ON COLUMN payroll_statutory_rates.floor_won IS 'pd:none — statutory amount floor; no natural-person row';
COMMENT ON COLUMN payroll_statutory_rates.cap_won IS 'pd:none — statutory amount cap; no natural-person row';
COMMENT ON COLUMN payroll_statutory_rates.basis IS 'pd:none — statutory calculation-basis enum; no natural-person row';
COMMENT ON COLUMN payroll_statutory_rates.bearer IS 'pd:none — statutory cost-bearer enum; no natural-person row';
COMMENT ON COLUMN payroll_statutory_rates.instrument_ko IS 'pd:none — public statutory instrument citation; no natural-person row';
COMMENT ON COLUMN payroll_statutory_rates.article_ko IS 'pd:none — public statutory article citation; no natural-person row';
COMMENT ON COLUMN payroll_statutory_rates.promulgation_ko IS 'pd:none — public promulgation identifier; no natural-person row';
COMMENT ON COLUMN payroll_statutory_rates.enforced_on IS 'pd:none — statutory enforcement date; no natural-person row';
COMMENT ON COLUMN payroll_statutory_rates.source_url IS 'pd:none — public law source URL; no natural-person row';
COMMENT ON COLUMN payroll_statutory_rates.retrieved_on IS 'pd:none — public-source retrieval date; no natural-person row';
COMMENT ON COLUMN payroll_statutory_rates.provenance_ko IS 'pd:none — public-source provenance note; no natural-person row';

-- employee_contract_wages
-- Each row is an effective-dated wage record for one employee. Identifiers,
-- dates, wage terms, and creator metadata are personal through that linkage;
-- source_note is additionally unbounded operator-authored text.
COMMENT ON COLUMN employee_contract_wages.id IS 'pd:personal — employee wage-record identifier; linkable to a natural person';
COMMENT ON COLUMN employee_contract_wages.org_id IS 'pd:personal — tenant component of an employee wage record';
COMMENT ON COLUMN employee_contract_wages.employee_id IS 'pd:personal — direct employee linkage';
COMMENT ON COLUMN employee_contract_wages.effective_from IS 'pd:personal — effective date of an individual wage term';
COMMENT ON COLUMN employee_contract_wages.wage_kind IS 'pd:personal — individual contract wage basis';
COMMENT ON COLUMN employee_contract_wages.amount_won IS 'pd:personal — individual contract wage amount; credit classification remains HOLD';
COMMENT ON COLUMN employee_contract_wages.monthly_standard_hours IS 'pd:personal — individual contractual working-hours term';
COMMENT ON COLUMN employee_contract_wages.source_note IS 'pd:personal,undeclared — operator-authored wage-source note with unbounded content';
COMMENT ON COLUMN employee_contract_wages.created_by IS 'pd:personal — user identifier for the person who recorded the wage term';
COMMENT ON COLUMN employee_contract_wages.created_at IS 'pd:personal — timestamp associated with employee and recording user';

CREATE OR REPLACE FUNCTION personal_data_columns()
RETURNS TABLE (schema_name TEXT, rel_name TEXT, col_name TEXT, tokens TEXT[])
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, public, pg_temp
AS $$
    SELECT
        n.nspname::TEXT,
        c.relname::TEXT,
        a.attname::TEXT,
        string_to_array(
            (regexp_match(d.description, '^pd:([^[:space:]]*)'))[1],
            ','
        )
    FROM pg_catalog.pg_class AS c
    JOIN pg_catalog.pg_namespace AS n ON n.oid = c.relnamespace
    JOIN pg_catalog.pg_attribute AS a ON a.attrelid = c.oid
    JOIN pg_catalog.pg_description AS d
      ON d.objoid = c.oid AND d.objsubid = a.attnum
    WHERE n.nspname <> 'pg_catalog'
      AND c.relpersistence <> 't'
      AND c.relkind IN ('r', 'p', 'm', 'f')
      AND a.attnum > 0
      AND NOT a.attisdropped
      AND d.description LIKE 'pd:%';
$$;

COMMENT ON FUNCTION personal_data_columns() IS
    'Field-level personal-data classification, read from the database catalog. '
    'Each row is a schema-qualified column carrying a pd: marker and its token '
    'list. The reader covers the same non-pg_catalog, non-temporary r/p/m/f '
    'relation universe as the catalog completeness assertion. Records what a '
    'column holds; asserts nothing about whether any obligation is met.';

CREATE OR REPLACE FUNCTION access_log_retention_floor_years()
RETURNS INTEGER
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, public, pg_temp
AS $$
    SELECT CASE
        WHEN EXISTS (
            SELECT 1
            -- Schema-qualified: `personal_data_columns` lives in `public`, so
            -- ordering `pg_catalog` first does not protect this call the way
            -- it protects the built-ins. Naming the schema does.
            FROM public.personal_data_columns() AS pdc,
                 LATERAL unnest(pdc.tokens) AS token
            WHERE token = 'unique-id' OR token LIKE 'unique-id/%'
               OR token = 'sensitive' OR token LIKE 'sensitive/%'
        ) THEN 2
        ELSE 1
    END;
$$;

COMMENT ON FUNCTION access_log_retention_floor_years() IS
    '접속기록 보관 기간의 하한(원문 단위: 년). 「개인정보의 안전성 확보조치 기준」 '
    '개인정보보호위원회 고시 제2026-9호(발령ㆍ시행 2026-07-01, '
    '행정규칙일련번호 2100000281400) 제8조제1항: 1년 이상, 다만 제2호의 '
    '고유식별정보 또는 민감정보를 처리하는 경우 2년 이상. 법문이 년 단위이므로 '
    '고정 일수로 환산하지 않는다. 분류가 부분적인 동안 이 값은 하한이며 과대 '
    '산출되지 않는다. 이 함수는 의무 이행 여부를 주장하지 않는다. '
    'HR/payroll 개인정보 스키마의 내부 안전성 검증용이며 제품 API가 아니다.';

-- --------------------------------------------------------------------------
-- Grants. THE REVOKE IS THE LOAD-BEARING HALF.
--
-- Both functions are SECURITY DEFINER, and PostgreSQL grants EXECUTE on a new
-- function to PUBLIC by default. Without the REVOKE below, every role in the
-- cluster could run them. They are internal schema-safety introspection for the
-- in-scope HR/payroll substrate, not a compliance product surface, so the
-- runtime role receives no grant.
--
-- Proved by `personal_data_classification.rs`, which drives a freshly created
-- role and `console_rt` into `42501 insufficient_privilege`; owner-privileged
-- derivation tests elsewhere in that file still exercise the functions.
-- --------------------------------------------------------------------------
REVOKE ALL ON FUNCTION personal_data_columns() FROM PUBLIC;
REVOKE ALL ON FUNCTION access_log_retention_floor_years() FROM PUBLIC;
