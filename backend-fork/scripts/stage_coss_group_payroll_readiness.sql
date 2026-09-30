-- G008 live staging for the governed COSS Group 2026-05 live import.
-- Source of truth is the append-only governed import ledger: data_import_runs + data_import_rows.
-- Payroll calculation remains blocked until an official NTS row and professional validation are attached.
-- This script creates review/staging objects only; it does not calculate, approve, issue, or send payroll.

WITH import_rows AS (
    SELECT
        r.id,
        r.org_id,
        r.run_id,
        run.source_filename,
        r.source_sheet,
        r.source_row,
        r.source_key,
        r.raw_row,
        r.canonical_row,
        COALESCE(NULLIF(r.canonical_row->>'source_key', ''), NULLIF(r.source_key, '')) AS canonical_source_key,
        -- Source-material flags test for a non-blank VALUE, not merely a present KEY.
        -- These were `raw_row ?| array[...]`, which is true when the spreadsheet
        -- merely HAS the column. Measured against a live PostgreSQL: a row whose
        -- 출근 cell was empty produced attendance_source_row_count = 1, exactly as
        -- if it were filled, so the close preflight's 근태 원천 확보 check was
        -- satisfied by a blank column. Provenance (#834) established the rows were
        -- applied; this establishes they say something.
        COALESCE(NULLIF(r.canonical_row->>'name', ''), NULLIF(r.raw_row->>'성명', ''), NULLIF(r.raw_row->>'성명_2', '')) AS imported_name,
        COALESCE(NULLIF(r.canonical_row->>'company', ''), NULLIF(r.raw_row->'__source'->>'company', ''), NULLIF(r.raw_row->>'소속', '')) AS imported_company,
        (jsonb_typeof(r.raw_row) = 'object' AND EXISTS (
            SELECT 1 FROM jsonb_each_text(r.raw_row) kv
             WHERE kv.key = ANY (array['기본시급','통상시급','공제총액','소득세','건강보험','건강/장기요양','고용보험','급여산정일','지급일','연차수당','상여금','은행','계좌','주민번호'])
               AND btrim(kv.value) <> ''
        )) AS is_payroll_source,
        (jsonb_typeof(r.raw_row) = 'object' AND EXISTS (
            SELECT 1 FROM jsonb_each_text(r.raw_row) kv
             WHERE kv.key = ANY (array['근무일자','출근','퇴근','근무시간','기본시간','기본근무','연장시간','심야시간','특근시간','특근연장시간','특근연장','근무일명칭','지각,조퇴시간'])
               AND btrim(kv.value) <> ''
        )) AS is_attendance_source,
        (jsonb_typeof(r.raw_row) = 'object' AND EXISTS (
            SELECT 1 FROM jsonb_each_text(r.raw_row) kv
             WHERE kv.key = ANY (array['발생연차','사용연차','잔여연차','연차수당'])
               AND btrim(kv.value) <> ''
        )) AS is_leave_source,
        (jsonb_typeof(r.raw_row) = 'object' AND EXISTS (
            SELECT 1 FROM jsonb_each_text(r.raw_row) kv
             WHERE kv.key = ANY (array['기본급','상여금','월계','합계','총합','연차수당'])
               AND btrim(kv.value) <> ''
        )) AS has_gross_pay_source,
        (jsonb_typeof(r.raw_row) = 'object' AND EXISTS (
            SELECT 1 FROM jsonb_each_text(r.raw_row) kv
             WHERE kv.key = ANY (array['차인지급액','실지급액','공제총액','소득세','건강보험','고용보험'])
               AND btrim(kv.value) <> ''
        )) AS has_net_pay_source
    FROM data_import_rows r
    JOIN data_import_runs run
      ON run.id = r.run_id
     AND run.org_id = r.org_id
    WHERE run.entity_type = 'employee_hr'
      AND run.source_filename LIKE '2026/5월/%'
      -- Provenance. `data_import_runs.status` admits PREVIEWED/DRY_RUN/APPLIED/
      -- FAILED and `data_import_rows.row_status` admits CANDIDATE/PRESERVED/
      -- ERROR (migration 0070). This filtered on NEITHER: the only thing keeping
      -- unapplied rows out was a human hand-typing one vetted filename above. A
      -- never-applied DRY_RUN, or a FAILED run's ERROR rows, would otherwise
      -- materialise a roster and count as payroll/attendance source material.
      AND run.status = 'APPLIED'
      AND r.row_status <> 'ERROR'
), import_orgs AS (
    SELECT
        org_id,
        count(DISTINCT run_id) AS import_run_count,
        count(*) AS source_row_count,
        count(*) FILTER (WHERE is_payroll_source) AS payroll_source_row_count,
        count(*) FILTER (WHERE is_attendance_source) AS attendance_source_row_count,
        count(*) FILTER (WHERE is_leave_source) AS leave_source_row_count
    FROM import_rows
    GROUP BY org_id
), upsert_runs AS (
    INSERT INTO payroll_draft_runs (
        org_id,
        period_start,
        period_end,
        source_label,
        status,
        calculation_enabled,
        legal_basis,
        source_summary
    )
    SELECT
        org_id,
        DATE '2026-05-01',
        DATE '2026-05-31',
        'COSS Group 2026-05 live import',
        'BLOCKED_LEGAL_GATE',
        FALSE,
        jsonb_build_object(
            'readiness_status', 'BLOCKED_LEGAL_GATE',
            'blocker', 'Payroll calculation remains blocked until an official NTS row and professional validation are attached',
            'official_source_requirements', jsonb_build_array(
                'NTS wage-income withholding row/version',
                'effective-dated 국민연금/NHIS/고용보험/산재보험 rate artifacts',
                'licensed labor/tax professional validation memo',
                'golden payroll cases before calculation_enabled can become true'
            )
        ),
        jsonb_build_object(
            'source', 'data_import_rows',
            'source_label', 'COSS Group 2026-05 live import',
            'import_run_count', import_run_count,
            'source_row_count', source_row_count,
            'payroll_source_row_count', payroll_source_row_count,
            'attendance_source_row_count', attendance_source_row_count,
            'leave_source_row_count', leave_source_row_count
        )
    FROM import_orgs
    ON CONFLICT (org_id, period_start, period_end, source_label) DO UPDATE SET
        status = 'BLOCKED_LEGAL_GATE',
        calculation_enabled = FALSE,
        legal_basis = EXCLUDED.legal_basis,
        source_summary = EXCLUDED.source_summary,
        updated_at = now()
    -- Re-running this script must never rewind a run that has left the
    -- pre-close states. Without this the DO UPDATE reset status to
    -- BLOCKED_LEGAL_GATE unconditionally, so a re-stage over an
    -- ATTENDANCE_CLOSED, CALCULATED, APPROVED, ISSUED or PAID run rewound it and
    -- rewrote its source_summary -- invalidating the close_receipt that attested
    -- it and desynchronising the append-only payroll_line_calculations from the
    -- lines they were computed from. The three admissible states are the same
    -- ones `CLOSEABLE` names in payroll/adapter-postgres/src/lifecycle.rs.
    --
    -- A run past those states is skipped entirely: it returns no row here, so
    -- `upsert_lines` joins nothing for it and its line evidence is left alone
    -- too. Doing nothing is the correct outcome; rewinding a paid run is not.
    WHERE payroll_draft_runs.status IN ('STAGED', 'BLOCKED_LEGAL_GATE', 'READY_FOR_REVIEW')
    RETURNING id, org_id
), employee_basis AS (
    SELECT
        e.org_id,
        e.id AS employee_id,
        COALESCE(NULLIF(e.source_key, ''), e.id::text) AS employee_source_key,
        e.name AS employee_display_name,
        COALESCE(NULLIF(e.company, ''), 'UNKNOWN_COMPANY') AS employee_company,
        e.leave_accrued,
        e.leave_used,
        e.leave_remaining
    FROM employees e
    WHERE EXISTS (SELECT 1 FROM import_orgs io WHERE io.org_id = e.org_id)
), row_metrics AS (
    SELECT
        ir.*,
        CASE WHEN btrim(COALESCE(ir.raw_row->>'근무일수', ir.raw_row->>'근무일', '')) ~ '^-?[0-9]+([.]?[0-9]+)?$'
             THEN btrim(COALESCE(ir.raw_row->>'근무일수', ir.raw_row->>'근무일'))::numeric END AS work_days_value,
        CASE WHEN btrim(COALESCE(ir.raw_row->>'근무시간', ir.raw_row->>'기본시간', ir.raw_row->>'기본근무', '')) ~ '^-?[0-9]+([.]?[0-9]+)?$'
             THEN btrim(COALESCE(ir.raw_row->>'근무시간', ir.raw_row->>'기본시간', ir.raw_row->>'기본근무'))::numeric END AS regular_hours_value,
        CASE WHEN btrim(COALESCE(ir.raw_row->>'연장시간', ir.raw_row->>'특근연장시간', ir.raw_row->>'특근연장', '')) ~ '^-?[0-9]+([.]?[0-9]+)?$'
             THEN btrim(COALESCE(ir.raw_row->>'연장시간', ir.raw_row->>'특근연장시간', ir.raw_row->>'특근연장'))::numeric END AS overtime_hours_value,
        CASE WHEN btrim(COALESCE(ir.raw_row->>'심야시간', '')) ~ '^-?[0-9]+([.]?[0-9]+)?$'
             THEN btrim(ir.raw_row->>'심야시간')::numeric END AS night_hours_value,
        CASE WHEN btrim(COALESCE(ir.raw_row->>'특근시간', '')) ~ '^-?[0-9]+([.]?[0-9]+)?$'
             THEN btrim(ir.raw_row->>'특근시간')::numeric END AS holiday_hours_value,
        CASE WHEN btrim(COALESCE(ir.raw_row->>'사용연차', '')) ~ '^-?[0-9]+([.]?[0-9]+)?$'
             THEN btrim(ir.raw_row->>'사용연차')::numeric END AS leave_used_value,
        CASE WHEN btrim(COALESCE(ir.raw_row->>'잔여연차', '')) ~ '^-?[0-9]+([.]?[0-9]+)?$'
             THEN btrim(ir.raw_row->>'잔여연차')::numeric END AS leave_remaining_value
    FROM import_rows ir
), employee_metrics AS (
    SELECT
        eb.org_id,
        eb.employee_id,
        eb.employee_source_key,
        eb.employee_display_name,
        eb.employee_company,
        eb.leave_accrued,
        count(rm.id) FILTER (WHERE rm.is_payroll_source) AS payroll_source_row_count,
        count(rm.id) FILTER (WHERE rm.is_attendance_source) AS attendance_source_row_count,
        COALESCE(sum(rm.work_days_value), 0) AS work_days,
        COALESCE(sum(rm.regular_hours_value), 0) AS regular_hours,
        COALESCE(sum(rm.overtime_hours_value), 0) AS overtime_hours,
        COALESCE(sum(rm.night_hours_value), 0) AS night_hours,
        COALESCE(sum(rm.holiday_hours_value), 0) AS holiday_hours,
        COALESCE(max(rm.leave_used_value), eb.leave_used) AS leave_used,
        COALESCE(max(rm.leave_remaining_value), eb.leave_remaining) AS leave_remaining,
        bool_or(COALESCE(rm.has_gross_pay_source, FALSE)) AS gross_pay_source_present,
        bool_or(COALESCE(rm.has_net_pay_source, FALSE)) AS net_pay_source_present,
        COALESCE(array_agg(rm.id ORDER BY rm.source_filename, rm.source_sheet, rm.source_row) FILTER (WHERE rm.id IS NOT NULL), ARRAY[]::uuid[]) AS source_data_import_row_ids
    FROM employee_basis eb
    LEFT JOIN row_metrics rm
      ON rm.org_id = eb.org_id
     AND rm.canonical_source_key = eb.employee_source_key
    GROUP BY
        eb.org_id,
        eb.employee_id,
        eb.employee_source_key,
        eb.employee_display_name,
        eb.employee_company,
        eb.leave_accrued,
        eb.leave_used,
        eb.leave_remaining
), upsert_lines AS (
    INSERT INTO payroll_draft_lines (
        org_id,
        run_id,
        employee_id,
        employee_source_key,
        employee_display_name,
        employee_company,
        payroll_source_row_count,
        attendance_source_row_count,
        attendance_event_count,
        work_days,
        regular_hours,
        overtime_hours,
        night_hours,
        holiday_hours,
        leave_used,
        leave_remaining,
        gross_pay_source_present,
        net_pay_source_present,
        nts_tax_row_status,
        calculation_status,
        blockers,
        source_data_import_row_ids
    )
    SELECT
        em.org_id,
        ur.id,
        em.employee_id,
        em.employee_source_key,
        em.employee_display_name,
        em.employee_company,
        em.payroll_source_row_count::integer,
        em.attendance_source_row_count::integer,
        0,
        NULLIF(em.work_days, 0),
        NULLIF(em.regular_hours, 0),
        NULLIF(em.overtime_hours, 0),
        NULLIF(em.night_hours, 0),
        NULLIF(em.holiday_hours, 0),
        em.leave_used,
        em.leave_remaining,
        em.gross_pay_source_present,
        em.net_pay_source_present,
        'REQUIRED_NOT_SUPPLIED',
        'BLOCKED_LEGAL_GATE',
        jsonb_build_array(
            'Payroll calculation remains blocked until an official NTS row and professional validation are attached',
            'HR must review source rows, leave balances, employment status, and statutory insurance applicability before approval',
            'Wage-statement issuance requires approved payroll run, passkey step-up, and immutable audit evidence'
        ),
        em.source_data_import_row_ids
    FROM employee_metrics em
    JOIN upsert_runs ur ON ur.org_id = em.org_id
    WHERE em.payroll_source_row_count > 0
       OR em.attendance_source_row_count > 0
       OR COALESCE(em.leave_remaining, 0) > 0
    ON CONFLICT (org_id, run_id, employee_source_key) DO UPDATE SET
        employee_id = EXCLUDED.employee_id,
        employee_display_name = EXCLUDED.employee_display_name,
        employee_company = EXCLUDED.employee_company,
        payroll_source_row_count = EXCLUDED.payroll_source_row_count,
        attendance_source_row_count = EXCLUDED.attendance_source_row_count,
        attendance_event_count = EXCLUDED.attendance_event_count,
        work_days = EXCLUDED.work_days,
        regular_hours = EXCLUDED.regular_hours,
        overtime_hours = EXCLUDED.overtime_hours,
        night_hours = EXCLUDED.night_hours,
        holiday_hours = EXCLUDED.holiday_hours,
        leave_used = EXCLUDED.leave_used,
        leave_remaining = EXCLUDED.leave_remaining,
        gross_pay_source_present = EXCLUDED.gross_pay_source_present,
        net_pay_source_present = EXCLUDED.net_pay_source_present,
        nts_tax_row_status = 'REQUIRED_NOT_SUPPLIED',
        calculation_status = 'BLOCKED_LEGAL_GATE',
        blockers = EXCLUDED.blockers,
        source_data_import_row_ids = EXCLUDED.source_data_import_row_ids,
        updated_at = now()
    RETURNING org_id, employee_id, leave_used, leave_remaining
), upsert_leave AS (
    INSERT INTO annual_leave_obligations (
        org_id,
        employee_id,
        leave_year,
        leave_accrued,
        leave_used,
        leave_remaining,
        status,
        statutory_basis,
        notification_plan
    )
    SELECT
        em.org_id,
        em.employee_id,
        2026,
        em.leave_accrued,
        em.leave_used,
        em.leave_remaining,
        CASE
            WHEN COALESCE(em.leave_remaining, 0) > 0 THEN 'USAGE_PROMOTION_DRAFT_REQUIRED'
            ELSE 'NEEDS_HR_REVIEW'
        END,
        jsonb_build_object(
            'source_label', 'COSS Group 2026-05 live import',
            'basis', 'annual-leave usage-promotion workflow requires HR legal review before notices are marked complete'
        ),
        jsonb_build_object(
            'channel_policy', 'messenger/mail/workflow notification is a workflow object',
            'send_allowed', FALSE,
            'blocker', 'HR review and statutory notice evidence are required before sending or closing annual-leave obligations'
        )
    FROM employee_metrics em
    WHERE em.employee_id IS NOT NULL
      AND (em.leave_accrued IS NOT NULL OR em.leave_used IS NOT NULL OR em.leave_remaining IS NOT NULL)
    ON CONFLICT (org_id, employee_id, leave_year) DO UPDATE SET
        leave_accrued = EXCLUDED.leave_accrued,
        leave_used = EXCLUDED.leave_used,
        leave_remaining = EXCLUDED.leave_remaining,
        status = EXCLUDED.status,
        statutory_basis = EXCLUDED.statutory_basis,
        notification_plan = EXCLUDED.notification_plan,
        updated_at = now()
    RETURNING org_id, employee_id
), line_counts AS (
    SELECT org_id, count(*) AS draft_line_count
    FROM upsert_lines
    GROUP BY org_id
), leave_counts AS (
    SELECT org_id, count(*) AS annual_leave_obligation_count
    FROM upsert_leave
    GROUP BY org_id
), audit_stage AS (
    INSERT INTO audit_events (
        id,
        actor,
        action,
        target_type,
        target_id,
        branch_id,
        before_snap,
        trace_id,
        span_id,
        after_snap,
        occurred_at,
        org_id
    )
    SELECT
        gen_random_uuid(),
        NULL,
        'data_import.payroll_readiness_stage',
        'payroll_readiness_stage',
        ur.org_id::text,
        NULL,
        NULL,
        substr(md5(gen_random_uuid()::text || clock_timestamp()::text), 1, 32),
        substr(md5(gen_random_uuid()::text), 1, 16),
        jsonb_build_object(
            'source_label', 'COSS Group 2026-05 live import',
            'source', 'data_import_rows',
            'draft_run_count', 1,
            'draft_line_count', COALESCE(lc.draft_line_count, 0),
            'annual_leave_obligation_count', COALESCE(ac.annual_leave_obligation_count, 0),
            'status', 'BLOCKED_LEGAL_GATE',
            'calculation_enabled', FALSE,
            'blocker', 'Payroll calculation remains blocked until an official NTS row and professional validation are attached'
        ),
        now(),
        ur.org_id
    FROM upsert_runs ur
    LEFT JOIN line_counts lc ON lc.org_id = ur.org_id
    LEFT JOIN leave_counts ac ON ac.org_id = ur.org_id
    WHERE NOT EXISTS (
        SELECT 1
        FROM audit_events ae
        WHERE ae.org_id = ur.org_id
          AND ae.action = 'data_import.payroll_readiness_stage'
          AND ae.target_type = 'payroll_readiness_stage'
          AND ae.target_id = ur.org_id::text
          AND ae.after_snap->>'source_label' = 'COSS Group 2026-05 live import'
    )
    RETURNING org_id
)
SELECT
    'payroll_readiness_stage' AS result,
    (SELECT count(*) FROM upsert_runs) AS draft_runs,
    (SELECT count(*) FROM upsert_lines) AS draft_lines,
    (SELECT count(*) FROM upsert_leave) AS annual_leave_obligations,
    (SELECT count(*) FROM audit_stage) AS audit_events_inserted;
