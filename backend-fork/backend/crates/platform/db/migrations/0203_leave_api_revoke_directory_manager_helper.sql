-- 0183 created this SECURITY DEFINER authorization predicate and set its owner
-- but issued no REVOKE, so it kept PostgreSQL's default EXECUTE TO PUBLIC while
-- every sibling in the same file was revoked. It is an internal helper:
-- `leave_api.create_employee` reaches it by PERFORM as the owning definer, so
-- no role needs a grant back.
REVOKE ALL ON FUNCTION leave_api.assert_employee_directory_manager(UUID, UUID)
    FROM PUBLIC, console_rt, console_leave_cmd;
