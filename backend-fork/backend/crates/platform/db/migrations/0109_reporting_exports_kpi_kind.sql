-- Allow KPI workbook exports to be recorded in excel_export_logs alongside
-- daily_status and work_diary. KPI downloads are audited exactly like the
-- sibling exports (record_export_log -> excel_export_logs + audit_events).
ALTER TABLE excel_export_logs
    DROP CONSTRAINT excel_export_logs_export_kind_check,
    ADD CONSTRAINT excel_export_logs_export_kind_check
        CHECK (export_kind IN ('daily_status', 'work_diary', 'kpi'));
