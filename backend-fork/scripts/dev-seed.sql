-- Local dev business-data seed for the KNL tenant (org …a1).
--
-- WHY THIS EXISTS: `dev-up.mjs` migrates the schema but seeds no business data,
-- so every console screen rendered 0-counts/empty for a fresh dev-auth session
-- (only the one stray support ticket a prior manual session left behind ever
-- surfaced). This file populates ONE realistic org-scoped story — KNL Logistics,
-- a 창원-based forklift fleet-maintenance operator — across every domain the
-- console's mounted screens read, so each screen shows REAL rows.
--
-- IDIOM: connected as the PG superuser
-- (BYPASSRLS), arm `app.current_org` to mirror runtime tenant-scoping and to
-- satisfy any FORCE-RLS WITH CHECK, deterministic all-hex ids, ON CONFLICT DO
-- NOTHING so re-runs are idempotent. NOT production ops (that goes through the
-- audited console API, per project policy) — this is local dev fixture tooling
-- only. The KNL tenant org row itself is migration-seeded (0028); everything
-- below is the tenant's operating data.

BEGIN;

SELECT set_config('app.current_org', '00000000-0000-0000-0000-0000000000a1', true);

-- ── org structure: one region, two branches ────────────────────────────────
INSERT INTO regions (id, name, org_id) VALUES
  ('00000000-0000-0000-0000-0000000000b1', '경남권역', '00000000-0000-0000-0000-0000000000a1')
ON CONFLICT (id) DO NOTHING;

INSERT INTO branches (id, region_id, name, org_id) VALUES
  ('00000000-0000-0000-0000-0000000000c1', '00000000-0000-0000-0000-0000000000b1', '창원 본사', '00000000-0000-0000-0000-0000000000a1'),
  ('00000000-0000-0000-0000-0000000000c2', '00000000-0000-0000-0000-0000000000b1', '부산 지점', '00000000-0000-0000-0000-0000000000a1')
ON CONFLICT (id) DO NOTHING;

-- ── users ───────────────────────────────────────────────────────────────────
-- d001 is the dev-auth SUPER_ADMIN principal. dev-auth upserts on
-- phone='dev-auth:{org}:{role}' (DevPrincipalProvisioner), so pre-seeding the
-- row with THIS id + phone makes the later `POST /dev-auth/session` mint
-- ON CONFLICT (phone) DO UPDATE the SAME row — the session's user_id stays d001,
-- so the "me"-scoped rows below (notifications, assigned tickets) surface. The
-- dev-auth mint now defaults an unnamed persona to a production-shaped person
-- (전성진 for SUPER_ADMIN, see auth-rest `dev_persona_display_name`), and the
-- upsert leaves `team` untouched, so the identity chip reads 전성진 · 관리 —
-- never a raw `dev:SUPER_ADMIN` debug label.
INSERT INTO users (id, display_name, phone, roles, team, is_active, org_id, is_org_lead) VALUES
  ('00000000-0000-0000-0000-00000000d001', '전성진', 'dev-auth:00000000-0000-0000-0000-0000000000a1:SUPER_ADMIN', ARRAY['SUPER_ADMIN'], '관리', true, '00000000-0000-0000-0000-0000000000a1', true),
  ('00000000-0000-0000-0000-00000000d002', '김정비', '01041110001', ARRAY['MECHANIC'], '정비', true, '00000000-0000-0000-0000-0000000000a1', false),
  ('00000000-0000-0000-0000-00000000d003', '박접수', '01041110002', ARRAY['RECEPTIONIST'], '접수', true, '00000000-0000-0000-0000-0000000000a1', false),
  ('00000000-0000-0000-0000-00000000d004', '이대표', '01041110003', ARRAY['EXECUTIVE'], '관리', true, '00000000-0000-0000-0000-0000000000a1', false),
  -- r11: four more human staff so evidence authors / support requesters read as
  -- real people, never the dev SUPER_ADMIN principal. Names mirror the roster
  -- adds below (한지민/오세훈/강도영/윤서준) for a coherent org.
  ('00000000-0000-0000-0000-00000000d005', '한지민', '01041110005', ARRAY['RECEPTIONIST'], '접수', true, '00000000-0000-0000-0000-0000000000a1', false),
  ('00000000-0000-0000-0000-00000000d006', '오세훈', '01041110006', ARRAY['MECHANIC'], '정비', true, '00000000-0000-0000-0000-0000000000a1', false),
  ('00000000-0000-0000-0000-00000000d007', '강도영', '01041110007', ARRAY['RECEPTIONIST'], '접수', true, '00000000-0000-0000-0000-0000000000a1', false),
  ('00000000-0000-0000-0000-00000000d008', '윤서준', '01041110008', ARRAY['ADMIN'], '관리', true, '00000000-0000-0000-0000-0000000000a1', false)
ON CONFLICT (id) DO NOTHING;

INSERT INTO user_branches (user_id, branch_id, org_id)
SELECT u, '00000000-0000-0000-0000-0000000000c1', '00000000-0000-0000-0000-0000000000a1'
FROM unnest(ARRAY[
  '00000000-0000-0000-0000-00000000d001'::uuid,
  '00000000-0000-0000-0000-00000000d002'::uuid,
  '00000000-0000-0000-0000-00000000d003'::uuid,
  '00000000-0000-0000-0000-00000000d004'::uuid,
  '00000000-0000-0000-0000-00000000d005'::uuid,
  '00000000-0000-0000-0000-00000000d006'::uuid,
  '00000000-0000-0000-0000-00000000d007'::uuid,
  '00000000-0000-0000-0000-00000000d008'::uuid]) u
ON CONFLICT (user_id, branch_id) DO NOTHING;

-- ── employees (roster → leave + policy 인원 count) ───────────────────────────
-- The conditional role switch keeps this file valid both before 0166 (during
-- bootstrap/recovery) and after its leave-owned employee guard is installed.
SELECT set_config(
  'role',
  CASE WHEN to_regprocedure('leave_api.employee_leave_writer_guard()') IS NULL
    THEN current_user ELSE 'console_leave_definer' END,
  true
);
INSERT INTO employees (
  id, org_id, company, name, source_filename, source_sheet, source_row, source_key,
  raw_row, source_metadata, employment_status, employee_number, org_unit, job, position,
  worksite_name, hire_date, leave_accrued, leave_used, leave_remaining,
  identity_resolution_strategy, identity_resolution_confidence, identity_review_required, identity_name_only_merge
) VALUES
  ('00000000-0000-0000-0000-000000ee0001', '00000000-0000-0000-0000-0000000000a1', 'KNL로지스틱스', '김정비', 'roster_2026.xlsx', '정규직', 2, 'emp-1001', '{}'::jsonb, '{}'::jsonb, 'ACTIVE', '1001', '정비팀', '지게차 정비', '주임', '창원 본사', '2019-03-04', 15, 4, 11, 'employee_number', 'high', false, false),
  ('00000000-0000-0000-0000-000000ee0002', '00000000-0000-0000-0000-0000000000a1', 'KNL로지스틱스', '박접수', 'roster_2026.xlsx', '정규직', 3, 'emp-1002', '{}'::jsonb, '{}'::jsonb, 'ACTIVE', '1002', '접수팀', '고객 접수', '사원', '창원 본사', '2021-07-12', 15, 9, 6, 'employee_number', 'high', false, false),
  ('00000000-0000-0000-0000-000000ee0003', '00000000-0000-0000-0000-0000000000a1', 'KNL로지스틱스', '최현장', 'roster_2026.xlsx', '정규직', 4, 'emp-1003', '{}'::jsonb, '{}'::jsonb, 'ACTIVE', '1003', '정비팀', '현장 정비', '반장', '부산 지점', '2016-01-20', 20, 2, 18, 'employee_number', 'high', false, false),
  ('00000000-0000-0000-0000-000000ee0004', '00000000-0000-0000-0000-0000000000a1', 'KNL로지스틱스', '정관리', 'roster_2026.xlsx', '정규직', 5, 'emp-1004', '{}'::jsonb, '{}'::jsonb, 'ACTIVE', '1004', '관리팀', '운영 관리', '과장', '창원 본사', '2014-09-01', 22, 15, 7, 'employee_number', 'high', false, false),
  -- roster depth (r11): 4 people read thin against the reference's dense 직원별
  -- 연차 현황 list. Add eight more across both branches / teams with varied
  -- 부여·사용·잔여 so the 소진율 meters and 촉진 대상 flags spread realistically.
  ('00000000-0000-0000-0000-000000ee0005', '00000000-0000-0000-0000-0000000000a1', 'KNL로지스틱스', '한지민', 'roster_2026.xlsx', '정규직', 6, 'emp-1005', '{}'::jsonb, '{}'::jsonb, 'ACTIVE', '1005', '영업팀', '고객 영업', '대리', '창원 본사', '2020-05-11', 17, 3, 14, 'employee_number', 'high', false, false),
  ('00000000-0000-0000-0000-000000ee0006', '00000000-0000-0000-0000-0000000000a1', 'KNL로지스틱스', '오세훈', 'roster_2026.xlsx', '정규직', 7, 'emp-1006', '{}'::jsonb, '{}'::jsonb, 'ACTIVE', '1006', '정비팀', '지게차 정비', '사원', '부산 지점', '2022-02-14', 15, 1, 14, 'employee_number', 'high', false, false),
  ('00000000-0000-0000-0000-000000ee0007', '00000000-0000-0000-0000-0000000000a1', 'KNL로지스틱스', '강도영', 'roster_2026.xlsx', '정규직', 8, 'emp-1007', '{}'::jsonb, '{}'::jsonb, 'ACTIVE', '1007', '접수팀', '고객 접수', '주임', '부산 지점', '2018-11-03', 19, 12, 7, 'employee_number', 'high', false, false),
  ('00000000-0000-0000-0000-000000ee0008', '00000000-0000-0000-0000-0000000000a1', 'KNL로지스틱스', '윤서준', 'roster_2026.xlsx', '정규직', 9, 'emp-1008', '{}'::jsonb, '{}'::jsonb, 'ACTIVE', '1008', '관리팀', '경영 지원', '대리', '창원 본사', '2019-08-19', 18, 8, 10, 'employee_number', 'high', false, false),
  ('00000000-0000-0000-0000-000000ee0009', '00000000-0000-0000-0000-0000000000a1', 'KNL로지스틱스', '임하늘', 'roster_2026.xlsx', '정규직', 10, 'emp-1009', '{}'::jsonb, '{}'::jsonb, 'ACTIVE', '1009', '정비팀', '현장 정비', '사원', '창원 본사', '2023-01-09', 15, 13, 2, 'employee_number', 'high', false, false),
  ('00000000-0000-0000-0000-000000ee0010', '00000000-0000-0000-0000-0000000000a1', 'KNL로지스틱스', '배정후', 'roster_2026.xlsx', '정규직', 11, 'emp-1010', '{}'::jsonb, '{}'::jsonb, 'ACTIVE', '1010', '영업팀', '고객 영업', '과장', '부산 지점', '2015-06-22', 21, 6, 15, 'employee_number', 'high', false, false),
  ('00000000-0000-0000-0000-000000ee0011', '00000000-0000-0000-0000-0000000000a1', 'KNL로지스틱스', '신라온', 'roster_2026.xlsx', '정규직', 12, 'emp-1011', '{}'::jsonb, '{}'::jsonb, 'ACTIVE', '1011', '정비팀', '지게차 정비', '주임', '창원 본사', '2017-04-17', 20, 16, 4, 'employee_number', 'high', false, false),
  ('00000000-0000-0000-0000-000000ee0012', '00000000-0000-0000-0000-0000000000a1', 'KNL로지스틱스', '문가온', 'roster_2026.xlsx', '정규직', 13, 'emp-1012', '{}'::jsonb, '{}'::jsonb, 'ACTIVE', '1012', '접수팀', '고객 접수', '사원', '부산 지점', '2021-10-05', 15, 5, 10, 'employee_number', 'high', false, false)
ON CONFLICT (id) DO NOTHING;
RESET ROLE;

-- ── leave: annual obligations + a couple of pending requests ─────────────────
INSERT INTO annual_leave_obligations (id, org_id, employee_id, leave_year, leave_accrued, leave_used, leave_remaining, status, statutory_basis, notification_plan) VALUES
  ('00000000-0000-0000-0000-000000a10001', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000ee0001', 2026, 15, 4, 11, 'NEEDS_HR_REVIEW', '{"law":"근로기준법 제60조"}'::jsonb, '{"stage":"none"}'::jsonb),
  ('00000000-0000-0000-0000-000000a10002', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000ee0002', 2026, 15, 9, 6, 'USAGE_PROMOTION_DRAFT_REQUIRED', '{"law":"근로기준법 제61조"}'::jsonb, '{"stage":"draft_required"}'::jsonb),
  ('00000000-0000-0000-0000-000000a10003', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000ee0003', 2026, 20, 2, 18, 'PROMOTION_SENT', '{"law":"근로기준법 제61조"}'::jsonb, '{"stage":"sent","sent_on":"2026-06-30"}'::jsonb)
ON CONFLICT (id) DO NOTHING;

-- 0166 makes leave-request writes command-owned. Keep this seed runnable both
-- before and after that migration: only assume the definer role once the
-- command guard exists, then restore the migration login immediately after the
-- fixture insert. The retained `days` column is the expand/rollback contract.
SELECT set_config(
  'role',
  CASE WHEN to_regprocedure('leave_api.protected_request_writer_guard()') IS NULL
    THEN current_user ELSE 'console_leave_definer' END,
  true
);
INSERT INTO leave_requests (id, org_id, branch_id, requester_user_id, subject_employee_id, leave_type, days, start_date, end_date, reason, status) VALUES
  ('00000000-0000-0000-0000-000000a20001', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-0000000000c1', '00000000-0000-0000-0000-00000000d002', '00000000-0000-0000-0000-000000ee0001', 'annual', 2, '2026-07-21', '2026-07-22', '개인 사유 연차 사용', 'pending'),
  ('00000000-0000-0000-0000-000000a20002', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-0000000000c2', '00000000-0000-0000-0000-00000000d003', '00000000-0000-0000-0000-000000ee0003', 'half_day', 0.5, '2026-07-18', '2026-07-18', '오전 반차 (병원 방문)', 'pending')
ON CONFLICT (id) DO NOTHING;
RESET ROLE;

-- ── registry chain (customer → site → equipment) backing work orders ─────────
INSERT INTO registry_customers (id, branch_id, name, org_id) VALUES
  ('00000000-0000-0000-0000-000000c00001', '00000000-0000-0000-0000-0000000000c1', '한성물류㈜', '00000000-0000-0000-0000-0000000000a1')
ON CONFLICT (id) DO NOTHING;

INSERT INTO registry_sites (id, branch_id, customer_id, name, org_id, address, province, city) VALUES
  ('00000000-0000-0000-0000-000000c10001', '00000000-0000-0000-0000-0000000000c1', '00000000-0000-0000-0000-000000c00001', '한성물류 창원센터', '00000000-0000-0000-0000-0000000000a1', '경남 창원시 성산구 창원대로 1234', '경상남도', '창원시')
ON CONFLICT (id) DO NOTHING;

INSERT INTO registry_equipment (id, branch_id, customer_id, site_id, equipment_no, manufacturer_code, kind_code, power_code, status, specification, ton_text, source_sheet, source_row, org_id) VALUES
  ('00000000-0000-0000-0000-000000c20001', '00000000-0000-0000-0000-0000000000c1', '00000000-0000-0000-0000-000000c00001', '00000000-0000-0000-0000-000000c10001', 'KNLFL-0001', 'DOOSAN', 'FORKLIFT', 'DIESEL', '임대', '디젤 지게차 2.5톤', '2.5톤', '장비대장', 2, '00000000-0000-0000-0000-0000000000a1'),
  ('00000000-0000-0000-0000-000000c20002', '00000000-0000-0000-0000-0000000000c1', '00000000-0000-0000-0000-000000c00001', '00000000-0000-0000-0000-000000c10001', 'KNLFL-0002', 'HYUNDAI', 'FORKLIFT', 'ELECTRIC', '임대', '전동 지게차 1.5톤', '1.5톤', '장비대장', 3, '00000000-0000-0000-0000-0000000000a1')
ON CONFLICT (id) DO NOTHING;

-- ── work orders (dashboard KPI + ops summary + evidence parent) ──────────────
-- r12: ad0001 carries explicit created_at/target_due_at so the KPI rollup's
-- completion-duration + due-compliance aggregates stop reading 미지정 — both
-- default to `now()`/NULL otherwise, and since approved_at (below, in
-- work_order_approval_steps) is deliberately backdated for KPI realism, a
-- DEFAULT now() created_at always lands AFTER approved_at, so the rollup's
-- `approved_at >= created_at` guard never fires (average_completion_seconds
-- stays None). Backdating created_at here — and giving it a real due date it
-- met — is the real write shape (a work order is opened before it's closed).
INSERT INTO work_orders (
  id, request_no, branch_id, equipment_id, customer_id, site_id, requested_by, status, priority,
  symptom, customer_request, result_type, kpi_excluded, evidence_verified, org_id, created_at, target_due_at
) VALUES
  ('00000000-0000-0000-0000-000000ad0001', '20260701-001', '00000000-0000-0000-0000-0000000000c1', '00000000-0000-0000-0000-000000c20001', '00000000-0000-0000-0000-000000c00001', '00000000-0000-0000-0000-000000c10001', '00000000-0000-0000-0000-00000000d003', 'FINAL_COMPLETED', 'P2', '유압 실린더 누유', '리프트 하강 시 기름 누출', 'COMPLETED', false, true, '00000000-0000-0000-0000-0000000000a1', now() - interval '4 days', now() - interval '18 hours'),
  ('00000000-0000-0000-0000-000000ad0002', '20260705-002', '00000000-0000-0000-0000-0000000000c1', '00000000-0000-0000-0000-000000c20002', '00000000-0000-0000-0000-000000c00001', '00000000-0000-0000-0000-000000c10001', '00000000-0000-0000-0000-00000000d003', 'IN_PROGRESS', 'P1', '배터리 충전 불가', '전동 지게차 시동 불량', 'UNKNOWN', false, false, '00000000-0000-0000-0000-0000000000a1', DEFAULT, NULL),
  ('00000000-0000-0000-0000-000000ad0003', '20260708-003', '00000000-0000-0000-0000-0000000000c1', '00000000-0000-0000-0000-000000c20001', '00000000-0000-0000-0000-000000c00001', '00000000-0000-0000-0000-000000c10001', '00000000-0000-0000-0000-00000000d003', 'ASSIGNED', 'P3', '경적 작동 불량', '경고음이 나지 않음', 'UNKNOWN', false, false, '00000000-0000-0000-0000-0000000000a1', DEFAULT, NULL)
ON CONFLICT (id) DO NOTHING;

-- ── evidence media (docs screen) + lifecycle rows ────────────────────────────
INSERT INTO evidence_media (
  id, work_order_id, stage, s3_key, content_type, size_bytes, checksum_sha256, uploaded_by,
  worm_replica_status, retry_count, next_retry_at, verified_at, upload_confirmed_at, confirmed_by,
  org_id, processing_status
) VALUES
  ('00000000-0000-0000-0000-000000ed0001', '00000000-0000-0000-0000-000000ad0002', 'AFTER', 'evidence/knl/wo2-after.jpg', 'image/jpeg', 482013, encode(sha256('wo2-after'), 'hex'), '00000000-0000-0000-0000-00000000d002', 'VERIFIED', 0, now(), now(), now(), '00000000-0000-0000-0000-00000000d002', '00000000-0000-0000-0000-0000000000a1', 'READY'),
  ('00000000-0000-0000-0000-000000ed0002', '00000000-0000-0000-0000-000000ad0002', 'BEFORE', 'evidence/knl/wo2-before.jpg', 'image/jpeg', 391244, encode(sha256('wo2-before'), 'hex'), '00000000-0000-0000-0000-00000000d002', 'PENDING', 0, now(), NULL, now(), '00000000-0000-0000-0000-00000000d002', '00000000-0000-0000-0000-0000000000a1', 'READY')
ON CONFLICT (id) DO NOTHING;

INSERT INTO object_lifecycles (id, org_id, object_type, object_id, current_state, legal_hold) VALUES
  ('00000000-0000-0000-0000-0000001c0001', '00000000-0000-0000-0000-0000000000a1', 'work_order', '00000000-0000-0000-0000-000000ad0001', 'final_completed', false),
  ('00000000-0000-0000-0000-0000001c0002', '00000000-0000-0000-0000-0000000000a1', 'work_order', '00000000-0000-0000-0000-000000ad0002', 'in_progress', false)
ON CONFLICT (id) DO NOTHING;

-- ── EV evidence OBJECTS (문서·기록물 → 증거 tab reads docs_evidence_objects, a
--     registration read-model SEPARATE from evidence_media; the domain's
--     create_object path writes: the EV code counter, the object row, and a
--     REGISTERED custody event atomically — mirrored exactly here, no shortcut
--     that diverges from the real write). Sourced from the two seeded
--     evidence_media rows above (source_type='work_order_evidence_media'). ────
INSERT INTO docs_evidence_code_counters (org_id, object_prefix, next_value) VALUES
  ('00000000-0000-0000-0000-0000000000a1', 'EV', 13)
ON CONFLICT (org_id, object_prefix) DO NOTHING;

INSERT INTO docs_evidence_objects (
  id, org_id, code, title, description, source_type, source_id, source_code,
  classification, record_owner_user_id, created_by, updated_by
) VALUES
  ('00000000-0000-0000-0000-000000ef0001', '00000000-0000-0000-0000-0000000000a1', 'EV-000001', '작업지시 20260705-002 완료 후 증빙 (After)', '전동 지게차 배터리 수리 완료 상태 사진', 'work_order_evidence_media', '00000000-0000-0000-0000-000000ed0001', '20260705-002', 'INTERNAL', '00000000-0000-0000-0000-00000000d002', '00000000-0000-0000-0000-00000000d002', '00000000-0000-0000-0000-00000000d002'),
  ('00000000-0000-0000-0000-000000ef0002', '00000000-0000-0000-0000-0000000000a1', 'EV-000002', '작업지시 20260705-002 착수 전 증빙 (Before)', '전동 지게차 배터리 수리 착수 전 상태 사진', 'work_order_evidence_media', '00000000-0000-0000-0000-000000ed0002', '20260705-002', 'SENSITIVE', '00000000-0000-0000-0000-00000000d002', '00000000-0000-0000-0000-00000000d002', '00000000-0000-0000-0000-00000000d002')
ON CONFLICT (id) DO NOTHING;

INSERT INTO docs_evidence_custody_events (
  id, org_id, evidence_object_id, stage, actor_user_id, reason, source_ref,
  event_digest_sha256, occurred_at
) VALUES
  ('00000000-0000-0000-0000-000000ef1001', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000ef0001', 'REGISTERED', '00000000-0000-0000-0000-00000000d002', '작업지시 증빙 등록', '{"source_type":"work_order_evidence_media","source_id":"00000000-0000-0000-0000-000000ed0001","source_code":"20260705-002"}'::jsonb, encode(sha256('ev-000001-registered'), 'hex'), now()),
  ('00000000-0000-0000-0000-000000ef1002', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000ef0002', 'REGISTERED', '00000000-0000-0000-0000-00000000d002', '작업지시 증빙 등록', '{"source_type":"work_order_evidence_media","source_id":"00000000-0000-0000-0000-000000ed0002","source_code":"20260705-002"}'::jsonb, encode(sha256('ev-000002-registered'), 'hex'), now())
ON CONFLICT (id) DO NOTHING;

-- Retention (보존) rows the 증거 screen resolves per-object via
-- GET /api/v1/lifecycles/evidence_object/{id}. One is inside the 90-day
-- expiry window so the "보존 만료 임박" stat surfaces a real count.
INSERT INTO object_lifecycles (id, org_id, object_type, object_id, current_state, legal_hold, retention_until) VALUES
  ('00000000-0000-0000-0000-0000001c0003', '00000000-0000-0000-0000-0000000000a1', 'evidence_object', '00000000-0000-0000-0000-000000ef0001', 'registered', false, (current_date + 60)),
  ('00000000-0000-0000-0000-0000001c0004', '00000000-0000-0000-0000-0000000000a1', 'evidence_object', '00000000-0000-0000-0000-000000ef0002', 'registered', false, (current_date + 1095))
ON CONFLICT (id) DO NOTHING;

-- ── evidence depth (r9): two evidence objects left the 증거 table nearly empty
-- against the reference's dense record list. Add ten more registered records
-- (varied source_type/classification/owner, registration dates spread over
-- recent months) so the table reads populated and the 총 기록물 / 이번달 등록
-- stats compute real counts. Each object carries its REGISTERED custody event,
-- exactly as the domain's create path writes it (counter → object → custody).
INSERT INTO docs_evidence_objects (
  id, org_id, code, title, description, source_type, source_id, source_code,
  classification, record_owner_user_id, created_by, updated_by, created_at
)
SELECT
  ('00000000-0000-0000-0000-000000ef0' || lpad(v.seq::text, 3, '0'))::uuid,
  '00000000-0000-0000-0000-0000000000a1',
  'EV-' || lpad(v.seq::text, 6, '0'),
  v.title, v.descr, v.stype, v.sid, v.scode, v.cls,
  v.owner::uuid, v.owner::uuid, v.owner::uuid,
  now() - (v.days_ago || ' days')::interval
FROM (VALUES
  (3,  '현장 CCTV 클립 — 지게차 유압 누유 정황', '창원센터 반입구 CCTV에서 확인된 누유 발생 시점 영상', 'external_document', 'ext-cctv-c207-0705', 'CAM-207', 'INTERNAL', '00000000-0000-0000-0000-00000000d002', 2),
  -- r11: record owner = the seeded human who owns the record, never the dev
  -- SUPER_ADMIN principal (d001). d001 now reads as a real person (전성진), but
  -- authorship still routes to the domain owners for a coherent org: governance
  -- docs → 이대표 (d004, executive); safety report → 김정비 (d002, 정비 주임).
  (4,  '무단결근 소명 진술 녹취', '근태 이의 제기 건 소명 청취 녹취 파일', 'inbox_doc', 'inbox-hr-0630', 'HR-0630', 'SENSITIVE', '00000000-0000-0000-0000-00000000d004', 5),
  (5,  '하도급 서면실태조사 대응 자료', '공정위 서면실태조사 회신 첨부 문서', 'mail_attachment', 'mail-fair-0628', 'FAIR-0628', 'CONFIDENTIAL', '00000000-0000-0000-0000-00000000d004', 9),
  (6,  '정기 점검 완료 사진 — KNLFL-0001', '2.5톤 디젤 지게차 정기점검 완료 상태 사진', 'work_order_evidence_media', 'wo-media-ad0005', '20260601-005', 'GENERAL', '00000000-0000-0000-0000-00000000d002', 14),
  (7,  '브레이크 패드 교체 전후 비교 사진', '제동 성능 저하 정비 증빙', 'work_order_evidence_media', 'wo-media-ad0007', '20260401-007', 'INTERNAL', '00000000-0000-0000-0000-00000000d002', 22),
  (8,  '임대 계약 갱신 합의서 스캔본', '한성물류 지게차 임대 계약 갱신 서명본', 'record_archive', 'arch-contract-c209', 'C-209', 'CONFIDENTIAL', '00000000-0000-0000-0000-00000000d003', 33),
  (9,  '유압 호스 교체 부품 수령 증빙', '작동유 누유 정비 부품 입고 확인 사진', 'work_order_evidence_media', 'wo-media-ad0009', '20260201-009', 'GENERAL', '00000000-0000-0000-0000-00000000d002', 45),
  (10, '고객 클레임 대응 이메일 스레드', '부산 창고 유압 누유 재발 클레임 대응 기록', 'mail_attachment', 'mail-claim-0210', 'SUP-5C0003', 'INTERNAL', '00000000-0000-0000-0000-00000000d003', 62),
  (11, '전조등 교체 작업 완료 확인서', '야간 작업등 불량 정비 완료 확인 서명', 'record_archive', 'arch-wo-ad0008', '20260301-008', 'GENERAL', '00000000-0000-0000-0000-00000000d002', 88),
  (12, '2026 상반기 정기 안전점검 보고서', '반기 안전점검 결과 종합 보고 문서', 'record_archive', 'arch-safety-2026h1', 'SAFE-2026H1', 'INTERNAL', '00000000-0000-0000-0000-00000000d002', 120)
) AS v(seq, title, descr, stype, sid, scode, cls, owner, days_ago)
ON CONFLICT (id) DO NOTHING;

INSERT INTO docs_evidence_custody_events (
  id, org_id, evidence_object_id, stage, actor_user_id, reason, source_ref,
  event_digest_sha256, occurred_at
)
SELECT
  ('00000000-0000-0000-0000-000000ef1' || lpad(v.seq::text, 3, '0'))::uuid,
  '00000000-0000-0000-0000-0000000000a1',
  ('00000000-0000-0000-0000-000000ef0' || lpad(v.seq::text, 3, '0'))::uuid,
  'REGISTERED', v.owner::uuid, '기록물 등재',
  jsonb_build_object('source_type', v.stype, 'source_id', v.sid, 'source_code', v.scode),
  encode(sha256(('ev-' || lpad(v.seq::text, 6, '0') || '-registered')::bytea), 'hex'),
  now() - (v.days_ago || ' days')::interval
FROM (VALUES
  (3,  'external_document', 'ext-cctv-c207-0705', 'CAM-207', '00000000-0000-0000-0000-00000000d002', 2),
  (4,  'inbox_doc', 'inbox-hr-0630', 'HR-0630', '00000000-0000-0000-0000-00000000d004', 5),
  (5,  'mail_attachment', 'mail-fair-0628', 'FAIR-0628', '00000000-0000-0000-0000-00000000d004', 9),
  (6,  'work_order_evidence_media', 'wo-media-ad0005', '20260601-005', '00000000-0000-0000-0000-00000000d002', 14),
  (7,  'work_order_evidence_media', 'wo-media-ad0007', '20260401-007', '00000000-0000-0000-0000-00000000d002', 22),
  (8,  'record_archive', 'arch-contract-c209', 'C-209', '00000000-0000-0000-0000-00000000d003', 33),
  (9,  'work_order_evidence_media', 'wo-media-ad0009', '20260201-009', '00000000-0000-0000-0000-00000000d002', 45),
  (10, 'mail_attachment', 'mail-claim-0210', 'SUP-5C0003', '00000000-0000-0000-0000-00000000d003', 62),
  (11, 'record_archive', 'arch-wo-ad0008', '20260301-008', '00000000-0000-0000-0000-00000000d002', 88),
  (12, 'record_archive', 'arch-safety-2026h1', 'SAFE-2026H1', '00000000-0000-0000-0000-00000000d002', 120)
) AS v(seq, stype, sid, scode, owner, days_ago)
ON CONFLICT (id) DO NOTHING;

-- Retention for a few of the new records; EV-000004 sits inside the 90-day
-- window so the 보존 만료 임박 stat reflects more than one object.
INSERT INTO object_lifecycles (id, org_id, object_type, object_id, current_state, legal_hold, retention_until) VALUES
  ('00000000-0000-0000-0000-0000001c0005', '00000000-0000-0000-0000-0000000000a1', 'evidence_object', '00000000-0000-0000-0000-000000ef0004', 'registered', false, (current_date + 45)),
  ('00000000-0000-0000-0000-0000001c0006', '00000000-0000-0000-0000-0000000000a1', 'evidence_object', '00000000-0000-0000-0000-000000ef0008', 'registered', false, (current_date + 1825)),
  ('00000000-0000-0000-0000-0000001c0007', '00000000-0000-0000-0000-0000000000a1', 'evidence_object', '00000000-0000-0000-0000-000000ef0012', 'registered', false, (current_date + 730))
ON CONFLICT (id) DO NOTHING;

-- r11: every remaining EV object gets a real retention_until so the 보존 column
-- reads populated across the whole table (verdict: no "—" retention gaps against
-- the reference's fully-populated 보존 list). Durations spread 3·5 years by
-- classification. Each resolves through the same
-- GET /api/v1/lifecycles/evidence_object/{id} read as the rows above.
INSERT INTO object_lifecycles (id, org_id, object_type, object_id, current_state, legal_hold, retention_until) VALUES
  ('00000000-0000-0000-0000-0000001c0008', '00000000-0000-0000-0000-0000000000a1', 'evidence_object', '00000000-0000-0000-0000-000000ef0003', 'registered', false, (current_date + 1825)),
  ('00000000-0000-0000-0000-0000001c0009', '00000000-0000-0000-0000-0000000000a1', 'evidence_object', '00000000-0000-0000-0000-000000ef0005', 'registered', false, (current_date + 1825)),
  ('00000000-0000-0000-0000-0000001c000a', '00000000-0000-0000-0000-0000000000a1', 'evidence_object', '00000000-0000-0000-0000-000000ef0006', 'registered', false, (current_date + 1095)),
  ('00000000-0000-0000-0000-0000001c000b', '00000000-0000-0000-0000-0000000000a1', 'evidence_object', '00000000-0000-0000-0000-000000ef0007', 'registered', false, (current_date + 1095)),
  ('00000000-0000-0000-0000-0000001c000c', '00000000-0000-0000-0000-0000000000a1', 'evidence_object', '00000000-0000-0000-0000-000000ef0009', 'registered', false, (current_date + 1095)),
  ('00000000-0000-0000-0000-0000001c000d', '00000000-0000-0000-0000-0000000000a1', 'evidence_object', '00000000-0000-0000-0000-000000ef0010', 'registered', false, (current_date + 1825)),
  ('00000000-0000-0000-0000-0000001c000e', '00000000-0000-0000-0000-0000000000a1', 'evidence_object', '00000000-0000-0000-0000-000000ef0011', 'registered', false, (current_date + 1095))
ON CONFLICT (id) DO NOTHING;

-- ── policy roles + permissions + version (거버넌스 / 정책) ───────────────────
INSERT INTO policy_roles (id, org_id, role_key, display_name, description, status, is_system, created_by) VALUES
  ('00000000-0000-0000-0000-000000b00001', '00000000-0000-0000-0000-0000000000a1', 'branch_manager', '지점장', '지점 단위 운영 및 승인 권한', 'ACTIVE', false, '00000000-0000-0000-0000-00000000d001'),
  ('00000000-0000-0000-0000-000000b00002', '00000000-0000-0000-0000-0000000000a1', 'field_mechanic', '현장 정비원', '작업지시 처리 및 증빙 업로드', 'ACTIVE', false, '00000000-0000-0000-0000-00000000d001')
ON CONFLICT (id) DO NOTHING;

INSERT INTO policy_role_permissions (id, org_id, role_id, feature_key, permission_level) VALUES
  ('00000000-0000-0000-0000-000000bb0001', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000b00001', 'evidence_attach', 'allow'),
  ('00000000-0000-0000-0000-000000bb0002', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000b00002', 'evidence_attach', 'allow')
ON CONFLICT (id) DO NOTHING;

INSERT INTO policy_versions (org_id, version, updated_at) VALUES
  ('00000000-0000-0000-0000-0000000000a1', 3, now())
ON CONFLICT (org_id) DO UPDATE SET version = EXCLUDED.version, updated_at = EXCLUDED.updated_at;

-- ── Cedar PBAC policy CATALOG (권한·정책 screen reads cedar_policy_catalog_entries
--     + cedar_policy_drafts — the PBAC read-model, NOT the legacy policy_roles/
--     policy_versions RBAC tables above; those two systems are disjoint). Rows
--     render 허용/금지 (effect) badges + 시행 중/초안 (status) chips. Enforced
--     rows carry the same triad a real promotion writes: policy_version +
--     schema_version + bundle_digest (the table's CHECK requires all three for
--     status IN ('enforced','shadow')). ────────────────────────────────────────
INSERT INTO cedar_policy_catalog_entries (
  id, org_id, stable_key, title, natural_language_rule, effect, status, source,
  principal, action, resource, normalized_row, validation_status,
  policy_version, schema_version, bundle_digest, created_by, updated_by
) VALUES
  ('00000000-0000-0000-0000-000000ca0001', '00000000-0000-0000-0000-0000000000a1', 'attendance.branch_lead_read', '지점장은 소속 지점 팀원의 근태를 열람할 수 있다', '지점장 역할은 자신이 속한 지점의 팀원 근태 기록을 열람할 수 있습니다.', 'permit', 'enforced', 'imported_fixture', '{"role":"branch_manager"}'::jsonb, '{"action_key":"attendance.read"}'::jsonb, '{"resource_type":"attendance","scope":"branch"}'::jsonb, '{"effect":"permit","action":"view","resource_type":"attendance","conditions":[{"attr":"branch","op":"eq","value":{"kind":"subject_attr","value":"branch"}}]}'::jsonb, 'valid', 1, '1', 'sha256:' || encode(sha256('cedar-bundle-attendance-read'), 'hex'), '00000000-0000-0000-0000-00000000d001', '00000000-0000-0000-0000-00000000d001'),
  ('00000000-0000-0000-0000-000000ca0002', '00000000-0000-0000-0000-0000000000a1', 'payroll.cross_org_deny', '타 법인 직원의 급여 상세는 볼 수 없다', '어떤 역할도 자신이 속하지 않은 법인 소속 직원의 급여 상세를 열람할 수 없습니다.', 'forbid', 'enforced', 'imported_fixture', '{"role":"any"}'::jsonb, '{"action_key":"payroll.detail.read"}'::jsonb, '{"resource_type":"payroll","scope":"cross_org"}'::jsonb, '{"effect":"forbid","action":"view","resource_type":"payroll_detail","conditions":[{"attr":"owner","op":"ne","value":{"kind":"subject_attr","value":"org"}}]}'::jsonb, 'valid', 1, '1', 'sha256:' || encode(sha256('cedar-bundle-payroll-cross-org'), 'hex'), '00000000-0000-0000-0000-00000000d001', '00000000-0000-0000-0000-00000000d001'),
  ('00000000-0000-0000-0000-000000ca0003', '00000000-0000-0000-0000-0000000000a1', 'payroll.self_read', '본인 급여 명세는 스스로 열람할 수 있다', '모든 직원은 자신의 급여 명세를 스스로 열람할 수 있습니다.', 'permit', 'enforced', 'imported_fixture', '{"role":"employee"}'::jsonb, '{"action_key":"payroll.self.read"}'::jsonb, '{"resource_type":"payroll","scope":"self"}'::jsonb, '{"effect":"permit","action":"view","resource_type":"payroll_detail","conditions":[{"attr":"owner","op":"eq","value":{"kind":"subject_attr","value":"user_id"}}]}'::jsonb, 'valid', 1, '1', 'sha256:' || encode(sha256('cedar-bundle-payroll-self'), 'hex'), '00000000-0000-0000-0000-00000000d001', '00000000-0000-0000-0000-00000000d001'),
  ('00000000-0000-0000-0000-000000ca0004', '00000000-0000-0000-0000-0000000000a1', 'hr.sensitive_read', '인사 책임자는 상세 정보(민감)를 열람할 수 있다', '인사 책임자 역할은 민감 인사 정보를 감사 기록을 남기며 열람할 수 있습니다.', 'permit', 'enforced', 'imported_fixture', '{"role":"hr_lead"}'::jsonb, '{"action_key":"hr.sensitive.read"}'::jsonb, '{"resource_type":"employee","field_class":"sensitive"}'::jsonb, '{"effect":"permit","action":"read_field","resource_type":"employee","conditions":[{"attr":"roles","op":"contains","value":{"kind":"literal","value":"hr_lead"}}]}'::jsonb, 'valid', 1, '1', 'sha256:' || encode(sha256('cedar-bundle-hr-sensitive'), 'hex'), '00000000-0000-0000-0000-00000000d001', '00000000-0000-0000-0000-00000000d001'),
  ('00000000-0000-0000-0000-000000ca0005', '00000000-0000-0000-0000-0000000000a1', 'workforce.terminated_exclude', '휴직·기간제 종료 인원은 재직 집계·근무 편성에서 제외한다', '휴직 또는 기간제 종료 상태의 인원은 재직 집계와 근무 편성 대상에서 제외됩니다.', 'forbid', 'draft', 'no_code_draft', '{"employment_status":["on_leave","terminated"]}'::jsonb, '{"action_key":"workforce.roster.include"}'::jsonb, '{"resource_type":"workforce_roster"}'::jsonb, NULL, 'valid', NULL, NULL, NULL, '00000000-0000-0000-0000-00000000d001', '00000000-0000-0000-0000-00000000d001'),
  ('00000000-0000-0000-0000-000000ca0006', '00000000-0000-0000-0000-0000000000a1', 'evidence.mechanic_upload', '현장 정비원은 작업지시 증빙을 업로드할 수 있다', '현장 정비원 역할은 배정된 작업지시에 증빙 자료를 업로드할 수 있습니다.', 'permit', 'enforced', 'imported_fixture', '{"role":"field_mechanic"}'::jsonb, '{"action_key":"evidence.attach"}'::jsonb, '{"resource_type":"work_order","scope":"assigned"}'::jsonb, '{"effect":"permit","action":"edit","resource_type":"work_order","conditions":[{"attr":"roles","op":"contains","value":{"kind":"literal","value":"field_mechanic"}}]}'::jsonb, 'valid', 1, '1', 'sha256:' || encode(sha256('cedar-bundle-evidence-upload'), 'hex'), '00000000-0000-0000-0000-00000000d001', '00000000-0000-0000-0000-00000000d001'),
  ('00000000-0000-0000-0000-000000ca0007', '00000000-0000-0000-0000-0000000000a1', 'dispatch.coordinator_scope', '파견 코디네이터는 배정 현장 인원만 조회한다', '파견 코디네이터 역할은 자신이 배정된 현장의 인원만 조회할 수 있습니다.', 'permit', 'draft', 'no_code_draft', '{"role":"dispatch_coordinator"}'::jsonb, '{"action_key":"workforce.read"}'::jsonb, '{"resource_type":"workforce_roster","scope":"assigned_site"}'::jsonb, NULL, 'valid', NULL, NULL, NULL, '00000000-0000-0000-0000-00000000d001', '00000000-0000-0000-0000-00000000d001')
ON CONFLICT (id) DO NOTHING;

-- ── policy catalog depth (r14): the 권한·정책 list held 6 enforced + 1 draft, so
-- the panel's lower half read empty against the reference's ~9 활성 rows. Seed
-- three more enforced rules (each with the real promotion triad the CHECK
-- requires) so the list fills to 활성 9 · 초안 1 and reads as a governed policy
-- set, not a stub. Distinct stable_keys, so re-runs stay idempotent.
INSERT INTO cedar_policy_catalog_entries (
  id, org_id, stable_key, title, natural_language_rule, effect, status, source,
  principal, action, resource, normalized_row, validation_status,
  policy_version, schema_version, bundle_digest, created_by, updated_by
) VALUES
  ('00000000-0000-0000-0000-000000ca0008', '00000000-0000-0000-0000-0000000000a1', 'audit.post_settle_reject', '감사팀은 최종승인·종결 건도 사후 반려할 수 있다', '감사팀 역할은 이미 최종승인되거나 종결된 건에 대해서도 사후 반려를 수행할 수 있습니다.', 'permit', 'enforced', 'imported_fixture', '{"role":"audit_lead"}'::jsonb, '{"action_key":"approval.post_settle.reject"}'::jsonb, '{"resource_type":"approval","scope":"org"}'::jsonb, '{"effect":"permit","action":"edit","resource_type":"approval","conditions":[{"attr":"roles","op":"contains","value":{"kind":"literal","value":"audit_lead"}}]}'::jsonb, 'valid', 1, '1', 'sha256:' || encode(sha256('cedar-bundle-audit-post-settle'), 'hex'), '00000000-0000-0000-0000-00000000d001', '00000000-0000-0000-0000-00000000d001'),
  ('00000000-0000-0000-0000-000000ca0009', '00000000-0000-0000-0000-0000000000a1', 'workforce.pool_hr_scope', '인력풀(비상근) 인사정보는 인사·파견운영·배정 현장 반장만 열람한다', '비상근 인력풀의 인사정보는 인사 책임자, 파견운영 담당, 배정 현장 반장 역할에 한해 열람할 수 있습니다.', 'permit', 'draft', 'no_code_draft', '{"role":["hr_lead","dispatch_ops","site_foreman"]}'::jsonb, '{"action_key":"workforce.pool.read"}'::jsonb, '{"resource_type":"workforce_roster","scope":"pool"}'::jsonb, NULL, 'valid', NULL, NULL, NULL, '00000000-0000-0000-0000-00000000d001', '00000000-0000-0000-0000-00000000d001'),
  ('00000000-0000-0000-0000-000000ca000a', '00000000-0000-0000-0000-0000000000a1', 'recruit.applicant_notice_hidden', '외부 인원(지원자)은 내부 공모 공고를 열람할 수 없다 — 존재 자체 비노출', '외부 지원자 신분의 인원은 내부 공모 공고를 열람할 수 없으며 공고의 존재 자체가 노출되지 않습니다.', 'forbid', 'enforced', 'imported_fixture', '{"role":"external_applicant"}'::jsonb, '{"action_key":"recruit.internal_notice.read"}'::jsonb, '{"resource_type":"recruit_notice","scope":"internal"}'::jsonb, '{"effect":"forbid","action":"view","resource_type":"recruit_notice","conditions":[{"attr":"roles","op":"contains","value":{"kind":"literal","value":"external_applicant"}}]}'::jsonb, 'valid', 1, '1', 'sha256:' || encode(sha256('cedar-bundle-recruit-notice-hidden'), 'hex'), '00000000-0000-0000-0000-00000000d001', '00000000-0000-0000-0000-00000000d001')
ON CONFLICT (id) DO NOTHING;

-- ── workflow studio definitions (자동화 / 워크플로) ──────────────────────────
-- f00003 is an Automate rule (hub envelope): a typed predicate condition +
-- object-monitor trigger over the seeded 정비 계약 (maintenance_contract) type,
-- so the 자동화 허브 renders a CONFIGURED flow (trigger body = 정비 계약, condition
-- chips = 상태/인건비율) instead of a skeletal one. updated_at is pinned latest so
-- it is the default-selected rule (list orders updated_at DESC).
INSERT INTO workflow_definitions (id, org_id, workflow_key, display_name, object_type, status, latest_version, active_version, created_by, updated_at) VALUES
  ('00000000-0000-0000-0000-000000f00003', '00000000-0000-0000-0000-0000000000a1', 'automate.contract_renewal_watch', '고인건비 계약 갱신 감시', 'maintenance_contract', 'ACTIVE', 1, 1, '00000000-0000-0000-0000-00000000d001', now() + interval '1 minute')
ON CONFLICT (id) DO NOTHING;
INSERT INTO workflow_definitions (id, org_id, workflow_key, display_name, object_type, status, latest_version, active_version, created_by) VALUES
  ('00000000-0000-0000-0000-000000f00001', '00000000-0000-0000-0000-0000000000a1', 'work_order.approval', '작업지시 승인 흐름', 'work_order', 'ACTIVE', 1, 1, '00000000-0000-0000-0000-00000000d001'),
  ('00000000-0000-0000-0000-000000f00002', '00000000-0000-0000-0000-0000000000a1', 'leave.promotion', '연차 촉진 통보', 'leave_obligation', 'DRAFT', 1, NULL, '00000000-0000-0000-0000-00000000d001')
ON CONFLICT (id) DO NOTHING;

INSERT INTO workflow_definition_versions (
  id, org_id, definition_id, version, status, definition, approval_line, payment_line,
  notification_rules, action_allowlist, required_approval_line, required_payment_line, created_by
) VALUES
  ('00000000-0000-0000-0000-000000fa0003', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000f00003', 1, 'PUBLISHED', '{"schema_version":"workflow.definition.v1","trigger":"maintenance_contract.monitor","steps":[],"automate":{"scope":"org","doc":null,"condition":{"join":"and","predicates":[{"id":"p-mc-status","field":"status","op":"eq","value":{"kind":"enum","value":"active"}},{"id":"p-mc-labor","field":"labor_cost_ratio_pct","op":"gte","value":{"kind":"number","value":40}}]},"monitor":{"object_type":"maintenance_contract","action_key":"renew"}}}'::jsonb, '[]'::jsonb, '[]'::jsonb, '[]'::jsonb, '["maintenance_contract.renew"]'::jsonb, false, false, '00000000-0000-0000-0000-00000000d001'),
  ('00000000-0000-0000-0000-000000fa0001', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000f00001', 1, 'PUBLISHED', '{"nodes":[{"id":"start","type":"start"},{"id":"approve","type":"approval"},{"id":"end","type":"end"}]}'::jsonb, '[{"step":1,"role":"branch_manager"}]'::jsonb, '[]'::jsonb, '[]'::jsonb, '["work_order.complete"]'::jsonb, true, false, '00000000-0000-0000-0000-00000000d001'),
  ('00000000-0000-0000-0000-000000fa0002', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000f00002', 1, 'DRAFT', '{"nodes":[{"id":"start","type":"start"},{"id":"notify","type":"notification"},{"id":"end","type":"end"}]}'::jsonb, '[]'::jsonb, '[]'::jsonb, '[{"channel":"in_app"}]'::jsonb, '[]'::jsonb, false, false, '00000000-0000-0000-0000-00000000d001')
ON CONFLICT (id) DO NOTHING;

-- ── ontology: one instance-backed object type + property defs + instances ────
-- Migration 0165 retains one audited compatibility path for the exact old
-- runtime DML shape. Enter that constrained identity only for protected
-- definitions; its trigger owns immutable key reservations. The surrounding
-- local-only fixture remains cluster-admin seeded as documented above.
SET LOCAL ROLE console_rt;

INSERT INTO ont_object_types (id, org_id, stable_key, title, title_property_key, backing_kind, schema_version, lifecycle_state, created_by) VALUES
  ('00000000-0000-0000-0000-000000a70001', '00000000-0000-0000-0000-0000000000a1', 'maintenance_contract', '정비 계약', 'name', 'instance', 1, 'published', '00000000-0000-0000-0000-00000000d001')
ON CONFLICT (id) DO NOTHING;

INSERT INTO ont_property_defs (id, org_id, object_type_id, key, title, type, config, required, in_property_policy) VALUES
  ('00000000-0000-0000-0000-000000a80001', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000a70001', 'name', '계약명', 'string', '{}'::jsonb, true, false),
  ('00000000-0000-0000-0000-000000a80002', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000a70001', 'annual_fee_won', '연간 계약금(원)', 'number', '{}'::jsonb, false, false)
ON CONFLICT (id) DO NOTHING;

RESET ROLE;

-- instances + genesis revision (hash chain: prev = 64 zeros, row = sha256 of attrs)
-- Explicit created_at on the contract instances (not just DEFAULT now()):
-- useOntologyWorkspace seeds the graph pane from `entries[0].instances[0]`
-- (types ordered by stable_key ASC, instances DESC by created_at) — within a
-- single seed transaction every DEFAULT now() ties, so root selection would be
-- undefined. Pinning a90001 strictly latest makes it the deterministic root.
INSERT INTO ont_instances (id, org_id, object_type_id, title, current_revision_id, lifecycle_state, created_at) VALUES
  ('00000000-0000-0000-0000-000000a90001', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000a70001', '한성물류 연간 정비 계약', '00000000-0000-0000-0000-000000ab0001', 'active', now()),
  ('00000000-0000-0000-0000-000000a90002', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000a70001', '부산 지점 정기 점검 계약', '00000000-0000-0000-0000-000000ab0002', 'active', now() - interval '2 days')
ON CONFLICT (id) DO NOTHING;

-- r12: attributes carry the 5 deepened properties below (vendor_name/
-- contract_start/contract_end/status/labor_cost_ratio_pct) — ont_instance_revisions
-- is append-only (no UPDATE), so the richer payload has to be in this genesis
-- row, not patched on afterward.
INSERT INTO ont_instance_revisions (id, org_id, instance_id, version, attributes, valid_from, prev_hash, row_hash) VALUES
  ('00000000-0000-0000-0000-000000ab0001', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000a90001', 1, '{"name":"한성물류 연간 정비 계약","annual_fee_won":36000000,"vendor_name":"한성물류㈜","contract_start":"2026-01-01","contract_end":"2026-12-31","status":"active","labor_cost_ratio_pct":38}'::jsonb, now(), repeat('0', 64), encode(sha256('mc-001-v1'), 'hex')),
  ('00000000-0000-0000-0000-000000ab0002', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000a90002', 1, '{"name":"부산 지점 정기 점검 계약","annual_fee_won":18000000,"vendor_name":"부산해운대물류㈜","contract_start":"2026-03-01","contract_end":"2027-02-28","status":"active","labor_cost_ratio_pct":42}'::jsonb, now(), repeat('0', 64), encode(sha256('mc-002-v1'), 'hex'))
ON CONFLICT (id) DO NOTHING;

-- r12: deepen the 매니저 tab's 속성 (5 more, real FIELD_KINDS tags) + populate
-- 액션/분석 (both tables were empty across every seeded object type, so those
-- two 매니저 subtabs always rendered EmptyChip — not a UI gap, a seed gap).
SET LOCAL ROLE console_rt;

INSERT INTO ont_property_defs (id, org_id, object_type_id, key, title, type, config, required, in_property_policy) VALUES
  ('00000000-0000-0000-0000-000000a80003', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000a70001', 'vendor_name', '거래처명', 'text', '{}'::jsonb, true, false),
  ('00000000-0000-0000-0000-000000a80004', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000a70001', 'contract_start', '계약 시작일', 'date', '{}'::jsonb, true, false),
  ('00000000-0000-0000-0000-000000a80005', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000a70001', 'contract_end', '계약 종료일', 'date', '{}'::jsonb, true, false),
  ('00000000-0000-0000-0000-000000a80006', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000a70001', 'status', '상태', 'choice', '{"choices":[{"id":"active","name":"활성"},{"id":"expired","name":"만료"},{"id":"terminated","name":"해지"}]}'::jsonb, true, false),
  ('00000000-0000-0000-0000-000000a80007', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000a70001', 'labor_cost_ratio_pct', '인건비율(%)', 'number', '{}'::jsonb, false, true)
ON CONFLICT (id) DO NOTHING;

INSERT INTO ont_action_types (id, org_id, object_type_id, stable_key, title, dispatch, control_points) VALUES
  ('00000000-0000-0000-0000-000000a91101', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000a70001', 'renew', '계약 갱신', 'instance_revision', '[]'::jsonb),
  ('00000000-0000-0000-0000-000000a91102', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000a70001', 'terminate', '계약 해지', 'instance_revision', '[]'::jsonb)
ON CONFLICT (id) DO NOTHING;

INSERT INTO ont_analytics (id, org_id, object_type_id, key, title, formula, result_type) VALUES
  ('00000000-0000-0000-0000-000000a91201', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000a70001', 'labor_cost_won', '인건비 = 계약금 × 인건비율', '{"expr":"annual_fee_won * labor_cost_ratio_pct / 100"}'::jsonb, '{"kind":"number"}'::jsonb),
  ('00000000-0000-0000-0000-000000a91202', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000a70001', 'margin_won', '마진 = 계약금 − 인건비', '{"expr":"annual_fee_won - labor_cost_won"}'::jsonb, '{"kind":"number"}'::jsonb)
ON CONFLICT (id) DO NOTHING;

-- ── ontology graph density (round 5): service_plan/vendor_partner/site_visit
--    object types, all linked FROM the hub contract (a90001), so the graph
--    explorer's default depth-2 search-around renders a real multi-node graph
--    instead of a 2-node island. Every new stable_key sorts AFTER
--    'maintenance_contract' alphabetically, so the hub type stays the graph
--    root's type. ──────────────────────────────────────────────────────────
INSERT INTO ont_object_types (id, org_id, stable_key, title, title_property_key, backing_kind, schema_version, lifecycle_state, created_by) VALUES
  ('00000000-0000-0000-0000-000000d50001', '00000000-0000-0000-0000-0000000000a1', 'service_plan', '정비 계획', 'name', 'instance', 1, 'published', '00000000-0000-0000-0000-00000000d001'),
  ('00000000-0000-0000-0000-000000d50002', '00000000-0000-0000-0000-0000000000a1', 'vendor_partner', '협력업체', 'name', 'instance', 1, 'published', '00000000-0000-0000-0000-00000000d001'),
  ('00000000-0000-0000-0000-000000d50003', '00000000-0000-0000-0000-0000000000a1', 'site_visit', '점검 방문', 'name', 'instance', 1, 'published', '00000000-0000-0000-0000-00000000d001')
ON CONFLICT (id) DO NOTHING;

INSERT INTO ont_property_defs (id, org_id, object_type_id, key, title, type, config, required, in_property_policy) VALUES
  ('00000000-0000-0000-0000-000000d60001', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d50001', 'name', '계획명', 'string', '{}'::jsonb, true, false),
  ('00000000-0000-0000-0000-000000d60002', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d50001', 'period', '대상 기간', 'string', '{}'::jsonb, false, false),
  ('00000000-0000-0000-0000-000000d60003', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d50002', 'name', '업체명', 'string', '{}'::jsonb, true, false),
  ('00000000-0000-0000-0000-000000d60004', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d50002', 'item', '공급 품목', 'string', '{}'::jsonb, false, false),
  ('00000000-0000-0000-0000-000000d60005', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d50003', 'name', '방문명', 'string', '{}'::jsonb, true, false),
  ('00000000-0000-0000-0000-000000d60006', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d50003', 'visit_date', '방문일', 'string', '{}'::jsonb, false, false),
  ('00000000-0000-0000-0000-000000d60007', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d50003', 'technician', '담당 기술자', 'string', '{}'::jsonb, false, false)
ON CONFLICT (id) DO NOTHING;

INSERT INTO ont_link_types (id, org_id, object_type_id, stable_key, title, reverse_title, to_object_type_id, cardinality) VALUES
  ('00000000-0000-0000-0000-000000d70001', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000a70001', 'covers_plan', '정비 계획', '소속 계약', '00000000-0000-0000-0000-000000d50001', 'one_many'),
  ('00000000-0000-0000-0000-000000d70002', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000a70001', 'has_vendor', '협력업체', '계약', '00000000-0000-0000-0000-000000d50002', 'many_many'),
  ('00000000-0000-0000-0000-000000d70003', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d50001', 'has_visit', '점검 방문', '소속 계획', '00000000-0000-0000-0000-000000d50003', 'one_many')
ON CONFLICT (id) DO NOTHING;

-- The protected audit guard accepts the retained runtime path only when the
-- matching parent was inserted by this transaction. The xmin filter makes a
-- successful reseed a true no-op: existing parents receive no duplicate audit.
INSERT INTO audit_events (actor, action, target_type, target_id, branch_id, before_snap, after_snap, trace_id, span_id, occurred_at, org_id)
SELECT
  '00000000-0000-0000-0000-00000000d001',
  'ontology.object_type.create',
  'ont_object_types',
  o.id::text,
  NULL,
  NULL,
  jsonb_build_object(
    'stable_key', o.stable_key,
    'schema_version', o.schema_version,
    'lifecycle_state', o.lifecycle_state
  ),
  repeat('d', 32),
  repeat('e', 16),
  o.updated_at,
  o.org_id
FROM ont_object_types o
WHERE o.id = ANY (ARRAY[
  '00000000-0000-0000-0000-000000a70001'::uuid,
  '00000000-0000-0000-0000-000000d50001'::uuid,
  '00000000-0000-0000-0000-000000d50002'::uuid,
  '00000000-0000-0000-0000-000000d50003'::uuid
])
  AND o.xmin = pg_current_xact_id()::xid;

RESET ROLE;

INSERT INTO ont_instances (id, org_id, object_type_id, title, current_revision_id, lifecycle_state) VALUES
  ('00000000-0000-0000-0000-000000d80001', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d50001', '1월-3월 정기점검 계획', '00000000-0000-0000-0000-000000db0001', 'active'),
  ('00000000-0000-0000-0000-000000d80002', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d50001', '4월-6월 정기점검 계획', '00000000-0000-0000-0000-000000db0002', 'active'),
  ('00000000-0000-0000-0000-000000d80003', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d50001', '7월-9월 정기점검 계획', '00000000-0000-0000-0000-000000db0003', 'active'),
  ('00000000-0000-0000-0000-000000d80004', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d50001', '10월-12월 정기점검 계획', '00000000-0000-0000-0000-000000db0004', 'active'),
  ('00000000-0000-0000-0000-000000d80005', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d50001', '긴급 유압계통 점검 계획', '00000000-0000-0000-0000-000000db0005', 'active')
ON CONFLICT (id) DO NOTHING;

INSERT INTO ont_instances (id, org_id, object_type_id, title, current_revision_id, lifecycle_state) VALUES
  ('00000000-0000-0000-0000-000000d90001', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d50002', '경남유압산업', '00000000-0000-0000-0000-000000db1001', 'active'),
  ('00000000-0000-0000-0000-000000d90002', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d50002', '부산배터리텍', '00000000-0000-0000-0000-000000db1002', 'active'),
  ('00000000-0000-0000-0000-000000d90003', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d50002', '창원공구상사', '00000000-0000-0000-0000-000000db1003', 'active')
ON CONFLICT (id) DO NOTHING;

INSERT INTO ont_instances (id, org_id, object_type_id, title, current_revision_id, lifecycle_state) VALUES
  ('00000000-0000-0000-0000-000000da0001', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d50003', '1월 정기점검 방문', '00000000-0000-0000-0000-000000db2001', 'active'),
  ('00000000-0000-0000-0000-000000da0002', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d50003', '3월 정기점검 방문', '00000000-0000-0000-0000-000000db2002', 'active'),
  ('00000000-0000-0000-0000-000000da0003', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d50003', '4월 정기점검 방문', '00000000-0000-0000-0000-000000db2003', 'active'),
  ('00000000-0000-0000-0000-000000da0004', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d50003', '6월 정기점검 방문', '00000000-0000-0000-0000-000000db2004', 'active'),
  ('00000000-0000-0000-0000-000000da0005', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d50003', '7월 정기점검 방문', '00000000-0000-0000-0000-000000db2005', 'active'),
  ('00000000-0000-0000-0000-000000da0006', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d50003', '9월 정기점검 방문', '00000000-0000-0000-0000-000000db2006', 'active'),
  ('00000000-0000-0000-0000-000000da0007', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d50003', '10월 정기점검 방문', '00000000-0000-0000-0000-000000db2007', 'active'),
  ('00000000-0000-0000-0000-000000da0008', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d50003', '12월 정기점검 방문', '00000000-0000-0000-0000-000000db2008', 'active'),
  ('00000000-0000-0000-0000-000000da0009', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d50003', '유압계통 긴급 점검 1차', '00000000-0000-0000-0000-000000db2009', 'active'),
  ('00000000-0000-0000-0000-000000da0010', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d50003', '유압계통 긴급 점검 2차', '00000000-0000-0000-0000-000000db2010', 'active')
ON CONFLICT (id) DO NOTHING;

INSERT INTO ont_instance_revisions (id, org_id, instance_id, version, attributes, valid_from, prev_hash, row_hash) VALUES
  ('00000000-0000-0000-0000-000000db0001', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d80001', 1, '{"name":"1월-3월 정기점검 계획","period":"2026-Q1"}'::jsonb, now(), repeat('0', 64), encode(sha256('sp-1-v1'), 'hex')),
  ('00000000-0000-0000-0000-000000db0002', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d80002', 1, '{"name":"4월-6월 정기점검 계획","period":"2026-Q2"}'::jsonb, now(), repeat('0', 64), encode(sha256('sp-2-v1'), 'hex')),
  ('00000000-0000-0000-0000-000000db0003', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d80003', 1, '{"name":"7월-9월 정기점검 계획","period":"2026-Q3"}'::jsonb, now(), repeat('0', 64), encode(sha256('sp-3-v1'), 'hex')),
  ('00000000-0000-0000-0000-000000db0004', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d80004', 1, '{"name":"10월-12월 정기점검 계획","period":"2026-Q4"}'::jsonb, now(), repeat('0', 64), encode(sha256('sp-4-v1'), 'hex')),
  ('00000000-0000-0000-0000-000000db0005', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d80005', 1, '{"name":"긴급 유압계통 점검 계획","period":"수시"}'::jsonb, now(), repeat('0', 64), encode(sha256('sp-5-v1'), 'hex'))
ON CONFLICT (id) DO NOTHING;

INSERT INTO ont_instance_revisions (id, org_id, instance_id, version, attributes, valid_from, prev_hash, row_hash) VALUES
  ('00000000-0000-0000-0000-000000db1001', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d90001', 1, '{"name":"경남유압산업","item":"유압 실린더·호스"}'::jsonb, now(), repeat('0', 64), encode(sha256('vp-1-v1'), 'hex')),
  ('00000000-0000-0000-0000-000000db1002', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d90002', 1, '{"name":"부산배터리텍","item":"전동 지게차 배터리"}'::jsonb, now(), repeat('0', 64), encode(sha256('vp-2-v1'), 'hex')),
  ('00000000-0000-0000-0000-000000db1003', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d90003', 1, '{"name":"창원공구상사","item":"일반 소모품·공구"}'::jsonb, now(), repeat('0', 64), encode(sha256('vp-3-v1'), 'hex'))
ON CONFLICT (id) DO NOTHING;

INSERT INTO ont_instance_revisions (id, org_id, instance_id, version, attributes, valid_from, prev_hash, row_hash) VALUES
  ('00000000-0000-0000-0000-000000db2001', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000da0001', 1, '{"name":"1월 정기점검 방문","visit_date":"2026-01-15","technician":"김정비"}'::jsonb, now(), repeat('0', 64), encode(sha256('sv-1-v1'), 'hex')),
  ('00000000-0000-0000-0000-000000db2002', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000da0002', 1, '{"name":"3월 정기점검 방문","visit_date":"2026-03-15","technician":"김정비"}'::jsonb, now(), repeat('0', 64), encode(sha256('sv-2-v1'), 'hex')),
  ('00000000-0000-0000-0000-000000db2003', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000da0003', 1, '{"name":"4월 정기점검 방문","visit_date":"2026-04-15","technician":"최현장"}'::jsonb, now(), repeat('0', 64), encode(sha256('sv-3-v1'), 'hex')),
  ('00000000-0000-0000-0000-000000db2004', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000da0004', 1, '{"name":"6월 정기점검 방문","visit_date":"2026-06-15","technician":"최현장"}'::jsonb, now(), repeat('0', 64), encode(sha256('sv-4-v1'), 'hex')),
  ('00000000-0000-0000-0000-000000db2005', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000da0005', 1, '{"name":"7월 정기점검 방문","visit_date":"2026-07-15","technician":"김정비"}'::jsonb, now(), repeat('0', 64), encode(sha256('sv-5-v1'), 'hex')),
  ('00000000-0000-0000-0000-000000db2006', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000da0006', 1, '{"name":"9월 정기점검 방문","visit_date":"2026-09-15","technician":"김정비"}'::jsonb, now(), repeat('0', 64), encode(sha256('sv-6-v1'), 'hex')),
  ('00000000-0000-0000-0000-000000db2007', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000da0007', 1, '{"name":"10월 정기점검 방문","visit_date":"2026-10-15","technician":"최현장"}'::jsonb, now(), repeat('0', 64), encode(sha256('sv-7-v1'), 'hex')),
  ('00000000-0000-0000-0000-000000db2008', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000da0008', 1, '{"name":"12월 정기점검 방문","visit_date":"2026-12-15","technician":"최현장"}'::jsonb, now(), repeat('0', 64), encode(sha256('sv-8-v1'), 'hex')),
  ('00000000-0000-0000-0000-000000db2009', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000da0009', 1, '{"name":"유압계통 긴급 점검 1차","visit_date":"2026-07-20","technician":"김정비"}'::jsonb, now(), repeat('0', 64), encode(sha256('sv-9-v1'), 'hex')),
  ('00000000-0000-0000-0000-000000db2010', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000da0010', 1, '{"name":"유압계통 긴급 점검 2차","visit_date":"2026-08-05","technician":"김정비"}'::jsonb, now(), repeat('0', 64), encode(sha256('sv-10-v1'), 'hex'))
ON CONFLICT (id) DO NOTHING;

-- edges from the hub: contract→5×plan, contract→3×vendor, plan→2×visit each
-- (19 nodes / 18 edges reachable from a90001 within the default depth-2 traverse)
INSERT INTO ont_links (id, org_id, link_type_id, from_instance_id, to_instance_id, valid_from) VALUES
  ('00000000-0000-0000-0000-000000dc0001', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d70001', '00000000-0000-0000-0000-000000a90001', '00000000-0000-0000-0000-000000d80001', now()),
  ('00000000-0000-0000-0000-000000dc0002', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d70001', '00000000-0000-0000-0000-000000a90001', '00000000-0000-0000-0000-000000d80002', now()),
  ('00000000-0000-0000-0000-000000dc0003', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d70001', '00000000-0000-0000-0000-000000a90001', '00000000-0000-0000-0000-000000d80003', now()),
  ('00000000-0000-0000-0000-000000dc0004', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d70001', '00000000-0000-0000-0000-000000a90001', '00000000-0000-0000-0000-000000d80004', now()),
  ('00000000-0000-0000-0000-000000dc0005', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d70001', '00000000-0000-0000-0000-000000a90001', '00000000-0000-0000-0000-000000d80005', now()),
  ('00000000-0000-0000-0000-000000dc0006', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d70002', '00000000-0000-0000-0000-000000a90001', '00000000-0000-0000-0000-000000d90001', now()),
  ('00000000-0000-0000-0000-000000dc0007', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d70002', '00000000-0000-0000-0000-000000a90001', '00000000-0000-0000-0000-000000d90002', now()),
  ('00000000-0000-0000-0000-000000dc0008', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d70002', '00000000-0000-0000-0000-000000a90001', '00000000-0000-0000-0000-000000d90003', now()),
  ('00000000-0000-0000-0000-000000dc0009', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d70003', '00000000-0000-0000-0000-000000d80001', '00000000-0000-0000-0000-000000da0001', now()),
  ('00000000-0000-0000-0000-000000dc0010', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d70003', '00000000-0000-0000-0000-000000d80001', '00000000-0000-0000-0000-000000da0002', now()),
  ('00000000-0000-0000-0000-000000dc0011', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d70003', '00000000-0000-0000-0000-000000d80002', '00000000-0000-0000-0000-000000da0003', now()),
  ('00000000-0000-0000-0000-000000dc0012', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d70003', '00000000-0000-0000-0000-000000d80002', '00000000-0000-0000-0000-000000da0004', now()),
  ('00000000-0000-0000-0000-000000dc0013', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d70003', '00000000-0000-0000-0000-000000d80003', '00000000-0000-0000-0000-000000da0005', now()),
  ('00000000-0000-0000-0000-000000dc0014', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d70003', '00000000-0000-0000-0000-000000d80003', '00000000-0000-0000-0000-000000da0006', now()),
  ('00000000-0000-0000-0000-000000dc0015', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d70003', '00000000-0000-0000-0000-000000d80004', '00000000-0000-0000-0000-000000da0007', now()),
  ('00000000-0000-0000-0000-000000dc0016', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d70003', '00000000-0000-0000-0000-000000d80004', '00000000-0000-0000-0000-000000da0008', now()),
  ('00000000-0000-0000-0000-000000dc0017', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d70003', '00000000-0000-0000-0000-000000d80005', '00000000-0000-0000-0000-000000da0009', now()),
  ('00000000-0000-0000-0000-000000dc0018', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000d70003', '00000000-0000-0000-0000-000000d80005', '00000000-0000-0000-0000-000000da0010', now())
ON CONFLICT (id) DO NOTHING;

-- ── finance GL vouchers (모듈 / 재무) — created DRAFT, lines attached while
--    DRAFT (the only status the append-only-lines trigger allows), then driven
--    forward through the real FSM so ledger amounts come from actual balanced
--    차/대 lines, not a status literal. GL-0001 reaches 전기(POSTED); GL-0002
--    stops at 차대검증; GL-0003 stops at 승인 (SoD: approver ≠ drafter). ──────
INSERT INTO finance_gl_vouchers (id, org_id, branch_id, voucher_no, status, memo, created_by) VALUES
  ('00000000-0000-0000-0000-000000fc0001', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-0000000000c1', 'GL-2026-0001', 'DRAFT', '유압 실린더 부품 매입', '00000000-0000-0000-0000-00000000d003'),
  ('00000000-0000-0000-0000-000000fc0002', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-0000000000c1', 'GL-2026-0002', 'DRAFT', '전동 지게차 배터리 교체', '00000000-0000-0000-0000-00000000d003'),
  ('00000000-0000-0000-0000-000000fc0003', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-0000000000c2', 'GL-2026-0003', 'DRAFT', '부산 지점 정기점검 부품비', '00000000-0000-0000-0000-00000000d003')
ON CONFLICT (id) DO NOTHING;

-- Lines via NOT EXISTS (not ON CONFLICT): the BEFORE-INSERT draft-only trigger
-- fires per candidate row before a conflict is even detected, so on a re-run
-- against an already-advanced voucher a plain ON CONFLICT INSERT would still
-- raise "not DRAFT" — pre-filtering already-seeded ids out of the SELECT
-- avoids the INSERT (and the trigger) entirely.
INSERT INTO finance_gl_voucher_lines (id, org_id, voucher_id, line_no, account_code, side, amount_won, memo)
SELECT v.id, v.org_id, v.voucher_id, v.line_no, v.account_code, v.side, v.amount_won, v.memo
FROM (VALUES
  ('00000000-0000-0000-0000-000000fd0001'::uuid, '00000000-0000-0000-0000-0000000000a1'::uuid, '00000000-0000-0000-0000-000000fc0001'::uuid, 1, '5104', 'DEBIT', 850000::bigint, '유압 실린더 부품 매입'),
  ('00000000-0000-0000-0000-000000fd0002'::uuid, '00000000-0000-0000-0000-0000000000a1'::uuid, '00000000-0000-0000-0000-000000fc0001'::uuid, 2, '2101', 'CREDIT', 850000::bigint, '경남유압산업 외상매입금'),
  ('00000000-0000-0000-0000-000000fd0003'::uuid, '00000000-0000-0000-0000-0000000000a1'::uuid, '00000000-0000-0000-0000-000000fc0002'::uuid, 1, '5108', 'DEBIT', 1200000::bigint, '전동 지게차 배터리 소모품비'),
  ('00000000-0000-0000-0000-000000fd0004'::uuid, '00000000-0000-0000-0000-0000000000a1'::uuid, '00000000-0000-0000-0000-000000fc0002'::uuid, 2, '2103', 'CREDIT', 1200000::bigint, '부산배터리텍 미지급금'),
  ('00000000-0000-0000-0000-000000fd0005'::uuid, '00000000-0000-0000-0000-0000000000a1'::uuid, '00000000-0000-0000-0000-000000fc0003'::uuid, 1, '5104', 'DEBIT', 620000::bigint, '부산 지점 정기점검 부품비'),
  ('00000000-0000-0000-0000-000000fd0006'::uuid, '00000000-0000-0000-0000-0000000000a1'::uuid, '00000000-0000-0000-0000-000000fc0003'::uuid, 2, '1102', 'CREDIT', 620000::bigint, '보통예금 지급')
) AS v(id, org_id, voucher_id, line_no, account_code, side, amount_won, memo)
WHERE NOT EXISTS (SELECT 1 FROM finance_gl_voucher_lines l WHERE l.id = v.id);

UPDATE finance_gl_vouchers SET status = 'BALANCE_CHECKED'
WHERE id IN ('00000000-0000-0000-0000-000000fc0001', '00000000-0000-0000-0000-000000fc0002', '00000000-0000-0000-0000-000000fc0003')
  AND status = 'DRAFT';

UPDATE finance_gl_vouchers SET status = 'APPROVED', approved_by = '00000000-0000-0000-0000-00000000d004'
WHERE id IN ('00000000-0000-0000-0000-000000fc0001', '00000000-0000-0000-0000-000000fc0003')
  AND status = 'BALANCE_CHECKED';

UPDATE finance_gl_vouchers SET status = 'POSTED', posted_at = now()
WHERE id = '00000000-0000-0000-0000-000000fc0001' AND status = 'APPROVED';

-- ── support tickets (지원 센터) — one assigned to the dev SUPER_ADMIN so the
--     overview action-inbox ("내 처리 대기") also surfaces a row. ─────────────
INSERT INTO support_tickets (id, branch_id, origin, category, priority, status, title, body, requester_name, requester_contact, assignee_user_id, org_id) VALUES
  ('00000000-0000-0000-0000-0000005c0001', '00000000-0000-0000-0000-0000000000c1', 'CUSTOMER', 'EQUIPMENT_INQUIRY', 'HIGH', 'OPEN', '지게차 시동 불량 문의', '전동 지게차가 아침부터 시동이 걸리지 않습니다.', '한성물류 김과장', '055-1234-5678', '00000000-0000-0000-0000-00000000d001', '00000000-0000-0000-0000-0000000000a1'),
  ('00000000-0000-0000-0000-0000005c0002', '00000000-0000-0000-0000-0000000000c1', 'CUSTOMER', 'OPERATIONAL', 'MEDIUM', 'IN_PROGRESS', '정기 점검 일정 문의', '다음 달 정기 점검 일정을 조율하고 싶습니다.', '한성물류 이대리', 'lee@hansung.co.kr', '00000000-0000-0000-0000-00000000d003', '00000000-0000-0000-0000-0000000000a1'),
  ('00000000-0000-0000-0000-0000005c0003', '00000000-0000-0000-0000-0000000000c2', 'CUSTOMER', 'COMPLAINT', 'URGENT', 'RESOLVED', '유압 누유 재발', '지난 번 수리한 부위에서 다시 누유가 발생했습니다.', '부산 창고 박팀장', '051-9876-5432', '00000000-0000-0000-0000-00000000d002', '00000000-0000-0000-0000-0000000000a1')
ON CONFLICT (id) DO NOTHING;

-- support depth (r11): 3 customer tickets left the 지원 센터 list thin against
-- the reference's 8-row list (열린 티켓 + FAQ 셀프해결). Add five INTERNAL staff
-- tickets — access/how-to topics that mirror the reference's FAQ entries; most
-- RESOLVED (self-served) so the 해결 stat surfaces a real count. INTERNAL origin
-- carries a requester_user_id (not free-form name/contact), so this needs its
-- own column list + the CHECK's requester_name/contact = NULL.
INSERT INTO support_tickets (id, branch_id, origin, category, priority, status, title, body, requester_user_id, assignee_user_id, org_id, resolved_at) VALUES
  ('00000000-0000-0000-0000-0000005c0004', '00000000-0000-0000-0000-0000000000c1', 'INTERNAL', 'ACCESS_REQUEST', 'MEDIUM', 'IN_PROGRESS', 'passkey 재등록 요청 (기기 교체)', '휴대폰을 교체해서 passkey 재등록이 필요합니다.', '00000000-0000-0000-0000-00000000d006', '00000000-0000-0000-0000-00000000d001', '00000000-0000-0000-0000-0000000000a1', NULL),
  ('00000000-0000-0000-0000-0000005c0005', '00000000-0000-0000-0000-0000000000c1', 'INTERNAL', 'ACCESS_REQUEST', 'LOW', 'RESOLVED', 'passkey가 인식되지 않을 때', '지문 등록을 다시 하니 정상 로그인됐습니다.', '00000000-0000-0000-0000-00000000d002', '00000000-0000-0000-0000-00000000d003', '00000000-0000-0000-0000-0000000000a1', now() - interval '2 days'),
  ('00000000-0000-0000-0000-0000005c0006', '00000000-0000-0000-0000-0000000000c1', 'INTERNAL', 'OPERATIONAL', 'LOW', 'RESOLVED', '수신함 문서가 잠겨 있어요', '보존 기간 문서라 잠금 상태였고 담당자 승인 후 열람했습니다.', '00000000-0000-0000-0000-00000000d007', '00000000-0000-0000-0000-00000000d003', '00000000-0000-0000-0000-0000000000a1', now() - interval '4 days'),
  ('00000000-0000-0000-0000-0000005c0007', '00000000-0000-0000-0000-0000000000c2', 'INTERNAL', 'SYSTEM_BUG', 'LOW', 'RESOLVED', '내보내기가 차단됐어요', '권한 정책상 대외비 문서는 내보내기가 제한됨을 확인했습니다.', '00000000-0000-0000-0000-00000000d005', '00000000-0000-0000-0000-00000000d003', '00000000-0000-0000-0000-0000000000a1', now() - interval '6 days'),
  ('00000000-0000-0000-0000-0000005c0008', '00000000-0000-0000-0000-0000000000c1', 'INTERNAL', 'OPERATIONAL', 'LOW', 'RESOLVED', '대근 편성은 어디서 하나요', '근태 화면의 편성 탭에서 처리하면 됩니다.', '00000000-0000-0000-0000-00000000d008', '00000000-0000-0000-0000-00000000d003', '00000000-0000-0000-0000-0000000000a1', now() - interval '1 day')
ON CONFLICT (id) DO NOTHING;

-- ── overview work-queue (결재/배차/정비/회신): one item per ActionInboxItem
--    source so GET /api/v1/me/action-inbox (and its stat-strip/chip derivation
--    in overviewModel.ts) returns a non-empty row for every kind. 회신
--    (support) already has ticket sc0001 assigned to d001 above. ────────────

-- 정비 — an assignment on the existing ad0003 (ASSIGNED) puts it on d001's
-- "assigned to me" work list; target_due_at re-stamped to *today* on every
-- reseed so the "오늘 마감"/urgency chips stay live against wall-clock now().
INSERT INTO work_order_assignments (id, org_id, work_order_id, mechanic_id, role, assigned_at) VALUES
  ('00000000-0000-0000-0000-000000ec0001', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000ad0003', '00000000-0000-0000-0000-00000000d001', 'PRIMARY', now())
ON CONFLICT (work_order_id, mechanic_id) DO NOTHING;

UPDATE work_orders SET target_due_at = date_trunc('day', now()) + interval '20 hours'
WHERE id = '00000000-0000-0000-0000-000000ad0003';

-- 배차 — a P1 emergency work order broadcasting for dispatch, with d001 as one
-- of the offered technicians (list_my_pending_offers: BROADCASTING + a live
-- accept window + a TECHNICIAN target row + no response yet).
INSERT INTO work_orders (
  id, request_no, branch_id, equipment_id, customer_id, site_id, requested_by, status, priority,
  symptom, customer_request, result_type, kpi_excluded, evidence_verified, org_id
) VALUES
  ('00000000-0000-0000-0000-000000ad0004', '20260710-004', '00000000-0000-0000-0000-0000000000c1', '00000000-0000-0000-0000-000000c20002', '00000000-0000-0000-0000-000000c00001', '00000000-0000-0000-0000-000000c10001', '00000000-0000-0000-0000-00000000d003', 'UNASSIGNED', 'P1', '화물 리프트 완전 정지 - 유압펌프 고장 의심', '리프트가 전혀 올라가지 않습니다. 긴급 점검 요청', 'UNKNOWN', false, false, '00000000-0000-0000-0000-0000000000a1')
ON CONFLICT (id) DO NOTHING;

INSERT INTO p1_dispatches (id, org_id, work_order_id, branch_id, status, accept_window_started_at, accept_window_ends_at, created_by, created_at, updated_at) VALUES
  ('00000000-0000-0000-0000-000000ea0001', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000ad0004', '00000000-0000-0000-0000-0000000000c1', 'BROADCASTING', now(), now() + interval '6 hours', '00000000-0000-0000-0000-00000000d003', now(), now())
ON CONFLICT (id) DO NOTHING;

INSERT INTO p1_dispatch_targets (id, org_id, dispatch_id, user_id, target_role, fanout_created_at) VALUES
  ('00000000-0000-0000-0000-000000eb0001', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000ea0001', '00000000-0000-0000-0000-00000000d001', 'TECHNICIAN', now()),
  ('00000000-0000-0000-0000-000000eb0002', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000ea0001', '00000000-0000-0000-0000-00000000d002', 'TECHNICIAN', now())
ON CONFLICT (dispatch_id, user_id) DO NOTHING;

-- 결재 — a real workflow run + waiting task on the published work_order.approval
-- definition, already claimed by d001 (task_visible's `claimed_by = me` path,
-- no dependency on role-key grants).
INSERT INTO workflow_runs (
  id, org_id, definition_id, definition_version, status, trigger_type, object_type, object_id,
  idempotency_key, correlation_id, initiated_by, started_at, updated_at
) VALUES
  ('00000000-0000-0000-0000-000000e80001', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000f00001', 1, 'WAITING', 'OBJECT_EVENT', 'work_order', '00000000-0000-0000-0000-000000ad0002', 'seed-approval-wo-ad0002', 'seed-corr-wo-ad0002', '00000000-0000-0000-0000-00000000d003', now(), now())
ON CONFLICT (id) DO NOTHING;

INSERT INTO workflow_waiting_tasks (
  id, org_id, run_id, waiting_key, title, status, assignee_role_key, source_object_type, source_object_id,
  due_at, claimed_by, claimed_at
) VALUES
  ('00000000-0000-0000-0000-000000e90001', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000e80001', 'approve_completion', '작업지시 완료 승인 요청', 'CLAIMED', 'branch_manager', 'work_order', '00000000-0000-0000-0000-000000ad0002', now() + interval '6 hours', '00000000-0000-0000-0000-00000000d001', now())
ON CONFLICT (id) DO NOTHING;

-- ── overview 결재 대기 depth (r14): one waiting task (e90001) left the queue
-- with a single 결재 row and NO 긴급 sub-value on the stat strip (긴급 = an
-- approval whose due_at is already past → urg "now"). Seed four more CLAIMED-by
-- -me tasks under one WAITING run: two OVERDUE (긴급 2) + two due later today,
-- so 결재 대기 reads 5 · 긴급 2 and today's agenda carries real approval rows.
-- Same real shape as e90001 (CLAIMED + claimed_by = d001, branch_manager line).
INSERT INTO workflow_runs (
  id, org_id, definition_id, definition_version, status, trigger_type, object_type, object_id,
  idempotency_key, correlation_id, initiated_by, started_at, updated_at
) VALUES
  ('00000000-0000-0000-0000-000000e80006', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000f00001', 1, 'WAITING', 'OBJECT_EVENT', 'work_order', '00000000-0000-0000-0000-000000ad0005', 'seed-approval-volume-run-ad0005', 'seed-corr-approval-volume-ad0005', '00000000-0000-0000-0000-00000000d003', now(), now())
ON CONFLICT (id) DO NOTHING;

INSERT INTO workflow_waiting_tasks (
  id, org_id, run_id, waiting_key, title, status, assignee_role_key, source_object_type, source_object_id,
  due_at, claimed_by, claimed_at
) VALUES
  ('00000000-0000-0000-0000-000000e90002', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000e80006', 'approve_completion_v1', '월 정비비 정산 승인 요청', 'CLAIMED', 'branch_manager', 'work_order', '00000000-0000-0000-0000-000000ad0005', now() - interval '3 hours', '00000000-0000-0000-0000-00000000d001', now()),
  ('00000000-0000-0000-0000-000000e90003', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000e80006', 'approve_completion_v2', '외주 정비 대금 지급 승인', 'CLAIMED', 'branch_manager', 'work_order', '00000000-0000-0000-0000-000000ad0006', now() - interval '1 hour', '00000000-0000-0000-0000-00000000d001', now()),
  ('00000000-0000-0000-0000-000000e90004', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000e80006', 'approve_completion_v3', '긴급 부품 발주 승인', 'CLAIMED', 'branch_manager', 'work_order', '00000000-0000-0000-0000-000000ad0007', now() + interval '2 hours', '00000000-0000-0000-0000-00000000d001', now()),
  ('00000000-0000-0000-0000-000000e90005', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000e80006', 'approve_completion_v4', '연장근로 시간 승인', 'CLAIMED', 'branch_manager', 'work_order', '00000000-0000-0000-0000-000000ad0008', now() + interval '5 hours', '00000000-0000-0000-0000-00000000d001', now())
ON CONFLICT (id) DO NOTHING;

-- ── workflow run history (자동화 → 워크플로 스튜디오 실행 이력, r9): the
-- run-log panel reads settled workflow_runs (SUCCEEDED/FAILED) with a duration
-- (started_at→completed_at/failed_at) and output_payload.generated_objects for
-- the object chips. With only the single WAITING run above the 실행 이력 stays
-- empty; seed a handful of settled runs so the panel shows real executions with
-- durations and generated-object chips (the real run write shape).
INSERT INTO workflow_runs (
  id, org_id, definition_id, definition_version, status, trigger_type, object_type, object_id,
  idempotency_key, correlation_id, initiated_by, output_payload, error_payload, started_at, updated_at, completed_at, failed_at
) VALUES
  ('00000000-0000-0000-0000-000000e80002', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000f00001', 1, 'SUCCEEDED', 'OBJECT_EVENT', 'work_order', '00000000-0000-0000-0000-000000ad0001', 'seed-run-ad0001-approve', 'seed-corr-run-ad0001', '00000000-0000-0000-0000-00000000d003', '{"generated_objects":["20260701-001"]}'::jsonb, NULL, now() - interval '6 hours', now() - interval '6 hours' + interval '800 milliseconds', now() - interval '6 hours' + interval '800 milliseconds', NULL),
  ('00000000-0000-0000-0000-000000e80003', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000f00001', 1, 'SUCCEEDED', 'MANUAL', 'work_order', '00000000-0000-0000-0000-000000ad0005', 'seed-run-ad0005-approve', 'seed-corr-run-ad0005', '00000000-0000-0000-0000-00000000d001', '{"generated_objects":["20260601-005"]}'::jsonb, NULL, now() - interval '2 days', now() - interval '2 days' + interval '600 milliseconds', now() - interval '2 days' + interval '600 milliseconds', NULL),
  ('00000000-0000-0000-0000-000000e80004', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000f00001', 1, 'SUCCEEDED', 'OBJECT_EVENT', 'work_order', '00000000-0000-0000-0000-000000ad0006', 'seed-run-ad0006-approve', 'seed-corr-run-ad0006', '00000000-0000-0000-0000-00000000d003', '{"generated_objects":["20260501-006"]}'::jsonb, NULL, now() - interval '9 days', now() - interval '9 days' + interval '1200 milliseconds', now() - interval '9 days' + interval '1200 milliseconds', NULL),
  ('00000000-0000-0000-0000-000000e80005', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000f00001', 1, 'FAILED', 'OBJECT_EVENT', 'work_order', '00000000-0000-0000-0000-000000ad0007', 'seed-run-ad0007-approve', 'seed-corr-run-ad0007', '00000000-0000-0000-0000-00000000d003', '{}'::jsonb, '{"error":"승인자 부재로 자동 반려"}'::jsonb, now() - interval '15 days', now() - interval '15 days' + interval '400 milliseconds', NULL, now() - interval '15 days' + interval '400 milliseconds')
ON CONFLICT (id) DO NOTHING;

-- ── messenger threads (커뮤니케이션 / 메신저) — two named branch channels, one
--    work-order auto-thread, one DM, dense with real message rows. ─────────
INSERT INTO messenger_threads (id, org_id, kind, branch_id, work_order_id, title, created_by, visibility) VALUES
  ('00000000-0000-0000-0000-000000e10001', '00000000-0000-0000-0000-0000000000a1', 'team', '00000000-0000-0000-0000-0000000000c1', NULL, '정비팀 공지', '00000000-0000-0000-0000-00000000d001', 'channel'),
  ('00000000-0000-0000-0000-000000e10002', '00000000-0000-0000-0000-0000000000a1', 'team', '00000000-0000-0000-0000-0000000000c1', NULL, '창원 본사 전체', '00000000-0000-0000-0000-00000000d001', 'channel'),
  ('00000000-0000-0000-0000-000000e10003', '00000000-0000-0000-0000-0000000000a1', 'work_order', '00000000-0000-0000-0000-0000000000c1', '00000000-0000-0000-0000-000000ad0002', NULL, '00000000-0000-0000-0000-00000000d003', 'direct'),
  ('00000000-0000-0000-0000-000000e10004', '00000000-0000-0000-0000-0000000000a1', 'dm', '00000000-0000-0000-0000-0000000000c1', NULL, NULL, '00000000-0000-0000-0000-00000000d001', 'direct')
ON CONFLICT (id) DO NOTHING;

INSERT INTO messenger_thread_members (thread_id, org_id, user_id, role, joined_at) VALUES
  ('00000000-0000-0000-0000-000000e10001', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-00000000d001', 'OWNER', now()),
  ('00000000-0000-0000-0000-000000e10001', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-00000000d002', 'MEMBER', now()),
  ('00000000-0000-0000-0000-000000e10001', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-00000000d003', 'MEMBER', now()),
  ('00000000-0000-0000-0000-000000e10002', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-00000000d001', 'OWNER', now()),
  ('00000000-0000-0000-0000-000000e10002', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-00000000d002', 'MEMBER', now()),
  ('00000000-0000-0000-0000-000000e10002', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-00000000d003', 'MEMBER', now()),
  ('00000000-0000-0000-0000-000000e10002', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-00000000d004', 'MEMBER', now()),
  ('00000000-0000-0000-0000-000000e10003', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-00000000d002', 'MEMBER', now()),
  ('00000000-0000-0000-0000-000000e10003', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-00000000d003', 'MEMBER', now()),
  ('00000000-0000-0000-0000-000000e10004', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-00000000d001', 'OWNER', now()),
  ('00000000-0000-0000-0000-000000e10004', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-00000000d004', 'MEMBER', now())
ON CONFLICT (thread_id, user_id) DO NOTHING;

INSERT INTO messenger_messages (id, org_id, thread_id, branch_id, sender_id, body, sent_at) VALUES
  ('00000000-0000-0000-0000-000000e30001', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000e10001', '00000000-0000-0000-0000-0000000000c1', '00000000-0000-0000-0000-00000000d002', '창원 본사 유압호스 재고 3개 남았습니다. 추가 발주 부탁드립니다.', now() - interval '2 hours'),
  ('00000000-0000-0000-0000-000000e30002', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000e10001', '00000000-0000-0000-0000-0000000000c1', '00000000-0000-0000-0000-00000000d001', '네, 경남유압산업에 발주 넣겠습니다.', now() - interval '90 minutes'),
  ('00000000-0000-0000-0000-000000e30003', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000e10001', '00000000-0000-0000-0000-0000000000c1', '00000000-0000-0000-0000-00000000d003', '부산 지점도 재고 확인 부탁드려요.', now() - interval '30 minutes'),
  ('00000000-0000-0000-0000-000000e30004', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000e10002', '00000000-0000-0000-0000-0000000000c1', '00000000-0000-0000-0000-00000000d004', '이번 달 정비팀 목표 완료율 92% 달성했습니다. 수고 많으셨습니다.', now() - interval '1 day'),
  ('00000000-0000-0000-0000-000000e30005', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000e10002', '00000000-0000-0000-0000-0000000000c1', '00000000-0000-0000-0000-00000000d001', '감사합니다. 다음 달도 이어가시죠.', now() - interval '23 hours'),
  ('00000000-0000-0000-0000-000000e30006', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000e10003', '00000000-0000-0000-0000-0000000000c1', '00000000-0000-0000-0000-00000000d003', '고객 문의: 배터리 충전이 전혀 안 된다고 합니다.', now() - interval '3 hours'),
  ('00000000-0000-0000-0000-000000e30007', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000e10003', '00000000-0000-0000-0000-0000000000c1', '00000000-0000-0000-0000-00000000d002', '충전기 및 배터리 셀 점검 중입니다.', now() - interval '2 hours 30 minutes'),
  ('00000000-0000-0000-0000-000000e30008', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000e10004', '00000000-0000-0000-0000-0000000000c1', '00000000-0000-0000-0000-00000000d004', '이번 주 임원 보고 자료 준비됐나요?', now() - interval '5 hours'),
  ('00000000-0000-0000-0000-000000e30009', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000e10004', '00000000-0000-0000-0000-0000000000c1', '00000000-0000-0000-0000-00000000d001', '네, 오늘 오후에 공유드리겠습니다.', now() - interval '4 hours')
ON CONFLICT (id) DO NOTHING;

-- ── mail (커뮤니케이션 / 메일) — one org-wide corporate mailbox account (the
--    read API resolves "the" account = latest email_accounts row for the org;
--    credentials are inert 1-byte placeholders, never real ciphertext, since
--    the dev harness never runs a live IMAP/SMTP sync). ─────────────────────
INSERT INTO email_accounts (
  id, org_id, branch_id, display_name, email_address, from_name,
  imap_host, imap_port, imap_security, imap_username,
  smtp_host, smtp_port, smtp_security, smtp_username,
  smtp_password_ct, smtp_password_nonce, imap_password_ct, imap_password_nonce,
  dek_wrapped, dek_nonce, imap_dek_wrapped, imap_dek_nonce,
  status, created_by
) VALUES (
  '00000000-0000-0000-0000-000000e40001', '00000000-0000-0000-0000-0000000000a1', NULL, 'KNL 대표 메일', 'office@knl-logistics.co.kr', 'KNL 로지스틱스',
  'imap.knl-logistics.co.kr', 993, 'TLS', 'office@knl-logistics.co.kr',
  'smtp.knl-logistics.co.kr', 587, 'STARTTLS', 'office@knl-logistics.co.kr',
  '\x00'::bytea, '\x00'::bytea, '\x00'::bytea, '\x00'::bytea,
  '\x00'::bytea, '\x00'::bytea, '\x00'::bytea, '\x00'::bytea,
  'ACTIVE', '00000000-0000-0000-0000-00000000d001'
) ON CONFLICT (id) DO NOTHING;

INSERT INTO email_folders (id, org_id, account_id, imap_path, role, name, unread_count, total_count) VALUES
  ('00000000-0000-0000-0000-000000e50001', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000e40001', 'INBOX', 'INBOX', '받은편지함', 3, 5)
ON CONFLICT (id) DO NOTHING;

INSERT INTO email_threads (id, org_id, account_id, normalized_subject, subject, last_message_at, message_count, unread_count, has_attachments, linked_customer_id) VALUES
  ('00000000-0000-0000-0000-000000e60001', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000e40001', '한성물류 정비 견적 요청', '[한성물류] 정비 견적 요청 드립니다', now() - interval '1 hour', 2, 1, false, '00000000-0000-0000-0000-000000c00001'),
  ('00000000-0000-0000-0000-000000e60002', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000e40001', '부산 지점 소모품 발주 확인', '부산 지점 소모품 발주 확인 요청', now() - interval '5 hours', 1, 1, false, NULL),
  ('00000000-0000-0000-0000-000000e60003', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000e40001', '경남유압산업 납품 확인', '경남유압산업 유압호스 납품 확인', now() - interval '8 hours', 1, 1, false, NULL),
  ('00000000-0000-0000-0000-000000e60004', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000e40001', '정기 점검 일정 조율 회신', 'RE: 정기 점검 일정 조율', now() - interval '1 day', 1, 0, false, '00000000-0000-0000-0000-000000c00001')
ON CONFLICT (id) DO NOTHING;

INSERT INTO email_messages (
  id, org_id, account_id, folder_id, thread_id, direction, from_address, from_name, to_addresses,
  subject, snippet, body_text, received_at, sent_at, seen
) VALUES
  ('00000000-0000-0000-0000-000000e70001', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000e40001', '00000000-0000-0000-0000-000000e50001', '00000000-0000-0000-0000-000000e60001', 'IN', 'kim@hansung.co.kr', '한성물류 김과장', '[{"address":"office@knl-logistics.co.kr"}]'::jsonb, '[한성물류] 정비 견적 요청 드립니다', '창원센터 지게차 2대 정비 견적 부탁드립니다.', '창원센터 지게차 2대 정비 견적 부탁드립니다.', now() - interval '3 hours', NULL, false),
  ('00000000-0000-0000-0000-000000e70002', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000e40001', '00000000-0000-0000-0000-000000e50001', '00000000-0000-0000-0000-000000e60001', 'OUT', 'office@knl-logistics.co.kr', 'KNL 로지스틱스', '[{"address":"kim@hansung.co.kr"}]'::jsonb, 'RE: [한성물류] 정비 견적 요청 드립니다', '견적서 첨부해 드립니다. 확인 부탁드립니다.', '견적서 첨부해 드립니다. 확인 부탁드립니다.', now() - interval '1 hour', now() - interval '1 hour', true),
  ('00000000-0000-0000-0000-000000e70003', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000e40001', '00000000-0000-0000-0000-000000e50001', '00000000-0000-0000-0000-000000e60002', 'IN', 'sales@changwon-tools.co.kr', '창원공구상사', '[{"address":"office@knl-logistics.co.kr"}]'::jsonb, '부산 지점 소모품 발주 확인 요청', '발주하신 소모품 목록 확인 부탁드립니다.', '발주하신 소모품 목록 확인 부탁드립니다.', now() - interval '5 hours', NULL, false),
  ('00000000-0000-0000-0000-000000e70004', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000e40001', '00000000-0000-0000-0000-000000e50001', '00000000-0000-0000-0000-000000e60003', 'IN', 'order@gn-hydraulic.co.kr', '경남유압산업', '[{"address":"office@knl-logistics.co.kr"}]'::jsonb, '경남유압산업 유압호스 납품 확인', '유압호스 10개 오늘 오후 배송 예정입니다.', '유압호스 10개 오늘 오후 배송 예정입니다.', now() - interval '8 hours', NULL, false),
  ('00000000-0000-0000-0000-000000e70005', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000e40001', '00000000-0000-0000-0000-000000e50001', '00000000-0000-0000-0000-000000e60004', 'IN', 'lee@hansung.co.kr', '한성물류 이대리', '[{"address":"office@knl-logistics.co.kr"}]'::jsonb, 'RE: 정기 점검 일정 조율', '다음 주 화요일 오전으로 확정하겠습니다.', '다음 주 화요일 오전으로 확정하겠습니다.', now() - interval '1 day', NULL, true)
ON CONFLICT (id) DO NOTHING;

-- ── notifications (개요 / 알림) — recipient = dev SUPER_ADMIN (d001) ──────────
INSERT INTO notifications (id, org_id, recipient_user_id, category, kind, body, link, unread) VALUES
  ('00000000-0000-0000-0000-000000ce0001', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-00000000d001', 'support', 'ticket_assigned', '새 지원 요청이 배정되었습니다: 지게차 시동 불량 문의', '{"type":"object","kind":"support_ticket","id":"00000000-0000-0000-0000-0000005c0001"}'::jsonb, true),
  ('00000000-0000-0000-0000-000000ce0002', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-00000000d001', 'leave', 'approval_pending', '연차 신청 승인 대기: 김정비 (2일)', '{"type":"object","kind":"leave_request","id":"00000000-0000-0000-0000-000000a20001"}'::jsonb, true),
  ('00000000-0000-0000-0000-000000ce0003', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-00000000d001', 'finance', 'voucher_posted', 'GL 전표가 전기되었습니다: GL-2026-0001', '{"type":"object","kind":"gl_voucher","id":"00000000-0000-0000-0000-000000fc0001"}'::jsonb, false),
  ('00000000-0000-0000-0000-000000ce0004', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-00000000d001', 'support', 'slo_violation', 'SLA 위반: 지게차 시동 불량 문의 응답 기한 초과', '{"type":"object","kind":"support_ticket","id":"00000000-0000-0000-0000-0000005c0001"}'::jsonb, true),
  ('00000000-0000-0000-0000-000000ce0005', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-00000000d001', '메신저', 'info', '정비팀 공지에 새 메시지가 있습니다: 유압호스 재고 확보 요청', '{"type":"object","kind":"messenger_thread","id":"00000000-0000-0000-0000-000000e10001"}'::jsonb, true),
  ('00000000-0000-0000-0000-000000ce0006', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-00000000d001', '공지', 'info', '여름철 지게차 냉각계통 점검 캠페인 안내', '{"type":"screen","screen":"notice"}'::jsonb, false)
ON CONFLICT (id) DO NOTHING;

-- ── dashboard KPI aggregation: completed work orders + completion approvals ───
-- WHY: the KPI rollup (backend reporting adapter) only counts a work order once
-- it carries an EXECUTIVE/ADMIN `work_order_approval_steps` row APPROVED within
-- the queried period. Without those steps every dashboard aggregate is zero
-- ("이 기간에 집계된 승인 보고가 없습니다"). ad0001 (already FINAL_COMPLETED)
-- had no approval step; we add its step plus one completed work order per
-- trailing month so the KPI stat strip, the ops summary, and the 완료 추이
-- projection all compute REAL numbers. approved_at is relative to now() so the
-- fixture stays inside the dashboard's rolling 6-month window on any dev-up.
-- This mirrors the real completion-approval write shape exactly.
-- r12: created_at/target_due_at added (same 미지정 fix as ad0001 above) — 3 of
-- 5 meet target_due_at (compliant), 2 miss it, for a realistic <100% bps.
INSERT INTO work_orders (
  id, request_no, branch_id, equipment_id, customer_id, site_id, requested_by, status, priority,
  symptom, customer_request, result_type, kpi_excluded, evidence_verified, org_id, created_at, target_due_at
) VALUES
  ('00000000-0000-0000-0000-000000ad0005', '20260601-005', '00000000-0000-0000-0000-0000000000c1', '00000000-0000-0000-0000-000000c20001', '00000000-0000-0000-0000-000000c00001', '00000000-0000-0000-0000-000000c10001', '00000000-0000-0000-0000-00000000d003', 'FINAL_COMPLETED', 'P2', '체인 장력 조정', '리프트 체인 소음', 'COMPLETED', false, true, '00000000-0000-0000-0000-0000000000a1', date_trunc('month', now()) - interval '1 month' + interval '7 days', date_trunc('month', now()) - interval '1 month' + interval '11 days'),
  ('00000000-0000-0000-0000-000000ad0006', '20260501-006', '00000000-0000-0000-0000-0000000000c1', '00000000-0000-0000-0000-000000c20002', '00000000-0000-0000-0000-000000c00001', '00000000-0000-0000-0000-000000c10001', '00000000-0000-0000-0000-00000000d003', 'FINAL_COMPLETED', 'P3', '타이어 마모 교체', '구동 타이어 교체', 'COMPLETED', false, true, '00000000-0000-0000-0000-0000000000a1', date_trunc('month', now()) - interval '2 months' + interval '7 days', date_trunc('month', now()) - interval '2 months' + interval '9 days 12 hours'),
  ('00000000-0000-0000-0000-000000ad0007', '20260401-007', '00000000-0000-0000-0000-0000000000c1', '00000000-0000-0000-0000-000000c20001', '00000000-0000-0000-0000-000000c00001', '00000000-0000-0000-0000-000000c10001', '00000000-0000-0000-0000-00000000d003', 'FINAL_COMPLETED', 'P2', '브레이크 패드 교체', '제동 성능 저하', 'COMPLETED', false, true, '00000000-0000-0000-0000-0000000000a1', date_trunc('month', now()) - interval '3 months' + interval '7 days', date_trunc('month', now()) - interval '3 months' + interval '11 days'),
  ('00000000-0000-0000-0000-000000ad0008', '20260301-008', '00000000-0000-0000-0000-0000000000c1', '00000000-0000-0000-0000-000000c20002', '00000000-0000-0000-0000-000000c00001', '00000000-0000-0000-0000-000000c10001', '00000000-0000-0000-0000-00000000d003', 'FINAL_COMPLETED', 'P3', '전조등 교체', '야간 작업등 불량', 'COMPLETED', false, true, '00000000-0000-0000-0000-0000000000a1', date_trunc('month', now()) - interval '4 months' + interval '7 days', date_trunc('month', now()) - interval '4 months' + interval '9 days 12 hours'),
  ('00000000-0000-0000-0000-000000ad0009', '20260201-009', '00000000-0000-0000-0000-0000000000c1', '00000000-0000-0000-0000-000000c20001', '00000000-0000-0000-0000-000000c00001', '00000000-0000-0000-0000-000000c10001', '00000000-0000-0000-0000-00000000d003', 'FINAL_COMPLETED', 'P2', '유압 호스 교체', '작동유 누유', 'COMPLETED', false, true, '00000000-0000-0000-0000-0000000000a1', date_trunc('month', now()) - interval '5 months' + interval '7 days', date_trunc('month', now()) - interval '5 months' + interval '11 days')
ON CONFLICT (id) DO NOTHING;

-- EXECUTIVE completion approvals: ad0001 this month, ad0005..ad0009 one per
-- trailing month. approver = 이대표 (d004, EXECUTIVE).
INSERT INTO work_order_approval_steps (id, org_id, work_order_id, step_order, role, approver_id, status, requested_at, approved_at, approved_by_id) VALUES
  ('00000000-0000-0000-0000-000000af0001', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000ad0001', 1, 'EXECUTIVE', '00000000-0000-0000-0000-00000000d004', 'APPROVED', now() - interval '2 days', now() - interval '1 day', '00000000-0000-0000-0000-00000000d004'),
  ('00000000-0000-0000-0000-000000af0005', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000ad0005', 1, 'EXECUTIVE', '00000000-0000-0000-0000-00000000d004', 'APPROVED', date_trunc('month', now()) - interval '1 month' + interval '9 days', date_trunc('month', now()) - interval '1 month' + interval '10 days', '00000000-0000-0000-0000-00000000d004'),
  ('00000000-0000-0000-0000-000000af0006', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000ad0006', 1, 'EXECUTIVE', '00000000-0000-0000-0000-00000000d004', 'APPROVED', date_trunc('month', now()) - interval '2 months' + interval '9 days', date_trunc('month', now()) - interval '2 months' + interval '10 days', '00000000-0000-0000-0000-00000000d004'),
  ('00000000-0000-0000-0000-000000af0007', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000ad0007', 1, 'EXECUTIVE', '00000000-0000-0000-0000-00000000d004', 'APPROVED', date_trunc('month', now()) - interval '3 months' + interval '9 days', date_trunc('month', now()) - interval '3 months' + interval '10 days', '00000000-0000-0000-0000-00000000d004'),
  ('00000000-0000-0000-0000-000000af0008', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000ad0008', 1, 'EXECUTIVE', '00000000-0000-0000-0000-00000000d004', 'APPROVED', date_trunc('month', now()) - interval '4 months' + interval '9 days', date_trunc('month', now()) - interval '4 months' + interval '10 days', '00000000-0000-0000-0000-00000000d004'),
  ('00000000-0000-0000-0000-000000af0009', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000ad0009', 1, 'EXECUTIVE', '00000000-0000-0000-0000-00000000d004', 'APPROVED', date_trunc('month', now()) - interval '5 months' + interval '9 days', date_trunc('month', now()) - interval '5 months' + interval '10 days', '00000000-0000-0000-0000-00000000d004')
ON CONFLICT (id) DO NOTHING;

-- ── dashboard trend depth (r9): the six completions above give exactly ONE
-- approval per trailing month — a FLAT all-1s series the 완료 추이 panel draws
-- as a degenerate sparkline, and a current-month stat strip stuck at 1건. Add
-- EXTRA completions so the per-month count VARIES (2→5) and route ~1/3 to
-- 부산 지점 (c2) so the 범위별 완료 bars differ per scope instead of reading an
-- identical 1건. Same real completion write shape: FINAL_COMPLETED + an
-- EXECUTIVE approval APPROVED within the month (the KPI rollup's count key).
INSERT INTO registry_customers (id, branch_id, name, org_id) VALUES
  ('00000000-0000-0000-0000-000000c00002', '00000000-0000-0000-0000-0000000000c2', '부산해운대물류㈜', '00000000-0000-0000-0000-0000000000a1')
ON CONFLICT (id) DO NOTHING;
INSERT INTO registry_sites (id, branch_id, customer_id, name, org_id, address, province, city) VALUES
  ('00000000-0000-0000-0000-000000c10002', '00000000-0000-0000-0000-0000000000c2', '00000000-0000-0000-0000-000000c00002', '부산해운대물류 센터', '00000000-0000-0000-0000-0000000000a1', '부산 해운대구 센텀중앙로 55', '부산광역시', '부산광역시')
ON CONFLICT (id) DO NOTHING;
INSERT INTO registry_equipment (id, branch_id, customer_id, site_id, equipment_no, manufacturer_code, kind_code, power_code, status, specification, ton_text, source_sheet, source_row, org_id) VALUES
  ('00000000-0000-0000-0000-000000c20003', '00000000-0000-0000-0000-0000000000c2', '00000000-0000-0000-0000-000000c00002', '00000000-0000-0000-0000-000000c10002', 'KNLFL-0003', 'CLARK', 'FORKLIFT', 'DIESEL', '임대', '디젤 지게차 3.0톤', '3.0톤', '장비대장', 4, '00000000-0000-0000-0000-0000000000a1')
ON CONFLICT (id) DO NOTHING;

-- Extra completed work orders: extra[ago] = {5:1,4:2,3:2,2:3,1:4,0:3}; combined
-- with the one existing completion per month the series becomes 2,3,3,4,5,4.
-- r12: created_at anchored 3 days before the matching approval's anchor date
-- (same 미지정 fix as above) so these rows count toward average_completion_seconds
-- too, not just the 6 concrete ad000N rows.
INSERT INTO work_orders (id, request_no, branch_id, equipment_id, customer_id, site_id, requested_by, status, priority, symptom, customer_request, result_type, kpi_excluded, evidence_verified, org_id, created_at)
SELECT
  ('00000000-0000-0000-0000-e5' || lpad((p.ago * 100 + g.n)::text, 10, '0'))::uuid,
  to_char(date_trunc('month', now()) - (p.ago || ' months')::interval, 'YYYYMM') || '20-' || lpad((600 + p.ago * 20 + g.n)::text, 3, '0'),
  (CASE WHEN (p.ago + g.n) % 3 = 0 THEN '00000000-0000-0000-0000-0000000000c2' ELSE '00000000-0000-0000-0000-0000000000c1' END)::uuid,
  (CASE WHEN (p.ago + g.n) % 3 = 0 THEN '00000000-0000-0000-0000-000000c20003'
       WHEN g.n % 2 = 0 THEN '00000000-0000-0000-0000-000000c20002'
       ELSE '00000000-0000-0000-0000-000000c20001' END)::uuid,
  (CASE WHEN (p.ago + g.n) % 3 = 0 THEN '00000000-0000-0000-0000-000000c00002' ELSE '00000000-0000-0000-0000-000000c00001' END)::uuid,
  (CASE WHEN (p.ago + g.n) % 3 = 0 THEN '00000000-0000-0000-0000-000000c10002' ELSE '00000000-0000-0000-0000-000000c10001' END)::uuid,
  '00000000-0000-0000-0000-00000000d003'::uuid, 'FINAL_COMPLETED', 'P2', '정기 정비 완료', '정기 점검 및 소모품 교체', 'COMPLETED', false, true, '00000000-0000-0000-0000-0000000000a1'::uuid,
  LEAST(now() - interval '3 hours', date_trunc('month', now()) - (p.ago || ' months')::interval + interval '7 days' + (g.n || ' hours')::interval)
FROM (VALUES (5, 1), (4, 2), (3, 2), (2, 3), (1, 4), (0, 3)) AS p(ago, extra),
     LATERAL generate_series(1, p.extra) AS g(n)
ON CONFLICT (id) DO NOTHING;

INSERT INTO work_order_approval_steps (id, org_id, work_order_id, step_order, role, approver_id, status, requested_at, approved_at, approved_by_id)
SELECT
  ('00000000-0000-0000-0000-e6' || lpad((p.ago * 100 + g.n)::text, 10, '0'))::uuid,
  '00000000-0000-0000-0000-0000000000a1'::uuid,
  ('00000000-0000-0000-0000-e5' || lpad((p.ago * 100 + g.n)::text, 10, '0'))::uuid,
  1, 'EXECUTIVE', '00000000-0000-0000-0000-00000000d004'::uuid, 'APPROVED',
  LEAST(now() - interval '2 hours', date_trunc('month', now()) - (p.ago || ' months')::interval + interval '9 days' + (g.n || ' hours')::interval),
  LEAST(now() - interval '1 hour', date_trunc('month', now()) - (p.ago || ' months')::interval + interval '10 days' + (g.n || ' hours')::interval),
  '00000000-0000-0000-0000-00000000d004'::uuid
FROM (VALUES (5, 1), (4, 2), (3, 2), (2, 3), (1, 4), (0, 3)) AS p(ago, extra),
     LATERAL generate_series(1, p.extra) AS g(n)
ON CONFLICT (id) DO NOTHING;

-- ── dashboard 재방문율 / 지연율 depth (r14): every seeded completion carried
-- result_type='COMPLETED' and a NULL delay_reason, so the KPI rollup's
-- revisit_count and delay_count were both 0 → 재방문율 0% / 지연율 0% on the
-- board. Seed two approved completions (current + prior month) that carry the
-- real revisit/delay signal the rollup counts: result_type='REVISIT_REQUIRED'
-- (revisit_count) + a non-null delay_reason (delay_count). Approved within the
-- month by 이대표 (d004, EXECUTIVE) — the same completion-approval key the
-- rollup joins on — so both rates read a real non-zero value per period.
INSERT INTO work_orders (
  id, request_no, branch_id, equipment_id, customer_id, site_id, requested_by, status, priority,
  symptom, customer_request, result_type, delay_reason, kpi_excluded, evidence_verified, org_id, created_at, target_due_at
) VALUES
  ('00000000-0000-0000-0000-000000ad00e1', to_char(date_trunc('month', now()), 'YYYYMM') || '15-901', '00000000-0000-0000-0000-0000000000c1', '00000000-0000-0000-0000-000000c20001', '00000000-0000-0000-0000-000000c00001', '00000000-0000-0000-0000-000000c10001', '00000000-0000-0000-0000-00000000d003', 'FINAL_COMPLETED', 'P2', '재점검 필요 — 추가 결함 발견', '동일 증상 재발로 재방문 필요', 'REVISIT_REQUIRED', 'ADDITIONAL_FAULT_FOUND', false, true, '00000000-0000-0000-0000-0000000000a1', date_trunc('month', now()) + interval '5 days', date_trunc('month', now()) + interval '6 days'),
  ('00000000-0000-0000-0000-000000ad00e2', to_char(date_trunc('month', now()) - interval '1 month', 'YYYYMM') || '15-902', '00000000-0000-0000-0000-0000000000c1', '00000000-0000-0000-0000-000000c20002', '00000000-0000-0000-0000-000000c00001', '00000000-0000-0000-0000-000000c10001', '00000000-0000-0000-0000-00000000d003', 'FINAL_COMPLETED', 'P3', '부품 대기로 지연 — 재방문', '부품 미입고로 재방문 처리', 'REVISIT_REQUIRED', 'PART_WAITING', false, true, '00000000-0000-0000-0000-0000000000a1', date_trunc('month', now()) - interval '1 month' + interval '5 days', date_trunc('month', now()) - interval '1 month' + interval '4 days')
ON CONFLICT (id) DO NOTHING;

INSERT INTO work_order_approval_steps (id, org_id, work_order_id, step_order, role, approver_id, status, requested_at, approved_at, approved_by_id) VALUES
  ('00000000-0000-0000-0000-000000af00e1', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000ad00e1', 1, 'EXECUTIVE', '00000000-0000-0000-0000-00000000d004', 'APPROVED', date_trunc('month', now()) + interval '6 days', LEAST(now() - interval '1 hour', date_trunc('month', now()) + interval '7 days'), '00000000-0000-0000-0000-00000000d004'),
  ('00000000-0000-0000-0000-000000af00e2', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000ad00e2', 1, 'EXECUTIVE', '00000000-0000-0000-0000-00000000d004', 'APPROVED', date_trunc('month', now()) - interval '1 month' + interval '6 days', date_trunc('month', now()) - interval '1 month' + interval '7 days', '00000000-0000-0000-0000-00000000d004')
ON CONFLICT (id) DO NOTHING;

-- ── dashboard KPI depth r12: 응답 속도 / 점검 계획 이행률 / P1 수락률 were
-- 미지정 (no qualifying source rows at all, not just zero) — fill each with a
-- real write shape so the stat strip stops reading "미지정" for these three.

-- 평균 응답 속도: an IN_PROGRESS status-history row between created_at and the
-- completion approval gives the rollup a first_in_progress_at to diff against
-- created_at (backend: work_order_status_history). One per concrete completed
-- work order is enough to make response_count > 0 in every rolling window.
INSERT INTO work_order_status_history (id, org_id, work_order_id, actor, action, from_status, to_status, occurred_at) VALUES
  ('00000000-0000-0000-0000-000000ee0001', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000ad0001', '00000000-0000-0000-0000-00000000d002', 'START_WORK', 'ASSIGNED', 'IN_PROGRESS', now() - interval '3 days 12 hours'),
  ('00000000-0000-0000-0000-000000ee0005', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000ad0005', '00000000-0000-0000-0000-00000000d002', 'START_WORK', 'ASSIGNED', 'IN_PROGRESS', date_trunc('month', now()) - interval '1 month' + interval '7 days 6 hours'),
  ('00000000-0000-0000-0000-000000ee0006', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000ad0006', '00000000-0000-0000-0000-00000000d002', 'START_WORK', 'ASSIGNED', 'IN_PROGRESS', date_trunc('month', now()) - interval '2 months' + interval '7 days 6 hours'),
  ('00000000-0000-0000-0000-000000ee0007', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000ad0007', '00000000-0000-0000-0000-00000000d002', 'START_WORK', 'ASSIGNED', 'IN_PROGRESS', date_trunc('month', now()) - interval '3 months' + interval '7 days 6 hours'),
  ('00000000-0000-0000-0000-000000ee0008', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000ad0008', '00000000-0000-0000-0000-00000000d002', 'START_WORK', 'ASSIGNED', 'IN_PROGRESS', date_trunc('month', now()) - interval '4 months' + interval '7 days 6 hours'),
  ('00000000-0000-0000-0000-000000ee0009', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000ad0009', '00000000-0000-0000-0000-00000000d002', 'START_WORK', 'ASSIGNED', 'IN_PROGRESS', date_trunc('month', now()) - interval '5 months' + interval '7 days 6 hours')
ON CONFLICT (id) DO NOTHING;

-- 점검 계획 이행률: regular_inspection_schedules due THIS calendar month — 3
-- COMPLETED + 2 still SCHEDULED gives a real 60% rather than 미지정 (zero rows
-- due at all) or a degenerate 100%.
INSERT INTO regular_inspection_schedules (
  id, org_id, branch_id, equipment_id, mechanic_id, cycle, interval_days, due_date, status,
  completed_at, completed_by, note, created_by, created_at
) VALUES
  ('00000000-0000-0000-0000-000000ef2001', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-0000000000c1', '00000000-0000-0000-0000-000000c20001', '00000000-0000-0000-0000-00000000d002', 'MONTHLY', 30, date_trunc('month', now())::date + 4, 'COMPLETED', date_trunc('month', now()) + interval '4 days 3 hours', '00000000-0000-0000-0000-00000000d002', '월간 정기점검 완료', '00000000-0000-0000-0000-00000000d001', date_trunc('month', now()) + interval '4 days'),
  ('00000000-0000-0000-0000-000000ef2002', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-0000000000c1', '00000000-0000-0000-0000-000000c20002', '00000000-0000-0000-0000-00000000d002', 'MONTHLY', 30, date_trunc('month', now())::date + 6, 'COMPLETED', date_trunc('month', now()) + interval '6 days 2 hours', '00000000-0000-0000-0000-00000000d002', '월간 정기점검 완료', '00000000-0000-0000-0000-00000000d001', date_trunc('month', now()) + interval '6 days'),
  ('00000000-0000-0000-0000-000000ef2003', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-0000000000c2', '00000000-0000-0000-0000-000000c20003', '00000000-0000-0000-0000-00000000d003', 'MONTHLY', 30, date_trunc('month', now())::date + 8, 'COMPLETED', date_trunc('month', now()) + interval '8 days 5 hours', '00000000-0000-0000-0000-00000000d003', '월간 정기점검 완료', '00000000-0000-0000-0000-00000000d001', date_trunc('month', now()) + interval '8 days'),
  ('00000000-0000-0000-0000-000000ef2004', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-0000000000c1', '00000000-0000-0000-0000-000000c20001', '00000000-0000-0000-0000-00000000d002', 'MONTHLY', 30, date_trunc('month', now())::date + 22, 'SCHEDULED', NULL, NULL, NULL, '00000000-0000-0000-0000-00000000d001', now()),
  ('00000000-0000-0000-0000-000000ef2005', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-0000000000c2', '00000000-0000-0000-0000-000000c20003', '00000000-0000-0000-0000-00000000d003', 'MONTHLY', 30, date_trunc('month', now())::date + 25, 'SCHEDULED', NULL, NULL, NULL, '00000000-0000-0000-0000-00000000d001', now())
ON CONFLICT (branch_id, equipment_id, due_date, cycle) DO NOTHING;

-- P1 수락률: two more P1 dispatches whose accept window opened this month, one
-- accepted by a technician and one auto-assigned — combined with the existing
-- BROADCASTING ea0001 (no response) the rollup reads a real ~67% instead of
-- 미지정/0%.
INSERT INTO p1_dispatches (id, org_id, work_order_id, branch_id, status, accept_window_started_at, accept_window_ends_at, auto_assigned_mechanic_id, created_by, created_at, updated_at) VALUES
  ('00000000-0000-0000-0000-000000ea0002', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000ad0005', '00000000-0000-0000-0000-0000000000c1', 'BROADCASTING', now() - interval '5 days', now() - interval '5 days' + interval '30 minutes', NULL, '00000000-0000-0000-0000-00000000d003', now() - interval '5 days', now() - interval '5 days' + interval '30 minutes'),
  ('00000000-0000-0000-0000-000000ea0003', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000ad0006', '00000000-0000-0000-0000-0000000000c1', 'AUTO_ASSIGNED', now() - interval '6 days', now() - interval '6 days' + interval '30 minutes', '00000000-0000-0000-0000-00000000d002', '00000000-0000-0000-0000-00000000d003', now() - interval '6 days', now() - interval '6 days' + interval '20 minutes')
ON CONFLICT (id) DO NOTHING;

INSERT INTO p1_dispatch_responses (id, org_id, dispatch_id, user_id, response, responded_at, workload_weight) VALUES
  ('00000000-0000-0000-0000-000000eb2001', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000ea0002', '00000000-0000-0000-0000-00000000d001', 'ACCEPT', now() - interval '5 days' + interval '4 minutes', 0)
ON CONFLICT (dispatch_id, user_id) DO NOTHING;

-- ── site attendance events (대시보드 사업장 커버리지 card) ─────────────────────
-- Business clock-in facts the hr/attendance-summary read groups per user into
-- arrivals/departures.
INSERT INTO site_attendance_events (id, org_id, user_id, branch_id, work_order_id, site_id, kind, occurred_at) VALUES
  ('00000000-0000-0000-0000-000000ae0001', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-00000000d002', '00000000-0000-0000-0000-0000000000c1', '00000000-0000-0000-0000-000000ad0001', '00000000-0000-0000-0000-000000c10001', 'ARRIVAL', now() - interval '1 day' - interval '8 hours'),
  ('00000000-0000-0000-0000-000000ae0002', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-00000000d002', '00000000-0000-0000-0000-0000000000c1', '00000000-0000-0000-0000-000000ad0001', '00000000-0000-0000-0000-000000c10001', 'DEPARTURE', now() - interval '1 day' - interval '1 hour'),
  ('00000000-0000-0000-0000-000000ae0003', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-00000000d003', '00000000-0000-0000-0000-0000000000c1', '00000000-0000-0000-0000-000000ad0002', '00000000-0000-0000-0000-000000c10001', 'ARRIVAL', now() - interval '2 days' - interval '8 hours'),
  ('00000000-0000-0000-0000-000000ae0004', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-00000000d003', '00000000-0000-0000-0000-0000000000c1', '00000000-0000-0000-0000-000000ad0002', '00000000-0000-0000-0000-000000c10001', 'DEPARTURE', now() - interval '2 days' - interval '1 hour'),
  ('00000000-0000-0000-0000-000000ae0005', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-00000000d002', '00000000-0000-0000-0000-0000000000c1', '00000000-0000-0000-0000-000000ad0001', '00000000-0000-0000-0000-000000c10001', 'ARRIVAL', now() - interval '8 hours')
ON CONFLICT (id, org_id) DO NOTHING;

-- ── 내 업무 (todos) + 개인 수신함 (inbox_docs) for d001 ─────────────────────
-- Backs the 개요/내 업무/개인 수신함 screens so a fresh dev-auth session (d001)
-- sees real rows instead of empty states. Deterministic ids + ON CONFLICT so
-- re-runs stay idempotent (file idiom).
INSERT INTO todos (id, org_id, owner_user_id, body, done, done_at) VALUES
  ('00000000-0000-0000-0000-000000f0d001', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-00000000d001', '리프트 3호기 유압 점검 일정 확정', false, NULL),
  ('00000000-0000-0000-0000-000000f0d002', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-00000000d001', '주간 안전점검 보고서 검토', true, now())
ON CONFLICT (id) DO NOTHING;

INSERT INTO inbox_docs (id, org_id, recipient_user_id, kind, title, payload) VALUES
  ('00000000-0000-0000-0000-000000f1d001', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-00000000d001', 'payslip', '2026년 6월 급여명세서', '{"기본급":"3,200,000","식대":"200,000","공제계":"420,000","실지급액":"2,980,000"}'::jsonb)
ON CONFLICT (id) DO NOTHING;

INSERT INTO inbox_docs (id, org_id, recipient_user_id, kind, notice_type, title, payload, legal_basis) VALUES
  ('00000000-0000-0000-0000-000000f1d002', '00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-00000000d001', 'legal_notice', '연차촉진', '2026년 연차휴가 사용촉진 통지', '{"paragraphs":["귀하의 미사용 연차 5일에 대해 근로기준법 제61조에 따라 사용을 촉진합니다.","사용 시기를 지정하여 회신하여 주시기 바랍니다."]}'::jsonb, '근로기준법 §61')
ON CONFLICT (id) DO NOTHING;

COMMIT;
