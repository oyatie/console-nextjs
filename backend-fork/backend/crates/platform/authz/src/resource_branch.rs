//! The branch a RESOURCE lives in — a type boundary, not a scan.
//!
//! `console-gate-fabricated-branch` is a text scan and says so in its own module
//! doc: three blind spots are known and none of them is fixable by another
//! pattern, because "a fabrication moved one function away scans clean" is a
//! property of reading text rather than a bug. The gate's own doc names the
//! control that closes all three — this module.
//!
//! [`ResourceBranch`] wraps a [`BranchId`] — and the [`OrgId`] it was read
//! under — in fields that are private to THIS module. Not `pub(crate)`:
//! private. So no expression anywhere — not in a
//! caller, not in this crate, not one function away, not behind an import alias
//! — can put a `BranchId` the caller invented into one. `BranchId::new()`,
//! `branches.iter().next()`, `.nth(0)`, `branches.first().copied()` and every
//! shape nobody has written yet all fail the same way: there is no constructor
//! that takes a `BranchId`, so there is nothing to call. The compiler rejects
//! them; nothing has to recognise them first.
//!
//! # "Came from Postgres" is not the claim
//!
//! A newtype whose entrance is any row read proves only that a uuid made a round
//! trip through the database. That is strictly weaker than "this is the
//! resource's branch", and the gap swallows both shapes the gate exists to stop:
//! an echo query (`SELECT $1::uuid` with a minted id bound) fabricates outright,
//! and a free-form `from_row(row, column)` accepts `branch_id` read off the
//! PRINCIPAL's own membership row — the original tautology, re-spelled as a read.
//!
//! So the only entrance is [`ResourceBranch::lookup`], and the caller does not
//! choose its SQL. It supplies its tenant, a [`BranchScopedResource`] — a closed
//! enum — and a row id; the table name, the branch column and the `org_id`
//! predicate come from this module. There is
//! no variant for `user_branches` or any other principal-side table and a caller
//! cannot add one, so the tautology has no spelling here rather than being
//! discouraged.
//!
//! The proof is executable and lives on [`ResourceBranch`] itself as
//! `compile_fail` doctests.
//!
//! # Why the failing examples are PAIRED with compiling ones
//!
//! A bare `compile_fail` passes when its example fails to compile for ANY
//! reason — a typo, a renamed type, a missing import — so on its own it is green
//! for the wrong reason and proves nothing. The `E0xxx` annotations below do NOT
//! close that: **stable rustdoc does not check the error code.** Verified by
//! mutation rather than assumed: changing `E0423` to `E0308` left the doctest
//! passing, so the code is documentation of intent, not an assertion.
//!
//! What closes it is a control example that DOES compile using the same imports
//! and the same paths, so the fabrication is the only candidate left for what is
//! failing. There are five failing examples and three controls, and they are not
//! all equally tight: two failures are a one-token delta from their control (an
//! argument type; a turbofish). The other three share imports, type name and
//! call syntax with a control but differ by more than the fabrication alone.
//!
//! For all five, the annotated `E0xxx` is the diagnostic that was actually
//! observed — each snippet was compiled against the built rlib and the code
//! copied off the output, not guessed from the shape of the example.

use console_kernel_core::{BranchId, KernelError, OrgId};
use sqlx::{Executor, Postgres};

/// The branch-scoped resource kinds authorization can read a branch for.
///
/// The table and its branch column are named HERE, never by the caller. That is
/// the whole difference between "a uuid that came back from Postgres" and "the
/// branch of THIS resource": a caller can pick which resource and which row, but
/// not which table, so it cannot point the read at `user_branches` or any other
/// principal-side table and get a `ResourceBranch` back.
///
/// `every_resource_kind_names_a_table_and_branch_column_that_exist` runs every
/// variant against the live schema, so a typo is a test failure rather than a
/// runtime denial nobody can explain.
///
/// Add a variant when a call site moves to [`crate::authorize_scoped`]. A
/// resource whose branch column is nullable does not belong here: those rows are
/// branch-LESS, and their door is [`crate::authorize_capability_at`]. If one is
/// added anyway, a `NULL` fails to decode as a uuid and
/// [`ResourceBranch::lookup`] returns `Internal` — closed, not defaulted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BranchScopedResource {
    WorkOrder,
    P1Dispatch,
    InventoryItem,
    RegistryCustomer,
    RegistrySite,
    RegistryEquipment,
}

/// [`BranchScopedResource::ALL`] and each variant's statement come from ONE
/// list below, because two hand-maintained lists drift: `ALL` carried its own
/// length, so a seventh variant compiled while both tests — which iterate `ALL`
/// — silently never visited it.
///
/// Now `select_branch_sql`'s `match` is exhaustive, so a variant that is not in
/// the list does not compile, and `ALL` is built from the same line.
///
/// The statement is a template with ONE hole, and the hole is a literal table
/// name. There is no string built at runtime, so no caller-supplied text can
/// reach the SQL even in principle (`sqlx`'s injection lint is satisfied without
/// an `AssertSqlSafe` bypass) — and a join, a CTE or an extra predicate has no
/// spelling here at all rather than being something a test must recognise.
macro_rules! branch_scoped_resources {
    ($($variant:ident => $table:literal,)+) => {
        impl BranchScopedResource {
            pub const ALL: &[Self] = &[$(Self::$variant),+];

            const fn select_branch_sql(self) -> &'static str {
                match self {
                    $(Self::$variant => concat!(
                        "SELECT branch_id FROM ", $table, " WHERE org_id = $1 AND id = $2"
                    ),)+
                }
            }
        }
    };
}

branch_scoped_resources! {
    WorkOrder => "work_orders",
    P1Dispatch => "p1_dispatches",
    InventoryItem => "inventory_items",
    RegistryCustomer => "registry_customers",
    RegistrySite => "registry_sites",
    RegistryEquipment => "registry_equipment",
}

/// The branch a resource row lives in, and the TENANT it was read under, read
/// from that row by this crate.
///
/// [`crate::authorize_scoped`] takes one of these and nothing else. A resource
/// that has no branch at all is a different door,
/// [`crate::authorize_capability_at`] — not an `Option` on this one.
///
/// The org travels WITH the branch because the two are one claim. `lookup`'s
/// tenant is an argument, and an argument can be taken from the request path
/// rather than from the principal; carrying it here lets
/// [`crate::authorize_scoped`] check that the tenant the row was read under is
/// the tenant the principal belongs to, so a mismatched pair is a denial at the
/// decision rather than an ALLOW against another tenant's resource.
///
/// # A fabricated branch does not compile
///
/// THE CONTROL. Everything the failing example below relies on — both imports,
/// the type name, the `::from` path — resolves and compiles here.
/// `From<T> for T` is reflexive, so `ResourceBranch::from` is a real, callable
/// path:
///
/// ```
/// use console_kernel_core::BranchId;
/// use console_platform_authz::ResourceBranch;
///
/// fn one_function_away(branch: ResourceBranch) -> ResourceBranch {
///     ResourceBranch::from(branch)
/// }
/// ```
///
/// Now replace the argument type with `BranchId` and change nothing else. There
/// is no `From<BranchId>`, so moving the expression one function away — the
/// blind spot no text scan can close — does not help:
///
/// ```compile_fail,E0308
/// use console_kernel_core::BranchId;
/// use console_platform_authz::ResourceBranch;
///
/// fn one_function_away(branch: BranchId) -> ResourceBranch {
///     ResourceBranch::from(branch)
/// }
/// ```
///
/// Nor does the tuple-struct constructor, which the module-private field keeps
/// out of scope everywhere else:
///
/// ```compile_fail,E0423
/// use console_kernel_core::BranchId;
/// use console_platform_authz::ResourceBranch;
///
/// let fabricated = ResourceBranch(BranchId::new());
/// ```
///
/// Nor an inherent constructor, because there is none that takes a `BranchId`:
///
/// ```compile_fail,E0599
/// use console_kernel_core::BranchId;
/// use console_platform_authz::ResourceBranch;
///
/// let fabricated = ResourceBranch::new(BranchId::new());
/// ```
///
/// # Nor a row read whose shape the CALLER chose
///
/// An echo query fabricates outright by binding a minted id and reading it
/// straight back: `query_scalar("SELECT $1::uuid").bind(BranchId::new().into_uuid())`.
/// That needs a [`Decode`](sqlx::Decode) impl, so this type has none:
///
/// ```compile_fail,E0277
/// fn decodes_from_any_sql_scalar<
///     T: for<'r> sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
/// >() {
/// }
///
/// decodes_from_any_sql_scalar::<console_platform_authz::ResourceBranch>();
/// ```
///
/// THE CONTROL for that one, and a minimal delta: same bound, same turbofish
/// syntax, only the type differs — and `uuid::Uuid` does decode.
///
/// ```
/// fn decodes_from_any_sql_scalar<
///     T: for<'r> sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
/// >() {
/// }
///
/// decodes_from_any_sql_scalar::<uuid::Uuid>();
/// ```
///
/// And a free-form `from_row(row, column)` would accept ANY row with ANY column
/// name — including `branch_id` off the principal's own membership row, where
/// `branch_scope.allows(b)` is true by construction and the branch dimension
/// disappears silently. There is no such entrance:
///
/// ```compile_fail,E0599
/// let caller_chose_the_column = console_platform_authz::ResourceBranch::from_row;
/// ```
///
/// THE CONTROL: the entrance that does exist, referenced exactly the same way.
/// It takes a resource KIND, not a table name or a column name.
///
/// ```
/// let authz_chose_the_table = console_platform_authz::ResourceBranch::lookup::<&sqlx::PgPool>;
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResourceBranch {
    org: OrgId,
    branch: BranchId,
}

impl ResourceBranch {
    /// Read one resource row's branch, within one tenant. The ONLY entrance.
    ///
    /// `resource` picks the table and its branch column from
    /// [`BranchScopedResource`]; `id` picks the row. No part of the SQL comes
    /// from the caller, so there is no argument that could aim this at a
    /// principal-side table.
    ///
    /// # `org` is not optional and not a convenience
    ///
    /// A row id is a bare uuid: nothing about it names a tenant. Without the
    /// `org_id` predicate this reads ANY tenant's row, so the value it returned
    /// proved "a uuid made a round trip through Postgres" and not "this is this
    /// tenant's resource branch" — and the branch it handed back was one the
    /// principal's own org never contained. A row belonging to any other tenant
    /// is `NotFound`, exactly as a row that does not exist.
    ///
    /// `org` is still an ARGUMENT, so it can be taken from the request path
    /// instead of from the principal. That is why the value it names is kept on
    /// the returned [`ResourceBranch`] and re-checked against the principal by
    /// [`crate::authorize_scoped`]: passing the wrong tenant here cannot become
    /// an allow, it can only become a denial.
    ///
    /// This is the same predicate RLS enforces, applied here rather than relied
    /// on: authorization must not be correct only for connections that happen to
    /// have armed `app.current_org`.
    ///
    /// # Errors
    ///
    /// [`console_kernel_core::ErrorKind::NotFound`] when no such row exists IN
    /// THIS ORG — never a defaulted or absent branch, because "the resource is
    /// missing" and "the resource is branch-less" authorize differently and must
    /// not collapse into one another. [`console_kernel_core::ErrorKind::Internal`]
    /// on any other database failure.
    pub async fn lookup<'e, E>(
        executor: E,
        org: OrgId,
        resource: BranchScopedResource,
        id: uuid::Uuid,
    ) -> Result<Self, KernelError>
    where
        E: Executor<'e, Database = Postgres>,
    {
        let branch: Option<uuid::Uuid> = sqlx::query_scalar(resource.select_branch_sql())
            .bind(*org.as_uuid())
            .bind(id)
            .fetch_optional(executor)
            .await
            .map_err(|err| {
                KernelError::internal(format!("cannot read {resource:?} branch: {err}"))
            })?;

        branch
            .map(|branch| Self {
                org,
                branch: BranchId::from_uuid(branch),
            })
            .ok_or_else(|| {
                KernelError::not_found(format!("no {resource:?} row {id} to authorize against"))
            })
    }

    /// The branch this resource lives in — readable only inside this crate.
    ///
    /// Not `pub`. A public accessor keeps the tenant check optional by another
    /// route: [`crate::authorize`] takes a bare [`BranchId`] and has no org to
    /// compare, so `authorize(principal, action, resource.get())` reaches the
    /// same decision as [`crate::authorize_scoped`] while skipping the
    /// comparison, and for a `BranchScope::All` principal that is an ALLOW
    /// against another tenant's resource. Keeping the unwrap inside the crate
    /// leaves [`crate::authorize_scoped`] as the only way out of this type, so
    /// the org check is not a step a caller can decline to take. The proof is a
    /// `compile_fail` doctest on [`crate::authorize`].
    #[must_use]
    pub(crate) const fn get(self) -> BranchId {
        self.branch
    }

    /// The tenant this branch was read under — the `org_id` the row matched, not
    /// a claim about who is asking.
    #[must_use]
    pub const fn org(self) -> OrgId {
        self.org
    }

    /// Unit-test constructor. `#[cfg(test)]` and `pub(crate)`, so it exists only
    /// in this crate's own test build and is not part of the published surface —
    /// a downstream crate cannot reach it even in ITS tests.
    #[cfg(test)]
    pub(crate) const fn for_test(org: OrgId, branch: BranchId) -> Self {
        Self { org, branch }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use console_kernel_core::{BranchId, OrgId};

    #[test]
    fn a_resource_branch_carries_the_branch_and_tenant_it_was_read_from() {
        let branch = BranchId::new();
        let org = OrgId::knl();

        assert_eq!(ResourceBranch::for_test(org, branch).get(), branch);
        assert_eq!(ResourceBranch::for_test(org, branch).org(), org);
    }

    /// The tripwire on the one way the enum could be extended into the hole
    /// again: a `ResourceBranch` read off `user_branches` would make
    /// `branch_scope.allows(b)` true by construction — the exact tautology this
    /// type replaces, re-spelled as a row read.
    ///
    /// It asserts what each statement MUST be, character for character. The
    /// earlier form asserted what it must not CONTAIN, over a hand-maintained
    /// four-entry denylist matched as a substring: a principal-side table
    /// spelled with different whitespace, reached through a join, or named in a
    /// CTE passed it silently, and so did any table nobody had thought to list.
    /// Exact equality has no such gap — every deviation is a failure, including
    /// the ones nobody enumerated.
    ///
    /// The `match` is exhaustive, so a variant added without a line here does
    /// not compile; `RESOURCE_SIDE_TABLES` is the second half, because the
    /// match alone would be satisfied by editing both sides at once. `ALL` is
    /// generated from the same list the statements are, so this loop cannot
    /// skip a variant that exists.
    #[test]
    fn every_resource_kind_reads_exactly_one_resource_side_table_by_org_and_id() {
        // The tables authorization may read a resource's branch from. This is
        // the whole set, not a set of exclusions: `user_branches`, `users`,
        // `branches` and `user_role_assignments` are absent because they are
        // not resource tables, and so is every table nobody has named.
        const RESOURCE_SIDE_TABLES: [&str; 6] = [
            "work_orders",
            "p1_dispatches",
            "inventory_items",
            "registry_customers",
            "registry_sites",
            "registry_equipment",
        ];

        for &resource in BranchScopedResource::ALL {
            let table = match resource {
                BranchScopedResource::WorkOrder => "work_orders",
                BranchScopedResource::P1Dispatch => "p1_dispatches",
                BranchScopedResource::InventoryItem => "inventory_items",
                BranchScopedResource::RegistryCustomer => "registry_customers",
                BranchScopedResource::RegistrySite => "registry_sites",
                BranchScopedResource::RegistryEquipment => "registry_equipment",
            };

            assert!(
                RESOURCE_SIDE_TABLES.contains(&table),
                "{resource:?} names `{table}`, which is not a resource-side table"
            );
            assert_eq!(
                resource.select_branch_sql(),
                format!("SELECT branch_id FROM {table} WHERE org_id = $1 AND id = $2"),
                "{resource:?} must read its own row's branch, scoped by tenant and row id, \
                 and nothing else"
            );
        }
    }
}
