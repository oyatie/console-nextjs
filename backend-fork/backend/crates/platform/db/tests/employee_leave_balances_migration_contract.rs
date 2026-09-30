#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::type_complexity
)]
//! Contract proof for `0218_create_employee_leave_balances.sql` — the PR-1
//! schema-only leave-balance table of the leave-writer removal (console-hee2).
//!
//! PR-1 is additive and behavior-neutral: the table exists with the exact
//! leave-owned shape, tenant isolation, and least-privilege grants, but nothing
//! reads or writes it yet. These assertions read the live catalog (not DDL
//! text) so a `COMMENT ON COLUMN` naming a missing column, a missing RLS half,
//! or a leaked grant is caught where it is real. The backfill and writer
//! re-pointing land in 0219 (PR-2); the `employees` balance columns and the
//! `console_leave_definer` grant on `employees` are dropped in 0220 (PR-4).

use sqlx::{PgPool, Row};

// ---------------------------------------------------------------------------
// Shape: exact column set and order, types, nullability, defaults, and the
// primary key / foreign keys that make the table tenant-global and erasable.
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "./migrations")]
async fn employee_leave_balances_has_the_exact_pr1_shape(pool: PgPool) {
    let columns: Vec<(
        String,
        String,
        Option<i32>,
        Option<i32>,
        bool,
        Option<String>,
    )> = sqlx::query(
        "SELECT column_name,
                    data_type,
                    numeric_precision,
                    numeric_scale,
                    is_nullable = 'NO' AS not_null,
                    column_default
               FROM information_schema.columns
              WHERE table_schema = 'public'
                AND table_name = 'employee_leave_balances'
              ORDER BY ordinal_position",
    )
    .fetch_all(&pool)
    .await
    .unwrap()
    .into_iter()
    .map(|row| {
        (
            row.get("column_name"),
            row.get("data_type"),
            row.get("numeric_precision"),
            row.get("numeric_scale"),
            row.get("not_null"),
            row.get("column_default"),
        )
    })
    .collect();

    let names: Vec<&str> = columns.iter().map(|(name, ..)| name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "org_id",
            "employee_id",
            "leave_accrued",
            "leave_used",
            "leave_remaining",
            "updated_at"
        ],
        "the leave ledger relocates exactly the three balance columns, keyed tenant-global"
    );

    for (name, data_type, not_null) in columns
        .iter()
        .map(|(name, data_type, .., not_null, _)| (name.as_str(), data_type.as_str(), *not_null))
    {
        match name {
            "org_id" | "employee_id" => {
                assert_eq!(data_type, "uuid", "{name} must be a UUID");
                assert!(not_null, "{name} must be NOT NULL");
            }
            "leave_accrued" | "leave_used" | "leave_remaining" => {
                assert_eq!(data_type, "numeric", "{name} must be NUMERIC(16,6)");
                assert!(not_null, "{name} must be NOT NULL");
            }
            "updated_at" => {
                assert_eq!(
                    data_type, "timestamp with time zone",
                    "{name} must be timestamptz"
                );
                assert!(not_null, "{name} must be NOT NULL");
            }
            other => panic!("unexpected column {other}"),
        }
    }

    for name in ["leave_accrued", "leave_used", "leave_remaining"] {
        let (_, _, precision, scale, _, default) =
            columns.iter().find(|(column, ..)| column == name).unwrap();
        assert_eq!(*precision, Some(16), "{name} precision");
        assert_eq!(*scale, Some(6), "{name} scale");
        assert_eq!(
            default.as_deref(),
            Some("0"),
            "{name} must default to 0 so a missing balance is an explicit zero"
        );
    }

    let (_, _, _, _, _, updated_default) = columns
        .iter()
        .find(|(column, ..)| column == "updated_at")
        .unwrap();
    assert!(
        updated_default
            .as_deref()
            .is_some_and(|d| d.contains("now()")),
        "updated_at must default to now()"
    );

    // PRIMARY KEY (org_id, employee_id) + the two cascading FKs.
    let mut constraints: Vec<String> = sqlx::query_scalar(
        "SELECT pg_get_constraintdef(c.oid)
           FROM pg_constraint c
           JOIN pg_class r ON r.oid = c.conrelid
           JOIN pg_namespace n ON n.oid = r.relnamespace
          WHERE n.nspname = 'public'
            AND r.relname = 'employee_leave_balances'
            AND c.contype IN ('p', 'f')",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    constraints.sort();

    assert_eq!(
        constraints,
        vec![
            "FOREIGN KEY (employee_id, org_id) REFERENCES employees(id, org_id) ON DELETE CASCADE"
                .to_owned(),
            "FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE CASCADE".to_owned(),
            "PRIMARY KEY (org_id, employee_id)".to_owned(),
        ],
        "PK must be tenant-global, and both parent rows must cascade so a deleted \
         employee's personal balance is erasable with the row"
    );
}

// ---------------------------------------------------------------------------
// Tenant isolation: ENABLE + FORCE, with the canonical org_isolation policy.
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "./migrations")]
async fn employee_leave_balances_is_tenant_isolated(pool: PgPool) {
    let row = sqlx::query(
        "SELECT c.relrowsecurity, c.relforcerowsecurity
           FROM pg_class c
          WHERE c.oid = 'public.employee_leave_balances'::regclass",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        row.get::<bool, _>("relrowsecurity"),
        "employee_leave_balances must enable RLS"
    );
    // FORCE is the 0030 owner-boundary floor: it subjects a non-BYPASSRLS owner
    // to the policies. console_app (the owner) is BYPASSRLS and expectedly
    // exempt; the enforced tenant boundary is the NOBYPASSRLS serving roles
    // (console_rt / console_leave_definer), which ENABLE + FORCE keep scoped.
    assert!(
        row.get::<bool, _>("relforcerowsecurity"),
        "employee_leave_balances must FORCE RLS (the 0030 owner-boundary floor); \
         console_app is BYPASSRLS and expectedly exempt, while the NOBYPASSRLS \
         console_rt / console_leave_definer serving roles are the enforced tenant boundary"
    );

    let policies: Vec<(String, Option<String>, Option<String>)> = sqlx::query(
        "SELECT policyname, qual, with_check
           FROM pg_policies
          WHERE schemaname = 'public'
            AND tablename = 'employee_leave_balances'",
    )
    .fetch_all(&pool)
    .await
    .unwrap()
    .into_iter()
    .map(|row| {
        (
            row.get("policyname"),
            row.get("qual"),
            row.get("with_check"),
        )
    })
    .collect();

    assert_eq!(policies.len(), 1, "exactly one org_isolation policy");
    let (name, using, with_check) = &policies[0];
    assert_eq!(name, "org_isolation");
    let using = using.as_deref().expect("USING clause is present");
    let with_check = with_check.as_deref().expect("WITH CHECK clause is present");
    assert!(
        using.contains("current_setting") && with_check.contains("current_setting"),
        "both policy clauses must gate on the app.current_org GUC"
    );
}

// ---------------------------------------------------------------------------
// Grants: the runtime role reads only; the leave definer reads and writes; the
// public and every mutation verb is withheld from the runtime role.
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "./migrations")]
async fn employee_leave_balances_grants_are_least_privilege(pool: PgPool) {
    let row = sqlx::query(
        "SELECT has_table_privilege('console_rt', 'public.employee_leave_balances', 'SELECT') AS rt_select,
                has_table_privilege('console_rt', 'public.employee_leave_balances', 'INSERT') AS rt_insert,
                has_table_privilege('console_rt', 'public.employee_leave_balances', 'UPDATE') AS rt_update,
                has_table_privilege('console_rt', 'public.employee_leave_balances', 'DELETE') AS rt_delete,
                has_table_privilege('console_leave_definer', 'public.employee_leave_balances', 'SELECT') AS definer_select,
                has_table_privilege('console_leave_definer', 'public.employee_leave_balances', 'INSERT') AS definer_insert,
                has_table_privilege('console_leave_definer', 'public.employee_leave_balances', 'UPDATE') AS definer_update,
                has_table_privilege('console_leave_definer', 'public.employee_leave_balances', 'DELETE') AS definer_delete",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    assert!(
        row.get::<bool, _>("rt_select"),
        "console_rt must read the leave ledger"
    );
    for verb in ["rt_insert", "rt_update", "rt_delete"] {
        assert!(
            !row.get::<bool, _>(verb),
            "console_rt must not mutate the leave ledger ({verb})"
        );
    }

    for verb in ["definer_select", "definer_insert", "definer_update"] {
        assert!(
            row.get::<bool, _>(verb),
            "console_leave_definer must hold {verb} so its SECURITY DEFINER functions can write the ledger"
        );
    }
    assert!(
        !row.get::<bool, _>("definer_delete"),
        "console_leave_definer must not delete balances directly (erasure is the employees cascade)"
    );

    // PUBLIC is a pseudo-role, not a name `has_table_privilege` can resolve, so
    // its grants are read off the ACL: `REVOKE ALL ... FROM PUBLIC` must leave
    // no entry whose grantee is PUBLIC (grantee 0).
    let public_grants: i64 = sqlx::query_scalar(
        "SELECT count(*)
           FROM pg_class c
           CROSS JOIN LATERAL aclexplode(coalesce(c.relacl, acldefault('r', c.relowner))) acl
          WHERE c.oid = 'public.employee_leave_balances'::regclass
            AND acl.grantee = 0",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        public_grants, 0,
        "PUBLIC must hold no ACL entry on the leave ledger"
    );
}
