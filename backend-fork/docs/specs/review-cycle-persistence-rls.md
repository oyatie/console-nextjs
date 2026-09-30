> **QUARRY / NON-AUTHORITY.** Idea or draft only. Cannot dispatch work, clear HOLDs, or override product scope. Current authority: repository README + [`docs/current/PRODUCT.md`](../current/PRODUCT.md) / ROADMAP / DELIVERY.

# Review-cycle persistence and RLS design

Status: implementation-ready persistence/RLS design note for Kanban `t_c402ad93` / GitHub issue #309.

Sources:
- Parent validation handoff: `.omc/handoffs/t_9684b60c-review-cycle-validation.md`.
- Prototype backend gap register: `.omc/research/oyatie/prototype-anatomy/04-backend-contract.md` lines 89-100, 245-247, and 276.
- Prototype review screen: `docs/design/oyatie-console/Oyatie Console.dc.html` lines 952-986 and 7598-7610.
- Existing backend conventions: clean-arch crates under `backend/crates/<domain>/*`, RLS policy shape in migrations, `with_org_conn` / `with_audit` / `with_audits` in `backend/crates/platform/db/src/audit_tx.rs`, and tenant-isolation gate expectations in `backend/ci/gates/tenant-isolation/src/lib.rs`.

Non-goals:
- No recruit, benefit, payroll, attendance-exception, or generic HRX mega-domain in this slice.
- No Excel/import-first evaluation workflow. CRUD/review tasks are primary; import can only be later bootstrap tooling.
- No migration-number reservation from this design card. Pick the next free migration number only immediately before the implementation PR is ready to merge.
- No REST/OpenAPI/generated-client edit in this design card; the REST child owns that serialized work.

## 1. Domain boundary and crate choice

Create a dedicated review domain root:

```text
backend/crates/review/domain/
backend/crates/review/application/
backend/crates/review/adapter-postgres/
backend/crates/review/rest/
```

Rust packages should follow the workspace naming convention:

```text
console-review-domain
console-review-application
console-review-adapter-postgres
console-review-rest
```

Rationale:
- The repo has no existing `backend/crates/hr*` owner, and adjacent gaps are being designed as their own thin domain crates.
- A narrow `review` root keeps UI-M12's bespoke review-cycle FSM from absorbing recruit/benefit/payroll concerns.
- The layer-boundary gate remains simple: domain depends only on `console-kernel-core` and `serde`; application depends on domain/kernel; adapter depends on application/domain plus `console-platform-db` and request context; rest depends on application/adapter/auth/authz.

Workspace wiring when implemented:
- `backend/Cargo.toml` already uses explicit domain member globs; add `"crates/review/*"` if not covered on the implementation branch.
- Add workspace dependencies for `console-review-domain`, `console-review-application`, `console-review-adapter-postgres`, and `console-review-rest`.
- Add kernel ID newtypes in `backend/crates/kernel/core/src/ids.rs`: `ReviewCycleId`, `ReviewCycleTeamId`, `ReviewTaskId`, `ReviewScoreEntryId`, `ReviewCommentId`, and `ReviewEventId`.

## 2. Domain model

### 2.1 Review cycle

`ReviewCycle` is the tenant-owned top-level object for a single evaluation cycle.

Core fields:
- `id: ReviewCycleId`
- `cycle_code: ReviewCycleCode` such as `RV-0001`
- `title: NonEmptyText`
- `cycle_kind: probation | annual | project | ad_hoc | calibration`
- `status: draft | scheduled | open | closed | cancelled`
- `period_start`, `period_end`, `due_at`
- `score_schema: ReviewScoreSchema`
- `comment_schema: ReviewCommentSchema`
- audit metadata: `created_by`, `updated_by`, timestamps

Status rules belong in the review domain/application layer, not hidden SQL updates:

```text
draft -> scheduled -> open -> closed
scheduled -> cancelled
open -> cancelled
closed/cancelled are terminal
```

The FSM/audited-mutation child may refine this graph, but storage must be able to prove each transition and reject skipped transitions.

### 2.2 Review cycle team

`ReviewCycleTeam` is a cycle-local grouping used to render prototype `rvTeams` team progress. Because the current backend has employees with `org_unit`/`position` and users with `team`, but no durable team table, the review domain stores a snapshot key/label for the grouping rather than inventing a global teams table.

Core fields:
- `id: ReviewCycleTeamId`
- `cycle_id: ReviewCycleId`
- optional `branch_id` / `site_id` to narrow operational scope
- `team_ref_kind: user_team | employee_org_unit | freeform_snapshot`
- `team_ref: String` for the source value, e.g. `관리`, `정비사업팀`, or a future org-unit key
- `display_label: String` for UI labels such as `경영지원팀`
- `display_order: i32`
- `target_task_count: Option<i32>` for planned denominator when tasks are generated later; actual progress is computed from `review_tasks`

Team progress should be a read model computed as:

```text
completed = count(review_tasks where status in submitted/accepted)
total = count(review_tasks where status not cancelled)
pct = completed / total, or 0 when total = 0 unless target_task_count is explicitly set
```

Do not store mutable percentage columns as source of truth. If the first implementation needs list performance, materialize a cache only as an adapter-owned projection with invalidation in the same audited transaction; do not expose it as authority.

### 2.3 Review task

`ReviewTask` is the per-person task surfaced by prototype `rvTasks` and by the later review-writing screen.

Core fields:
- `id: ReviewTaskId`
- `cycle_id: ReviewCycleId`
- `team_id: Option<ReviewCycleTeamId>`
- `subject_employee_id: Option<EmployeeId>`: employee being reviewed
- `assigned_to_user_id: UserId`: person who must complete the task
- `reviewer_role: self | manager | peer | hr | calibration_panel | executive`
- `task_kind: self_review | manager_review | peer_review | probation_review | calibration | approval`
- `title: String`
- `status: open | in_progress | submitted | accepted | returned | cancelled`
- `due_at`, `started_at`, `submitted_at`, `accepted_at`, `returned_at`, `cancelled_at`
- `completed_by: Option<UserId>`; set only for submitted/accepted terminal-enough states
- `idempotency_key` for generated task creation
- audit metadata and timestamps

Task status rules:

```text
open -> in_progress -> submitted -> accepted
submitted -> returned -> in_progress
open/in_progress/submitted/returned -> cancelled
accepted/cancelled are terminal for the assigned task
```

Assigned-to checks must use the authenticated principal. Clients may identify a task id but may not claim `assigned_to_user_id`, `completed_by`, or `org_id` authority.

### 2.4 Score and comment structure

The prototype currently renders only team progress plus task rows, but issue #309 requires the score/comment structure consumed by the review-writing screen. Use relational rows for queryable score criteria and comments; keep bounded JSON only for low-risk display metadata.

`ReviewScoreEntry`:
- `id: ReviewScoreEntryId`
- `task_id: ReviewTaskId`
- `criterion_key: String` matching `^[a-z][a-z0-9_]{1,63}$`
- `criterion_label: String`
- `score_value: numeric(6,2)` nullable until draft save/submission
- `score_min: numeric(6,2)` default 0
- `score_max: numeric(6,2)` default 5
- `weight_bps: int` default 0, 0..=10000
- `display_order: int`
- `evidence_refs: jsonb array` for object codes/links only, not raw PII blobs
- `updated_by`, timestamps

`ReviewComment`:
- `id: ReviewCommentId`
- `task_id: ReviewTaskId`
- `comment_kind: overall | strength | improvement | private_note | calibration | return_reason`
- `visibility: subject | reviewer_group | hr_only | audit_only`
- `body: text` capped and non-empty after trim
- `created_by`, `updated_by`, timestamps

The REST read model can compose these into a prototype-friendly shape:

```json
{
  "task": { "id": "uuid", "title": "수습 근무평가 — 조이슨", "dueLabel": "D-3", "who": "조이슨" },
  "scorecard": [
    { "key": "performance", "label": "업무 수행", "score": 4.0, "max": 5, "weightBps": 4000 }
  ],
  "comments": [
    { "kind": "overall", "visibility": "subject", "body": "..." }
  ]
}
```

## 3. Persistence model

Migration filename placeholder:

```text
backend/crates/platform/db/migrations/<next>_create_review_cycles.sql
```

Use the next-free number only immediately before merge. Re-check `origin/main` and the active backend worktrees first because this checkout already has dirty/untracked migration work through `0102`.

### 3.1 `review_code_counters`

Purpose: per-tenant immutable `RV-0001` code issuance.

Columns:
- `org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT`
- `object_prefix TEXT NOT NULL CHECK (object_prefix = 'RV')`
- `next_value BIGINT NOT NULL DEFAULT 1 CHECK (next_value >= 1)`
- `updated_at TIMESTAMPTZ NOT NULL DEFAULT now()`
- `PRIMARY KEY (org_id, object_prefix)`

Runtime grants: `SELECT, INSERT, UPDATE`; no `DELETE`.

### 3.2 `review_cycles`

Suggested DDL shape:

```sql
-- console-gate: audited-table review_cycles
CREATE TABLE review_cycles (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id        UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    cycle_code    TEXT        NOT NULL CHECK (cycle_code ~ '^RV-[0-9]{4,}$'),
    title         TEXT        NOT NULL CHECK (char_length(btrim(title)) BETWEEN 1 AND 160),
    cycle_kind    TEXT        NOT NULL CHECK (cycle_kind IN ('PROBATION','ANNUAL','PROJECT','AD_HOC','CALIBRATION')),
    status        TEXT        NOT NULL DEFAULT 'DRAFT' CHECK (status IN ('DRAFT','SCHEDULED','OPEN','CLOSED','CANCELLED')),
    period_start  DATE        NULL,
    period_end    DATE        NULL,
    due_at        TIMESTAMPTZ NULL,
    score_schema  JSONB       NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(score_schema) = 'object'),
    comment_schema JSONB      NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(comment_schema) = 'object'),
    created_by    UUID        NOT NULL,
    updated_by    UUID        NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, cycle_code),
    CHECK (period_end IS NULL OR period_start IS NULL OR period_end >= period_start),
    CHECK (due_at IS NULL OR period_end IS NULL OR due_at::date >= period_end),
    FOREIGN KEY (created_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (updated_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);
```

Indexes:
- `idx_review_cycles_org_status_due ON review_cycles (org_id, status, due_at DESC NULLS LAST, created_at DESC)`
- `idx_review_cycles_org_kind_period ON review_cycles (org_id, cycle_kind, period_start DESC NULLS LAST)`
- unique natural draft guard: `(org_id, lower(btrim(title)), cycle_kind, coalesce(period_start, '0001-01-01'::date)) WHERE status <> 'CANCELLED'`

### 3.3 `review_cycle_teams`

```sql
-- console-gate: audited-table review_cycle_teams
CREATE TABLE review_cycle_teams (
    id                UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id            UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    cycle_id          UUID        NOT NULL,
    branch_id         UUID        NULL,
    site_id           UUID        NULL,
    team_ref_kind     TEXT        NOT NULL CHECK (team_ref_kind IN ('USER_TEAM','EMPLOYEE_ORG_UNIT','FREEFORM_SNAPSHOT')),
    team_ref          TEXT        NOT NULL CHECK (char_length(btrim(team_ref)) BETWEEN 1 AND 160),
    display_label     TEXT        NOT NULL CHECK (char_length(btrim(display_label)) BETWEEN 1 AND 120),
    display_order     INTEGER     NOT NULL DEFAULT 0,
    target_task_count INTEGER     NULL CHECK (target_task_count IS NULL OR target_task_count >= 0),
    created_by        UUID        NOT NULL,
    updated_by        UUID        NOT NULL,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, cycle_id, team_ref_kind, team_ref),
    FOREIGN KEY (cycle_id, org_id) REFERENCES review_cycles(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (site_id, org_id) REFERENCES registry_sites(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (created_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (updated_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);
```

Indexes:
- `idx_review_cycle_teams_cycle_order ON review_cycle_teams (org_id, cycle_id, display_order, display_label)`
- `idx_review_cycle_teams_branch_site ON review_cycle_teams (org_id, branch_id, site_id) WHERE branch_id IS NOT NULL`

### 3.4 `review_tasks`

```sql
-- console-gate: audited-table review_tasks
CREATE TABLE review_tasks (
    id                   UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id               UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    cycle_id             UUID        NOT NULL,
    team_id              UUID        NULL,
    branch_id            UUID        NULL,
    site_id              UUID        NULL,
    subject_employee_id  UUID        NULL,
    assigned_to_user_id  UUID        NOT NULL,
    reviewer_role        TEXT        NOT NULL CHECK (reviewer_role IN ('SELF','MANAGER','PEER','HR','CALIBRATION_PANEL','EXECUTIVE')),
    task_kind            TEXT        NOT NULL CHECK (task_kind IN ('SELF_REVIEW','MANAGER_REVIEW','PEER_REVIEW','PROBATION_REVIEW','CALIBRATION','APPROVAL')),
    title                TEXT        NOT NULL CHECK (char_length(btrim(title)) BETWEEN 1 AND 180),
    status               TEXT        NOT NULL DEFAULT 'OPEN' CHECK (status IN ('OPEN','IN_PROGRESS','SUBMITTED','ACCEPTED','RETURNED','CANCELLED')),
    due_at               TIMESTAMPTZ NULL,
    started_at           TIMESTAMPTZ NULL,
    submitted_at         TIMESTAMPTZ NULL,
    accepted_at          TIMESTAMPTZ NULL,
    returned_at          TIMESTAMPTZ NULL,
    cancelled_at         TIMESTAMPTZ NULL,
    completed_by         UUID        NULL,
    idempotency_key      TEXT        NOT NULL CHECK (char_length(btrim(idempotency_key)) BETWEEN 16 AND 200),
    request_fingerprint  TEXT        NOT NULL CHECK (request_fingerprint ~ '^[a-f0-9]{64}$'),
    created_by           UUID        NOT NULL,
    updated_by           UUID        NOT NULL,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, idempotency_key),
    FOREIGN KEY (cycle_id, org_id) REFERENCES review_cycles(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (team_id, org_id) REFERENCES review_cycle_teams(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (branch_id, org_id) REFERENCES branches(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (site_id, org_id) REFERENCES registry_sites(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (subject_employee_id, org_id) REFERENCES employees(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (assigned_to_user_id, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (completed_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (created_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (updated_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    CHECK ((status IN ('SUBMITTED','ACCEPTED') AND completed_by IS NOT NULL) OR status NOT IN ('SUBMITTED','ACCEPTED')),
    CHECK (accepted_at IS NULL OR submitted_at IS NOT NULL),
    CHECK (cancelled_at IS NULL OR status = 'CANCELLED')
);
```

Indexes:
- `idx_review_tasks_assignee_status_due ON review_tasks (org_id, assigned_to_user_id, status, due_at ASC NULLS LAST)` for `rvTasks` / `GET /api/v1/me/review-tasks`
- `idx_review_tasks_cycle_team_status ON review_tasks (org_id, cycle_id, team_id, status)` for `rvTeams` progress aggregation
- `idx_review_tasks_subject_cycle ON review_tasks (org_id, subject_employee_id, cycle_id) WHERE subject_employee_id IS NOT NULL`
- `idx_review_tasks_branch_status_due ON review_tasks (org_id, branch_id, status, due_at ASC NULLS LAST) WHERE branch_id IS NOT NULL`
- Optional partial unique to prevent duplicate active work: `(org_id, cycle_id, assigned_to_user_id, subject_employee_id, task_kind) WHERE status NOT IN ('CANCELLED')`

### 3.5 `review_task_score_entries`

```sql
-- console-gate: audited-table review_task_score_entries
CREATE TABLE review_task_score_entries (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id          UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    task_id         UUID        NOT NULL,
    criterion_key   TEXT        NOT NULL CHECK (criterion_key ~ '^[a-z][a-z0-9_]{1,63}$'),
    criterion_label TEXT        NOT NULL CHECK (char_length(btrim(criterion_label)) BETWEEN 1 AND 120),
    score_value     NUMERIC(6,2) NULL,
    score_min       NUMERIC(6,2) NOT NULL DEFAULT 0,
    score_max       NUMERIC(6,2) NOT NULL DEFAULT 5,
    weight_bps      INTEGER     NOT NULL DEFAULT 0 CHECK (weight_bps BETWEEN 0 AND 10000),
    evidence_refs   JSONB       NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(evidence_refs) = 'array'),
    display_order   INTEGER     NOT NULL DEFAULT 0,
    updated_by      UUID        NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, task_id, criterion_key),
    FOREIGN KEY (task_id, org_id) REFERENCES review_tasks(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (updated_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    CHECK (score_max > score_min),
    CHECK (score_value IS NULL OR (score_value >= score_min AND score_value <= score_max))
);
```

Indexes:
- `idx_review_task_score_entries_task_order ON review_task_score_entries (org_id, task_id, display_order, criterion_key)`
- `idx_review_task_score_entries_updated ON review_task_score_entries (org_id, updated_by, updated_at DESC)`

### 3.6 `review_task_comments`

```sql
-- console-gate: audited-table review_task_comments
CREATE TABLE review_task_comments (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id        UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    task_id       UUID        NOT NULL,
    comment_kind  TEXT        NOT NULL CHECK (comment_kind IN ('OVERALL','STRENGTH','IMPROVEMENT','PRIVATE_NOTE','CALIBRATION','RETURN_REASON')),
    visibility    TEXT        NOT NULL CHECK (visibility IN ('SUBJECT','REVIEWER_GROUP','HR_ONLY','AUDIT_ONLY')),
    body          TEXT        NOT NULL CHECK (char_length(btrim(body)) BETWEEN 1 AND 4000),
    created_by    UUID        NOT NULL,
    updated_by    UUID        NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    FOREIGN KEY (task_id, org_id) REFERENCES review_tasks(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (created_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (updated_by, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);
```

Indexes:
- `idx_review_task_comments_task_kind ON review_task_comments (org_id, task_id, comment_kind, created_at ASC)`
- `idx_review_task_comments_author ON review_task_comments (org_id, created_by, created_at DESC)`

### 3.7 `review_events`

Append-only domain timeline for cycle/task transitions and score/comment commits. This is not a replacement for `audit_events`; every write that inserts `review_events` must also insert the corresponding shared audit event in the same `with_audits` transaction.

```sql
-- console-gate: audited-table review_events
CREATE TABLE review_events (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id              UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    cycle_id            UUID        NOT NULL,
    task_id             UUID        NULL,
    event_kind          TEXT        NOT NULL CHECK (event_kind IN ('CYCLE_TRANSITION','TASK_TRANSITION','SCORE_SAVE','COMMENT_SAVE','TASK_RETURN','TASK_CANCEL')),
    from_status         TEXT        NULL,
    to_status           TEXT        NULL,
    actor_id            UUID        NOT NULL,
    reason              TEXT        NULL CHECK (reason IS NULL OR char_length(reason) <= 1000),
    payload             JSONB       NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(payload) = 'object'),
    idempotency_key     TEXT        NOT NULL CHECK (char_length(btrim(idempotency_key)) BETWEEN 16 AND 200),
    request_fingerprint TEXT        NOT NULL CHECK (request_fingerprint ~ '^[a-f0-9]{64}$'),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (id, org_id),
    UNIQUE (org_id, idempotency_key),
    FOREIGN KEY (cycle_id, org_id) REFERENCES review_cycles(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (task_id, org_id) REFERENCES review_tasks(id, org_id) ON DELETE RESTRICT,
    FOREIGN KEY (actor_id, org_id) REFERENCES users(id, org_id) ON DELETE RESTRICT
);
```

Indexes:
- `idx_review_events_cycle_created ON review_events (org_id, cycle_id, created_at DESC)`
- `idx_review_events_task_created ON review_events (org_id, task_id, created_at DESC) WHERE task_id IS NOT NULL`
- `idx_review_events_actor_created ON review_events (org_id, actor_id, created_at DESC)`

Runtime grants: `SELECT, INSERT` only. No update/delete on the append-only timeline.

## 4. RLS, grants, and tenant isolation

All review tables are tenant tables. They must have `org_id UUID NOT NULL`, `ENABLE ROW LEVEL SECURITY`, `FORCE ROW LEVEL SECURITY`, and the standard fail-closed policy:

```sql
ALTER TABLE <table> ENABLE ROW LEVEL SECURITY;
ALTER TABLE <table> FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON <table>
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);
CREATE TRIGGER trg_<table>_org_immutable
    BEFORE UPDATE ON <table>
    FOR EACH ROW EXECUTE FUNCTION enforce_org_id_immutable();
```

Runtime-role grants:
- `review_code_counters`: `SELECT, INSERT, UPDATE`; no delete.
- `review_cycles`: `SELECT, INSERT, UPDATE`; no delete.
- `review_cycle_teams`: `SELECT, INSERT, UPDATE`; no delete.
- `review_tasks`: `SELECT, INSERT, UPDATE`; no delete.
- `review_task_score_entries`: `SELECT, INSERT, UPDATE`; no delete.
- `review_task_comments`: `SELECT, INSERT, UPDATE`; no delete.
- `review_events`: `SELECT, INSERT`; no update/delete.

Deny-by-omission rules:
- Unset `app.current_org` sees zero review rows and rejects writes under `console_rt`.
- A platform/operator principal without an entered tenant context sees zero review rows.
- Cross-org ids in request paths or bodies are indistinguishable from missing rows; return 404/empty lists, not cross-tenant 403 detail.
- Branch/site/team visibility is applied after org RLS using the existing `BranchScope` / PBAC boundary. Unauthorized branch/team rows are omitted from lists and direct reads return not found.
- The client never sends `org_id`; if a request body contains `orgId`, reject it with validation error rather than ignoring it silently.

Tenant-isolation allowlist impact:
- No new entries are required in `global_table_allowlist`, `owner_only_table_allowlist`, or `nullable_org_allowlist` because every new review table is tenant-scoped with non-null `org_id` and RLS.
- The migration may insert feature keys into existing global `feature_catalog`; that table is already globally allowlisted and stores canonical keys only.
- Do not introduce a global `review_score_criteria_catalog` in v1. If a future shared catalog is truly global, it needs an explicit tenant-gate allowlist rationale and must not grant broad raw-table access to `console_rt` unless it is deliberately public metadata.
- Mark mutable review tables with `-- console-gate: audited-table ...` comments so the audit-coverage gate treats them as state-changing tables that require audit evidence.

## 5. Principal-derived org scope and adapter rules

Every adapter method derives org from the authenticated principal / request context:

```rust
let org = current_org().map_err(KernelError::from)?;
with_org_conn::<_, _, PgReviewError>(&self.pool, org, move |tx| {
    Box::pin(async move {
        // SELECTs only through tx.as_mut(); never &self.pool.
    })
}).await
```

Mutation methods use `with_audits` when the audit event depends on locked before/after rows:

```rust
let org = current_org().map_err(KernelError::from)?;
with_audits::<_, _, PgReviewError>(&self.pool, org, move |tx| {
    Box::pin(async move {
        // SELECT ... FOR UPDATE current row
        // validate FSM transition / score/comment invariants
        // UPDATE/INSERT review rows
        // INSERT review_events append-only row
        // build AuditEvent(s).with_org(org)
        Ok((view, audit_events))
    })
}).await
```

Adapter rules:
- Reads run through `with_org_conn`; writes run through `with_audit` or `with_audits`.
- No production code may execute review-table SQL on a bare pool. `console-gate-rls-arming` should flag that.
- Use runtime `sqlx::query` / `QueryBuilder` + typed row mapping, or commit fresh `.sqlx` metadata if the implementation chooses compile-time macros. Runtime SQL keeps `SQLX_OFFLINE=true` friendly for new review crates.
- Do not assume PostgreSQL is on port 5432. DB tests and local commands must read `DATABASE_URL`, `TEST_DATABASE_URL`, or the repo's dev-deps environment. Runtime-role test helpers should change role with `SET ROLE console_rt` on the opened test connection/pool rather than constructing hard-coded URLs.

## 6. Authz and feature seeds

Add feature keys in the migration and corresponding `Feature` enum/matrix entries:

```sql
INSERT INTO feature_catalog (feature_key) VALUES
    ('review_cycle_read'),
    ('review_cycle_manage'),
    ('review_task_write'),
    ('review_calibration_manage')
ON CONFLICT (feature_key) DO NOTHING;
```

Suggested gate semantics:
- `review_cycle_read`: HR/admin/executive visibility into cycles, team progress, and non-private comments inside delegated branch/site scope.
- `review_cycle_manage`: create/update cycles, generate teams/tasks, open/close/cancel cycles.
- `review_task_write`: a user may edit/submit only tasks assigned to themselves; broad role granting is not enough without assignment-row proof.
- `review_calibration_manage`: HR/executive calibration comments, acceptance, returns, and finalization.

Cedar/PBAC integration can later refine these checks, but Postgres RLS remains the hard tenant wall.

## 7. Audit requirements

Required shared audit action strings:
- `review_cycle.create`
- `review_cycle.update`
- `review_cycle.transition`
- `review_cycle_team.upsert`
- `review_task.create`
- `review_task.transition`
- `review_task.score_save`
- `review_task.comment_save`
- `review_task.return`
- `review_task.cancel`

Each mutation audit event must include:
- `actor`: authenticated user or system job user where applicable.
- `org_id`: always attached with `.with_org(org)` so the GUC is armed before mutations and audit insert.
- `branch_id`: when the cycle/task is branch-scoped.
- `target_type`: `review_cycle`, `review_cycle_team`, `review_task`, `review_score_entry`, or `review_comment`.
- `target_id`: UUID string; expose `cycle_code` in snapshots for auditor usability.
- `trace_id` / `span_id` from request context.
- before/after snapshots for state changes, score/comment saves, and team generation.
- transition snapshots with `from_status`, `to_status`, reason, due dates, assigned user, subject employee, and relevant score/comment summary.

Audit atomicity requirements:
- Domain row update, `review_events` insert, and `audit_events` insert commit together.
- If validation fails after a `SELECT ... FOR UPDATE`, neither review data nor audit rows persist.
- REST tests must not seed behavior-critical mutable review state with bare unaudited `INSERT`/`UPDATE`; use adapter/REST paths or a repo-approved audited fixture helper.

## 8. API/read-model contract for later REST child

Proposed paths for the REST/OpenAPI child:
- `GET /api/v1/review-cycles`
- `POST /api/v1/review-cycles`
- `GET /api/v1/review-cycles/{cycleId}`
- `PATCH /api/v1/review-cycles/{cycleId}`
- `POST /api/v1/review-cycles/{cycleId}/transitions`
- `PUT /api/v1/review-cycles/{cycleId}/teams`
- `POST /api/v1/review-cycles/{cycleId}/tasks:generate`
- `GET /api/v1/review-cycles/{cycleId}/team-progress`
- `GET /api/v1/me/review-tasks`
- `GET /api/v1/review-tasks/{taskId}`
- `PUT /api/v1/review-tasks/{taskId}/scores`
- `POST /api/v1/review-tasks/{taskId}/comments`
- `POST /api/v1/review-tasks/{taskId}/transitions`

Prototype mapping:

| Prototype field | Backend read model |
|---|---|
| `rvTeams[].team` | `ReviewTeamProgress.display_label` |
| `rvTeams[].pctLabel` / `pctW` | computed `completed_count / total_count` |
| `rvTeams[].pctColor` | client presentation from percent thresholds |
| `rvTasks[].t` | task title, including subject display name when present |
| `rvTasks[].d` | server due label or raw `due_at`; client may render D-day |
| `rvTasks[].who` | subject employee/person display link if visible |
| write button | `GET /api/v1/review-tasks/{taskId}` then score/comment save + task transition |

Response shape for `GET /api/v1/review-cycles/{cycleId}/team-progress`:

```json
{
  "cycleId": "uuid",
  "teams": [
    { "teamId": "uuid", "team": "경영지원팀", "completedCount": 8, "totalCount": 10, "pct": 80 }
  ]
}
```

Response shape for `GET /api/v1/me/review-tasks`:

```json
{
  "items": [
    {
      "taskId": "uuid",
      "cycleId": "uuid",
      "title": "수습 근무평가 — 조이슨",
      "dueAt": "RFC3339",
      "dueLabel": "D-3",
      "subject": { "employeeId": "uuid", "displayName": "조이슨" },
      "status": "open"
    }
  ]
}
```

OpenAPI/client obligations:
- Any REST path edit must update `backend/openapi/openapi.yaml` and run the repo's canonical client regeneration for TypeScript, Kotlin, and Swift in one serialized lane.
- Generated clients should not expose or require `orgId` for review writes.
- Route drift tests should prove implemented routes match OpenAPI.

## 9. Implementation sequence

1. Re-check active branch, `origin/main`, open PRs, and migration filenames immediately before implementation; do not reuse the design-time `<next>` placeholder.
2. Add kernel ID newtypes and review clean-arch crates.
3. Add the migration with tables, constraints, indexes, feature seeds, RLS, grants, immutable-org triggers, and audited-table markers.
4. Implement domain validators and FSM transition functions.
5. Implement application commands/read models:
   - list/get cycles;
   - create/update cycle;
   - transition cycle;
   - replace teams;
   - generate tasks idempotently;
   - list assigned tasks;
   - get task scorecard/comments;
   - save scores/comments;
   - transition task.
6. Implement Postgres adapter with `with_org_conn` / `with_audits` only, branch-scope predicates, and runtime SQL for `SQLX_OFFLINE=true` friendliness.
7. Implement REST module and app router wiring.
8. Update OpenAPI/generated clients in the REST child only after backend route shape is stable.
9. Run focused tests and gates below.

## 10. Test obligations

Domain/unit tests:
- cycle status transition graph accepts only valid transitions and terminal states stay terminal.
- task status transition graph accepts submit/return/accept/cancel rules and rejects skipped completion.
- score schema validation rejects blank criterion keys, invalid weights, score values outside bounds, and malformed evidence refs.
- comment validation rejects empty/overlong bodies and unsupported visibility/kind values.
- team progress calculation handles zero tasks, cancelled tasks, returned tasks, submitted/accepted tasks, and optional target denominator.

Adapter DB tests as real `console_rt` / FORCE RLS:
- runtime-role pool uses the same migrated database but executes as `console_rt` (`SET ROLE console_rt` or app runtime URL), not a superuser/BYPASSRLS owner.
- without `app.current_org`, `console_rt` sees zero review rows and writes fail/affect zero rows.
- with org A armed, org B cycles/tasks/scores/comments/events are invisible and direct lookups return not found.
- client-supplied org fields are rejected before persistence.
- create cycle/team/task derives `org_id` from `current_org()` and writes composite FKs inside that org only.
- branch/site deny-by-omission: list omits unauthorized tasks/teams and direct get/update returns not found.
- task completion by a non-assigned actor is denied even if the actor has a broad feature role.
- score/comment save emits `review_events` and shared `audit_events` atomically.
- rollback: a failing score/comment replacement leaves no partial score rows, comment rows, review_events, or audit_events.
- runtime role has no `DELETE` grant on review tables; `review_events` also has no `UPDATE` grant.
- `org_id` update is rejected by the immutable-org trigger.
- team progress aggregation returns the prototype counts/percentages under RLS.

REST tests:
- 401 unauthenticated; 403 when authenticated but missing feature/assignment authority.
- 404/empty list for cross-org or branch-inaccessible ids.
- `GET /api/v1/review-cycles/{cycleId}/team-progress` returns `rvTeams`-ready labels/counts/percentages.
- `GET /api/v1/me/review-tasks` returns only the principal's assigned tasks unless the caller has a review-management read route.
- task score/comment save rejects `orgId`, `assignedToUserId`, `completedBy`, and status fields in request bodies.
- task transition submit/return/accept writes audit and updates due/completion timestamps correctly.
- OpenAPI route drift remains green after the REST child adds specs and regenerates clients.

Gate commands for implementation PRs:

```text
cd backend
SQLX_OFFLINE=true cargo fmt --check
SQLX_OFFLINE=true cargo test -p console-review-domain -p console-review-application -p console-review-adapter-postgres -p console-review-rest
SQLX_OFFLINE=true cargo clippy -p console-review-domain -p console-review-application -p console-review-adapter-postgres -p console-review-rest --all-targets -- -D warnings
cargo run -p console-gate-tenant-isolation
cargo run -p console-gate-rls-arming
cargo run -p console-gate-audit-coverage
cargo run -p console-gate-migration-safety
cargo run -p console-gate-layer-boundary
```

Use the repo's current DB bootstrap/dev-deps command to supply `DATABASE_URL` for dynamic DB tests; do not hard-code host ports in tests or docs. If local Docker or macOS environment differs, the test harness should fail with a clear missing-DSN message rather than assuming a default port.

## 11. Acceptance checklist for implementation cards

- Review cycles, cycle teams, per-person tasks, score entries, comments, and append-only events are stored in tenant-scoped tables.
- `rvTeams` progress is derived from task rows, not mutable percent authority.
- `rvTasks` lists only assigned or authorized tasks and includes due labels/subject links needed by the prototype.
- Every table has constraints, indexes, composite same-org FKs where applicable, immutable-org trigger, RLS enabled, and FORCE RLS.
- Runtime grants are minimal and omit physical delete; append-only events omit update/delete.
- No tenant-isolation allowlist entry is needed for the new review tables.
- All reads/writes derive org from the principal/current request context and use `with_org_conn` / `with_audit` / `with_audits`.
- No client body or query parameter can set `org_id` or completion authority fields.
- All transitions and score/comment saves are audited atomically and visible in both `review_events` and `audit_events`.
- Tests prove `console_rt` RLS behavior, deny-by-omission, no bare-pool access, audit rollback, no delete grants, and `SQLX_OFFLINE=true` compatibility.
- The implementation picks the migration number only at merge time and does not assume PostgreSQL port 5432.
