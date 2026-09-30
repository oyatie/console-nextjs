-- Eleven shipped features cannot be granted to any tenant role.
--
-- `policy_role_permissions.feature_key` FKs to `feature_catalog` (0065), so a feature with no
-- catalog row is unreachable through the tenant grant path no matter what the compile-time
-- matrix says about it. Eleven of the 96 keys in `Feature::as_str`
-- (backend/crates/platform/authz/src/lib.rs:472) have no row.
--
-- Measured rather than assumed: the arms of `as_str` were compared against every
-- `INSERT INTO feature_catalog` in the migration tree. An earlier count of 19 was wrong — it
-- derived keys by naive snake_case conversion of the variant names, which produces
-- `equipment3r_approve` where the real key is `equipment_3r_approve`. The serialization
-- function is the authority, not the variant name.
--
-- WHAT WAS UNGRANTABLE, and it is not a uniform set:
--
--   payroll_run_read, payroll_run_manage   Payroll has no reviewer-who-cannot-pay role,
--                                          because neither half of the read/manage split is
--                                          grantable. This is the separation-of-duties hole.
--   approval_finalize                      Feature::ApprovalFinalize permits ADMIN and above in
--                                          the matrix, and no tenant role can hold it.
--   attendance_exception_manage,           Attendance correction paths.
--   attendance_substitution_manage
--   exit_case_report, exit_case_hr_confirm,  The 퇴직 flow, in full.
--   exit_case_hq_confirm, exit_settlement_manage
--   notice_manage, production_source_ingest
--
-- This seeds the rows. It does NOT grant anything to anyone: `feature_catalog` is the set of
-- keys a grant may name, and `policy_role_permissions` remains empty of these until a tenant
-- deliberately grants them. Adding a row makes a feature expressible, not held.
--
-- THIRTEEN ORPHAN ROWS ARE LEFT ALONE, deliberately. `feature_catalog` also holds keys with no
-- corresponding `Feature` — evidence_*, governance_*, org_change_*. Deleting them would break
-- the FK for any existing grant that names one, and a row that no code reads is inert rather
-- than dangerous. The reverse direction is the one that silently removes capability, and that
-- is what this migration closes. `feature_catalog_covers_every_feature` (added alongside this)
-- asserts only that direction, for the same reason.

INSERT INTO feature_catalog (feature_key) VALUES
    ('approval_finalize'),
    ('attendance_exception_manage'),
    ('attendance_substitution_manage'),
    ('exit_case_hq_confirm'),
    ('exit_case_hr_confirm'),
    ('exit_case_report'),
    ('exit_settlement_manage'),
    ('notice_manage'),
    ('payroll_run_manage'),
    ('payroll_run_read'),
    ('production_source_ingest')
ON CONFLICT (feature_key) DO NOTHING;
