-- Users and branch memberships.
-- Roles follow the 5-role matrix from plan §2.3: SUPER_ADMIN, ADMIN, MECHANIC,
-- RECEPTIONIST, EXECUTIVE. Stored as text[] so a user can hold multiple roles
-- (e.g. ADMIN + EXECUTIVE for cross-branch visibility).
-- 예방점검팀 affiliation is a `team` text attribute (정비/예방), NOT a 6th role.

CREATE TABLE users (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    display_name TEXT        NOT NULL,
    phone        TEXT,
    -- One or more roles from the 5-role matrix.
    roles        TEXT[]      NOT NULL DEFAULT '{}' CHECK (
                     roles <@ ARRAY['SUPER_ADMIN','ADMIN','MECHANIC','RECEPTIONIST','EXECUTIVE']
                 ),
    -- 정비 or 예방 team affiliation (null = not a field technician).
    team         TEXT        CHECK (team IN ('정비', '예방', '관리', '접수')),
    is_active    BOOLEAN     NOT NULL DEFAULT true,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- A user may be assigned to multiple branches (ADMIN manages their branch(es);
-- SUPER_ADMIN/EXECUTIVE scope is handled in policy, not rows here).
CREATE TABLE user_branches (
    user_id   UUID NOT NULL REFERENCES users(id),
    branch_id UUID NOT NULL REFERENCES branches(id),
    PRIMARY KEY (user_id, branch_id)
);
