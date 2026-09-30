#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The receipt store's `owner`/`target` CHECK constraints, proven against a real
//! PostgreSQL rather than as strings.
//!
//! `ReceiptOwner::owner_check_constraint_sql()` and
//! `target_check_constraint_sql()` generate the constraint bodies that migration
//! 0223 installs. Before 0223 those functions were referenced ONLY by unit tests
//! that asserted the generated STRING — they described columns PostgreSQL had
//! never seen, which is a control that looks present and is not.
//!
//! These tests close that by exercising the constraints themselves, and they are
//! written over `ReceiptOwner::ALL` and `DispatchTarget::ALL` rather than a
//! hand-listed set, so a seventh object key cannot be added without either
//! passing here or failing loudly.

use console_ontology_canonical_domain::{DispatchTarget, ReceiptOwner};
use sqlx::PgPool;
use uuid::Uuid;

async fn seed_actor(pool: &PgPool) -> (Uuid, Uuid) {
    let org = Uuid::new_v4();
    let actor = Uuid::new_v4();
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, 'Receipt Widening')")
        .bind(org)
        .bind(format!("recpt-{}", &org.to_string()[..8]))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO users (id, org_id, display_name) VALUES ($1, $2, 'Receipt Tester')")
        .bind(actor)
        .bind(org)
        .execute(pool)
        .await
        .unwrap();
    (org, actor)
}

async fn insert_receipt(
    pool: &PgPool,
    org: Uuid,
    actor: Uuid,
    owner: &str,
    target: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO ont_action_command_receipts \
         (org_id, command_id, actor_id, payload_digest, receipt, created_at, owner, target) \
         VALUES ($1, $2, $3, decode(repeat('ab', 32), 'hex'), '{}'::jsonb, now(), $4, $5)",
    )
    .bind(org)
    .bind(Uuid::new_v4())
    .bind(actor)
    .bind(owner)
    .bind(target)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Every owner the roster admits is accepted by the database, with the target
/// shape the roster implies. Written over `ReceiptOwner::ALL`, so this is a
/// totality check, not a sample.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn every_roster_owner_is_accepted_with_its_target_shape(pool: PgPool) {
    let (org, actor) = seed_actor(&pool).await;
    for owner in ReceiptOwner::ALL {
        let target = match owner {
            // The pre-existing rows have no dispatch target, and inventing one
            // for them would be a lie in the store.
            ReceiptOwner::OntologyAction => None,
            ReceiptOwner::Canonical(key) => Some(
                DispatchTarget::ALL
                    .iter()
                    .find(|candidate| candidate.object() == *key)
                    .unwrap_or_else(|| panic!("{key:?} has no dispatch target"))
                    .as_str(),
            ),
        };
        insert_receipt(&pool, org, actor, owner.as_str(), target)
            .await
            .unwrap_or_else(|err| panic!("{owner:?} with target {target:?} was refused: {err}"));
    }
}

/// The database refuses what the roster does not admit. Four separate shapes,
/// because one rejection could come from the wrong constraint.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn the_database_refuses_what_the_roster_does_not_admit(pool: PgPool) {
    let (org, actor) = seed_actor(&pool).await;

    assert!(
        insert_receipt(&pool, org, actor, "contractor", None)
            .await
            .is_err(),
        "an owner outside ReceiptOwner::ALL must be refused"
    );
    assert!(
        insert_receipt(&pool, org, actor, "employment", None)
            .await
            .is_err(),
        "a canonical owner with NO target must be refused: the target is what says which action"
    );
    assert!(
        insert_receipt(&pool, org, actor, "ontology.action", Some("hr.appoint"))
            .await
            .is_err(),
        "an ontology.action row must not carry a fabricated dispatch target"
    );
    assert!(
        insert_receipt(&pool, org, actor, "employment", Some("hr.invent"))
            .await
            .is_err(),
        "a target outside DispatchTarget::ALL must be refused"
    );
}

/// A receipt written the old way — no owner, no target — still lands, and lands
/// as `ontology.action`.
///
/// This is what lets 0223 precede the caller edit. The live REST writer at
/// crates/ontology/rest/src/lib.rs names its columns explicitly and supplies
/// neither; if the DEFAULT stopped covering it, that writer would break on the
/// next deploy.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn a_receipt_written_without_owner_defaults_to_ontology_action(pool: PgPool) {
    let (org, actor) = seed_actor(&pool).await;
    sqlx::query(
        "INSERT INTO ont_action_command_receipts \
         (org_id, command_id, actor_id, payload_digest, receipt, created_at) \
         VALUES ($1, $2, $3, decode(repeat('ab', 32), 'hex'), '{}'::jsonb, now())",
    )
    .bind(org)
    .bind(Uuid::new_v4())
    .bind(actor)
    .execute(&pool)
    .await
    .expect("the pre-widening INSERT shape must keep working");

    let (owner, target_is_null): (String, bool) = sqlx::query_as(
        "SELECT owner, target IS NULL FROM ont_action_command_receipts WHERE org_id = $1",
    )
    .bind(org)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(owner, "ontology.action");
    assert!(
        target_is_null,
        "a defaulted receipt must carry no dispatch target"
    );
}

/// A canonical port records its OWN owner and target, not the default.
///
/// `owner` DEFAULTs to `'ontology.action'`, so before the writers passed it
/// explicitly every canonical receipt claimed to belong to the pre-existing
/// instance-action path. That is a wrong attribution recorded as fact — worse
/// than the column not existing, because it reads as an answer.
///
/// Written over `DispatchTarget::ALL` rather than a sample: every dispatch
/// target must be attributable to the object that owns it, so a fourteenth
/// target cannot arrive unattributed.
#[sqlx::test(migrations = "../../platform/db/migrations")]
async fn every_dispatch_target_attributes_to_its_owning_object(pool: PgPool) {
    let (org, actor) = seed_actor(&pool).await;
    for target in DispatchTarget::ALL {
        let owner = ReceiptOwner::Canonical(target.object());
        insert_receipt(&pool, org, actor, owner.as_str(), Some(target.as_str()))
            .await
            .unwrap_or_else(|err| panic!("{target:?} must be storable under {owner:?}: {err}"));
    }
    let mislabelled: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM ont_action_command_receipts \
         WHERE org_id = $1 AND owner = 'ontology.action'",
    )
    .bind(org)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        mislabelled, 0,
        "no canonical receipt may fall back to the instance-action default"
    );
}
