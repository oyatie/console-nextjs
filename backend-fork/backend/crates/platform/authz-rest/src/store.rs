//! Postgres persistence for the Cedar Policy Studio.
//!
//! Every mutation flows through `with_audit` (mutation + audit row in one tx) and
//! every read through `with_org_conn`, so `app.current_org` is armed before any
//! statement and RLS scopes it to the tenant. Draft writes respect the `0103`
//! CHECKs: a draft can never carry a `shadow`/`enforced` status or a
//! bundle/policy version — promotion is a separate, gated lane.
//!
//! ONE EXCEPTION, and it is stricter rather than looser:
//! [`PgCedarPolicyStore::attach_object_policy`] uses `with_org_conn` on a separate
//! `console_ontology_cmd` pool, because migration 0206 writes its audit row inside
//! the definer — the credential that can attach cannot skip it, which an
//! application-appended row could never guarantee.
//!
//! `ponytail:` the store lives in the thin `authz-rest` crate (its only consumer)
//! rather than a separate 4-crate hexagon — one vertical slice, no speculative
//! ports. Split it out if a second consumer ever appears.

use console_kernel_core::{AuditAction, AuditEvent, KernelError, TraceContext, UserId};
use console_platform_authz::cedar_pbac::authoring::{
    self, AuthoredPolicy, DeclaredAttr, DraftValidation, NoCodeBlocks, ReviewDecision,
    ReviewStatus, SimRequest, SimulationOutcome,
};
use console_platform_db::{DbError, with_audit, with_org_conn};
use console_platform_request_context::current_org;
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, PgPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum PgCedarError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Domain(#[from] KernelError),
    /// The audited attach path needs the `console_ontology_cmd` pool and this
    /// store was not wired with one. Returned by [`PgCedarPolicyStore::command_pool`],
    /// which is deliberately `ok_or` and never `.unwrap_or(&self.pool)`: the
    /// fallback compiles, reads as defensive, and restores the exact capability
    /// 0206 removes while every other test stays green.
    #[error("cedar policy command database capability is unavailable")]
    CommandUnavailable,
}

impl From<sqlx::Error> for PgCedarError {
    fn from(value: sqlx::Error) -> Self {
        Self::Db(DbError::Sqlx(value))
    }
}

#[derive(Debug, Clone)]
pub struct PgCedarPolicyStore {
    pool: PgPool,
    /// The `console_ontology_cmd` credential, the only role migration 0206 leaves
    /// able to execute `ont_policy_api.attach_object_policy`. `None` on every
    /// read-only and draft-only construction, and the attach path then fails
    /// closed rather than reaching for `pool` — see [`PgCedarError::CommandUnavailable`].
    command_pool: Option<PgPool>,
}

// -- DTOs --------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize)]
pub struct CatalogEntry {
    pub id: Uuid,
    pub stable_key: String,
    pub title: String,
    pub effect: String,
    pub status: String,
    pub source: String,
    pub validation_status: String,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DraftRecord {
    pub id: Uuid,
    pub draft_key: String,
    pub title: String,
    pub normalized_row: serde_json::Value,
    pub generated_policy_text: String,
    pub validation_status: String,
    pub validation_errors: serde_json::Value,
    pub review_status: String,
    pub reviewer_id: Option<Uuid>,
    pub created_by: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

pub struct CreateDraftCommand {
    pub actor: UserId,
    pub draft_key: String,
    pub title: String,
    pub author_note: Option<String>,
    pub blocks: NoCodeBlocks,
}

pub struct UpdateDraftCommand {
    pub actor: UserId,
    pub draft_id: Uuid,
    pub title: Option<String>,
    pub author_note: Option<String>,
    pub blocks: NoCodeBlocks,
}

pub struct ReviewDraftCommand {
    pub reviewer: UserId,
    pub draft_id: Uuid,
    pub decision: ReviewDecision,
    pub note: Option<String>,
}

/// One org-authored object policy, on its way to becoming an enforced catalog
/// row plus the attachment that binds it to an object-type version.
///
/// `declared` MUST be the same declared-attribute set the read path derives for
/// that object type: `load_enforced_object_policy_blocks` re-validates and
/// re-normalizes every row on every read and hard-errors on any disagreement
/// (see the canonicality checks below), so an attach validated against a
/// different set would 500 every later read of the type.
///
/// It carries NO `stable_key`, `title` or `natural_language_rule`: all three are
/// functions of the object type the definer resolves for itself, so migration
/// 0205 generates them. A caller cannot forge a value it cannot supply, and a
/// hand-crafted call to the definer therefore cannot mint a catalog row this
/// route would have labelled differently. It carries no generated Cedar text
/// either — 0205 stores NULL rather than accept a value nothing can re-derive.
pub struct AttachObjectPolicyCommand {
    pub actor: UserId,
    /// The object-type VERSION id the attachment is filed under, matching the
    /// `ont_object_policies.object_type_id` the read path queries.
    pub object_type_id: Uuid,
    pub blocks: NoCodeBlocks,
    pub declared: Vec<DeclaredAttr>,
}

fn digest(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

/// `target_type` is a parameter rather than the hardcoded `"cedar_policy_draft"`
/// it used to be: an object-policy attachment writes no draft row, and naming
/// one in the audit trail would be a lie about what was mutated.
fn audit_event(
    action: &str,
    target_type: &'static str,
    actor: UserId,
    target_id: impl ToString,
) -> Result<AuditEvent, KernelError> {
    Ok(AuditEvent::new(
        Some(actor),
        AuditAction::new(action)?,
        target_type,
        target_id.to_string(),
        TraceContext::generate(),
        time::OffsetDateTime::now_utc(),
    ))
}

/// The audit `target_type` for the four Cedar draft verbs.
const DRAFT_TARGET_TYPE: &str = "cedar_policy_draft";

impl PgCedarPolicyStore {
    /// Construct a store whose object-policy ATTACH path fails closed. Every
    /// other verb here reads or writes draft/catalog/decision-log tables the
    /// runtime role owns outright, so `pool` alone is a complete store for them.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            command_pool: None,
        }
    }

    /// Attach the `console_ontology_cmd` credential that migration 0206 made the
    /// only executor of the attach definer.
    #[must_use]
    pub fn with_command_pool(mut self, command_pool: PgPool) -> Self {
        self.command_pool = Some(command_pool);
        self
    }

    /// The READ pool, deliberately: `authz-rest/src/lib.rs` resolves principals
    /// with it and the command credential holds no SELECT on `users`.
    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Copied literally from `PgOntologyStore::command_pool`
    /// (`ontology/adapter-postgres/src/lib.rs`). NEVER `.unwrap_or(&self.pool)`.
    fn command_pool(&self) -> Result<&PgPool, PgCedarError> {
        self.command_pool
            .as_ref()
            .ok_or(PgCedarError::CommandUnavailable)
    }

    // -- §5a catalog -------------------------------------------------------

    /// List catalog entries, optionally filtered by `status`. Read-only; the
    /// runtime role has SELECT on the catalog but never INSERT/UPDATE/DELETE.
    pub async fn list_catalog(
        &self,
        status: Option<String>,
    ) -> Result<Vec<CatalogEntry>, PgCedarError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_, _, PgCedarError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let rows = sqlx::query(
                    r#"
                    SELECT id, stable_key, title, effect, status, source,
                           validation_status, updated_at
                    FROM cedar_policy_catalog_entries
                    WHERE ($1::text IS NULL OR status = $1)
                    ORDER BY updated_at DESC
                    "#,
                )
                .bind(status)
                .fetch_all(tx.as_mut())
                .await?;
                Ok(rows.iter().map(catalog_from_row).collect())
            })
        })
        .await
    }

    /// Attach one org-authored object policy to an object type, as one enforced
    /// catalog row plus its attachment, audited in the same transaction.
    ///
    /// The runtime role deliberately still has NO INSERT on the catalog
    /// (`0150:117-118`) and, since migration 0206, NO EXECUTE on the definer
    /// either. The two policy rows AND the audit row are written by
    /// `ont_policy_api.attach_object_policy`, a SECURITY DEFINER routine owned by
    /// the NOBYPASSRLS `console_ontology_writer` role, so the RLS org floor still
    /// applies to every one of them and the residual can only narrow it. The only
    /// credential that may execute it is `console_ontology_cmd`, reached through
    /// [`Self::command_pool`] — an unwired store fails closed here rather than
    /// falling back to `pool`, which is the capability 0206 removes.
    ///
    /// `with_org_conn` and not `with_audit`: the audit row is no longer appended
    /// by the application. It is one INSERT inside the definer, after the policy
    /// rows and in the same transaction, so an attach and its audit claim still
    /// commit or roll back together — and the command credential cannot write it
    /// itself (it holds no INSERT on `audit_events`, and no EXECUTE on the inner
    /// `attach_object_policy_rows`, so it cannot skip it either).
    ///
    /// Validation is the authoring validator's verdict, never re-encoded here:
    /// an invalid policy is a `Validation` error and nothing is written.
    pub async fn attach_object_policy(
        &self,
        command: AttachObjectPolicyCommand,
    ) -> Result<Uuid, PgCedarError> {
        let org = current_org().map_err(KernelError::from)?;
        let validation = authoring::validate_blocks_with(org, &command.blocks, &command.declared);
        if !validation.valid {
            return Err(PgCedarError::Domain(KernelError::validation(
                validation.errors.join("; "),
            )));
        }
        // Generated here exactly as `audit_event` generated it per call before the
        // row moved into the definer, so the trace/span the audit row carries are
        // unchanged in origin as well as in shape.
        let trace = TraceContext::generate();
        let actor = *command.actor.as_uuid();
        let org_uuid = *org.as_uuid();
        // All three `effect` writes derive from one value: the loader rejects any
        // disagreement between blocks, catalog and attachment.
        let effect = command.blocks.effect.as_str();

        with_org_conn::<_, Uuid, PgCedarError>(self.command_pool()?, org, move |tx| {
            Box::pin(async move {
                let policy_id: Uuid = sqlx::query_scalar(
                    "SELECT ont_policy_api.attach_object_policy($1,$2,$3,$4,$5,$6,$7,$8)",
                )
                .bind(org_uuid)
                .bind(actor)
                .bind(command.object_type_id)
                .bind(effect)
                // The CANONICAL normalized row, never `to_value(&blocks)`: the
                // read path re-derives it and rejects any non-canonical row, so
                // the raw JSON would pass this write and 500 every later read.
                // 0205 also checks its three scalars against `effect` and against
                // the object type it resolves, so a hand-crafted call cannot mint
                // an inert row either.
                //
                // `validation.generated_policy_text` is deliberately NOT sent and
                // 0205 no longer accepts it: see the note on
                // [`Self::load_enforced_policies`]. Enforcement re-derives
                // everything from this row.
                .bind(&validation.normalized_row)
                .bind(authoring::AUTHORING_SCHEMA_VERSION)
                .bind(trace.trace_id())
                .bind(trace.span_id())
                .fetch_one(tx.as_mut())
                .await?;
                Ok(policy_id)
            })
        })
        .await
    }

    // -- §5a drafts --------------------------------------------------------

    /// Create a no-code draft. `review_status` is forced to `draft` and the
    /// normalized row carries no status/version — the `0103` CHECKs make a
    /// shadow/enforced draft impossible; this is the app-side belt.
    pub async fn create_draft(
        &self,
        command: CreateDraftCommand,
    ) -> Result<DraftRecord, PgCedarError> {
        let org = current_org().map_err(KernelError::from)?;
        let validation = authoring::validate_blocks(org, &command.blocks);
        let draft_id = Uuid::new_v4();
        let event = audit_event(
            "cedar.draft.create",
            DRAFT_TARGET_TYPE,
            command.actor,
            draft_id,
        )?
        .with_org(org)
        .with_snapshots(None, Some(validation.normalized_row.clone()));
        let actor = *command.actor.as_uuid();
        let org_uuid = *org.as_uuid();

        with_audit::<_, DraftRecord, PgCedarError>(&self.pool, event, move |tx| {
            Box::pin(async move {
                sqlx::query(
                    r#"
                    INSERT INTO cedar_policy_drafts
                        (id, org_id, draft_key, title, author_note, blocks, normalized_row,
                         generated_policy_text, generated_policy_digest,
                         validation_status, validation_errors, review_status,
                         created_by, updated_by)
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, 'draft', $12, $12)
                    "#,
                )
                .bind(draft_id)
                .bind(org_uuid)
                .bind(command.draft_key.trim())
                .bind(command.title.trim())
                .bind(command.author_note.as_deref())
                .bind(serde_json::to_value(&command.blocks).map_err(DbError::Serialize)?)
                .bind(&validation.normalized_row)
                .bind(&validation.generated_policy_text)
                .bind(digest(&validation.generated_policy_text))
                .bind(validation_status_str(&validation))
                .bind(errors_json(&validation))
                .bind(actor)
                .execute(tx.as_mut())
                .await?;
                draft_row_conn(tx.as_mut(), draft_id).await
            })
        })
        .await
    }

    /// Edit a draft (per-user invisible draft, benchmark §3e). Re-validates the
    /// new blocks and RESETS `review_status` to `draft` — an edit invalidates any
    /// prior submission so a changed policy is re-reviewed from scratch.
    pub async fn update_draft(
        &self,
        command: UpdateDraftCommand,
    ) -> Result<DraftRecord, PgCedarError> {
        let org = current_org().map_err(KernelError::from)?;
        let validation = authoring::validate_blocks(org, &command.blocks);
        let event = audit_event(
            "cedar.draft.update",
            DRAFT_TARGET_TYPE,
            command.actor,
            command.draft_id,
        )?
        .with_org(org)
        .with_snapshots(None, Some(validation.normalized_row.clone()));
        let actor = *command.actor.as_uuid();
        let draft_id = command.draft_id;

        with_audit::<_, DraftRecord, PgCedarError>(&self.pool, event, move |tx| {
            Box::pin(async move {
                let updated = sqlx::query(
                    r#"
                    UPDATE cedar_policy_drafts
                    SET title                   = COALESCE($2, title),
                        author_note             = $3,
                        blocks                  = $4,
                        normalized_row          = $5,
                        generated_policy_text   = $6,
                        generated_policy_digest = $7,
                        validation_status       = $8,
                        validation_errors       = $9,
                        review_status           = 'draft',
                        reviewer_id             = NULL,
                        review_note             = NULL,
                        updated_by              = $10,
                        updated_at              = now()
                    WHERE id = $1
                    "#,
                )
                .bind(draft_id)
                .bind(command.title.as_deref().map(str::trim))
                .bind(command.author_note.as_deref())
                .bind(serde_json::to_value(&command.blocks).map_err(DbError::Serialize)?)
                .bind(&validation.normalized_row)
                .bind(&validation.generated_policy_text)
                .bind(digest(&validation.generated_policy_text))
                .bind(validation_status_str(&validation))
                .bind(errors_json(&validation))
                .bind(actor)
                .execute(tx.as_mut())
                .await?;
                if updated.rows_affected() == 0 {
                    return Err(KernelError::not_found("draft was not found").into());
                }
                draft_row_conn(tx.as_mut(), draft_id).await
            })
        })
        .await
    }

    pub async fn get_draft(&self, draft_id: Uuid) -> Result<DraftRecord, PgCedarError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_, _, PgCedarError>(&self.pool, org, move |tx| {
            Box::pin(async move { draft_row_conn(tx.as_mut(), draft_id).await })
        })
        .await
    }

    pub async fn list_drafts(&self) -> Result<Vec<DraftRecord>, PgCedarError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_, _, PgCedarError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let rows = sqlx::query(DRAFT_SELECT_ALL).fetch_all(tx.as_mut()).await?;
                rows.iter().map(draft_from_row).collect()
            })
        })
        .await
    }

    /// Re-run strict validation on the stored blocks and persist the result.
    /// Returns errors without activating anything.
    pub async fn validate_draft(
        &self,
        actor: UserId,
        draft_id: Uuid,
    ) -> Result<DraftRecord, PgCedarError> {
        let org = current_org().map_err(KernelError::from)?;
        let event =
            audit_event("cedar.draft.validate", DRAFT_TARGET_TYPE, actor, draft_id)?.with_org(org);
        with_audit::<_, DraftRecord, PgCedarError>(&self.pool, event, move |tx| {
            Box::pin(async move {
                let blocks: NoCodeBlocks =
                    serde_json::from_value(blocks_json(tx.as_mut(), draft_id).await?)
                        .map_err(DbError::Serialize)?;
                let validation = authoring::validate_blocks(org, &blocks);
                sqlx::query(
                    r#"
                    UPDATE cedar_policy_drafts
                    SET generated_policy_text   = $2,
                        generated_policy_digest = $3,
                        validation_status       = $4,
                        validation_errors       = $5,
                        updated_at              = now()
                    WHERE id = $1
                    "#,
                )
                .bind(draft_id)
                .bind(&validation.generated_policy_text)
                .bind(digest(&validation.generated_policy_text))
                .bind(validation_status_str(&validation))
                .bind(errors_json(&validation))
                .execute(tx.as_mut())
                .await?;
                draft_row_conn(tx.as_mut(), draft_id).await
            })
        })
        .await
    }

    /// Submit a draft for review. Guarded by [`authoring::submit_draft`]
    /// (validation must be `valid`, current status must allow submit); the DB
    /// CHECK `review_status <> 'review_pending' OR validation = 'valid'` is the
    /// backstop.
    pub async fn submit_draft(
        &self,
        actor: UserId,
        draft_id: Uuid,
    ) -> Result<DraftRecord, PgCedarError> {
        let org = current_org().map_err(KernelError::from)?;
        let event =
            audit_event("cedar.draft.submit", DRAFT_TARGET_TYPE, actor, draft_id)?.with_org(org);
        with_audit::<_, DraftRecord, PgCedarError>(&self.pool, event, move |tx| {
            Box::pin(async move {
                let current = draft_row_conn(tx.as_mut(), draft_id).await?;
                let status = ReviewStatus::from_db_str(&current.review_status)?;
                let valid = current.validation_status == "valid";
                let next = authoring::submit_draft(status, valid)?;
                sqlx::query(
                    "UPDATE cedar_policy_drafts SET review_status = $2, updated_at = now() WHERE id = $1",
                )
                .bind(draft_id)
                .bind(next.as_db_str())
                .execute(tx.as_mut())
                .await?;
                draft_row_conn(tx.as_mut(), draft_id).await
            })
        })
        .await
    }

    /// Four-eyes review. Guarded by [`authoring::review_draft`] — the reviewer
    /// MUST differ from the draft author, and the draft must be `review_pending`.
    /// Approval yields `approved_for_promotion`, never a live/shadow row.
    pub async fn review_draft(
        &self,
        command: ReviewDraftCommand,
    ) -> Result<DraftRecord, PgCedarError> {
        let org = current_org().map_err(KernelError::from)?;
        let event = audit_event(
            "cedar.draft.review",
            DRAFT_TARGET_TYPE,
            command.reviewer,
            command.draft_id,
        )?
        .with_org(org);
        let reviewer = command.reviewer;
        let draft_id = command.draft_id;
        let decision = command.decision;
        let note = command.note;

        with_audit::<_, DraftRecord, PgCedarError>(&self.pool, event, move |tx| {
            Box::pin(async move {
                let current = draft_row_conn(tx.as_mut(), draft_id).await?;
                let status = ReviewStatus::from_db_str(&current.review_status)?;
                let author = UserId::from_uuid(current.created_by);
                let next = authoring::review_draft(status, decision, author, reviewer)?;
                sqlx::query(
                    r#"
                    UPDATE cedar_policy_drafts
                    SET review_status = $2, reviewer_id = $3, review_note = $4, updated_at = now()
                    WHERE id = $1
                    "#,
                )
                .bind(draft_id)
                .bind(next.as_db_str())
                .bind(*reviewer.as_uuid())
                .bind(note.as_deref())
                .execute(tx.as_mut())
                .await?;
                draft_row_conn(tx.as_mut(), draft_id).await
            })
        })
        .await
    }

    // -- §5c live authorize (object / property policy) ---------------------

    /// Live object-policy decision: load the catalog policies attached to
    /// `object_type_id` and evaluate row visibility for `request` — the SAME
    /// [`authoring::simulate`] evaluator the `/policy/simulate` what-if uses.
    pub async fn authorize_object_row(
        &self,
        object_type_id: Uuid,
        request: &SimRequest,
    ) -> Result<SimulationOutcome, PgCedarError> {
        let policies = self
            .load_attached_policies(OBJECT_POLICY_SELECT, object_type_id)
            .await?;
        Ok(authoring::simulate(&policies, request))
    }

    /// Load the enforced no-code blocks attached to an ontology object type.
    /// The ontology read path lowers these into the SQL residual under the
    /// already-armed RLS tenant floor; malformed persisted rows are a hard error
    /// rather than an optimistic unfiltered read.
    /// `declared` are the object type's own property definitions, supplied by the
    /// ontology caller. The execution layer has always lowered conditions against
    /// arbitrary instance attributes; passing the declared set lets the authoring
    /// validator admit the same properties instead of only the four generic
    /// `Resource` attributes, while Cedar still strict-validates every read.
    pub async fn load_enforced_object_policy_blocks(
        &self,
        object_type_id: Uuid,
        declared: &[DeclaredAttr],
    ) -> Result<Vec<NoCodeBlocks>, PgCedarError> {
        let org = current_org().map_err(KernelError::from)?;
        let declared = declared.to_vec();
        with_org_conn::<_, _, PgCedarError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let rows = sqlx::query(
                    r#"
                    SELECT a.effect AS attachment_effect,
                           c.id,
                           c.effect,
                           c.validation_status,
                           c.normalized_row
                    FROM ont_object_policies a
                    JOIN cedar_policy_catalog_entries c
                      ON c.id = a.cedar_policy_id AND c.org_id = a.org_id
                    WHERE a.object_type_id = $1
                      AND c.status = 'enforced'
                    "#,
                )
                .bind(object_type_id)
                .fetch_all(tx.as_mut())
                .await?;
                rows.iter()
                    .map(|row| {
                        let attachment_effect: String = row.try_get("attachment_effect")?;
                        let policy_id: Uuid = row.try_get("id")?;
                        let catalog_effect: String = row.try_get("effect")?;
                        let validation_status: String = row.try_get("validation_status")?;
                        let value: serde_json::Value = row.try_get("normalized_row")?;
                        let blocks: NoCodeBlocks = serde_json::from_value(value.clone()).map_err(|error| {
                            PgCedarError::Domain(KernelError::validation(format!(
                                "invalid enforced object policy row {policy_id}: {error}"
                            )))
                        })?;
                        let validation = authoring::validate_blocks_with(org, &blocks, &declared);
                        if validation_status != "valid" || !validation.valid {
                            return Err(PgCedarError::Domain(KernelError::validation(format!(
                                "enforced object policy row {policy_id} is not valid"
                            ))));
                        }
                        if validation.normalized_row != value {
                            return Err(PgCedarError::Domain(KernelError::validation(format!(
                                "enforced object policy row {policy_id} is not canonical"
                            ))));
                        }
                        if blocks.effect.as_str() != catalog_effect
                            || catalog_effect != attachment_effect
                        {
                            return Err(PgCedarError::Domain(KernelError::validation(format!(
                                "enforced object policy row {policy_id} effect does not match attachment and catalog"
                            ))));
                        }
                        Ok(blocks)
                    })
                    .collect()
            })
        })
        .await
    }

    /// Live property-policy decision for `property_def_id`.
    pub async fn authorize_property_field(
        &self,
        property_def_id: Uuid,
        request: &SimRequest,
    ) -> Result<SimulationOutcome, PgCedarError> {
        let policies = self
            .load_attached_policies(PROPERTY_POLICY_SELECT, property_def_id)
            .await?;
        Ok(authoring::simulate(&policies, request))
    }

    /// Load the enforced catalog policy set (those carrying generated Cedar text)
    /// for the org — the live set behind `/policy/authorize`.
    ///
    /// Object-policy attachments are EXCLUDED, twice over.
    ///
    /// `generated_policy_text` is the one parameter
    /// `ont_policy_api.attach_object_policy` can neither derive nor re-validate
    /// (rendering Cedar in SQL would be a second copy of
    /// `generate_cedar_text_with` living in a migration, and bounding the text
    /// with LIKE predicates does not bound it — a condition literal may
    /// legitimately contain `;`). So 0205 does not take it: an attached row stores
    /// NULL, which the `IS NOT NULL` predicate below already excludes, and which
    /// `OBJECT_POLICY_SELECT` excludes on the type-scoped path for the same
    /// reason. That path — `POST /policy/authorize` carrying an `object_type_id`
    /// — is NOT a separate concern from this one: it is the same endpoint reading
    /// the same forgeable column, and excluding only the org-wide set left a
    /// definer call choosing the Cedar that decided it (measured `Allow` for a
    /// principal owning nothing, `object_policy_attach_as_runtime_role.rs`).
    ///
    /// The `NOT EXISTS` clause below therefore now only bites rows attached some
    /// OTHER way — i.e. before 0205 revoked `console_rt`'s INSERT on
    /// `ont_object_policies`. It is kept as depth, not because it is redundant.
    /// It is NOT a pure narrowing: dropping a `forbid` row from the org-wide set
    /// widens every decision that row would have blocked. That is acceptable only
    /// because the population it can reach is legacy attachments, and the
    /// alternative is honouring Cedar text nothing can re-derive.
    ///
    /// [`Self::load_enforced_object_policy_blocks`] is untouched by any of this:
    /// it re-derives every policy from `normalized_row` and never reads the text.
    pub async fn load_enforced_policies(&self) -> Result<Vec<AuthoredPolicy>, PgCedarError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_, _, PgCedarError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let rows = sqlx::query(
                    r#"
                    SELECT c.id, c.generated_policy_text
                    FROM cedar_policy_catalog_entries c
                    WHERE c.status = 'enforced'
                      AND c.generated_policy_text IS NOT NULL
                      AND NOT EXISTS (
                          SELECT 1 FROM ont_object_policies a
                          WHERE a.cedar_policy_id = c.id AND a.org_id = c.org_id
                      )
                    "#,
                )
                .fetch_all(tx.as_mut())
                .await?;
                Ok(rows.iter().map(authored_from_row).collect())
            })
        })
        .await
    }

    async fn load_attached_policies(
        &self,
        sql: &'static str,
        fk_value: Uuid,
    ) -> Result<Vec<AuthoredPolicy>, PgCedarError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_, _, PgCedarError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let rows = sqlx::query(sql)
                    .bind(fk_value)
                    .fetch_all(tx.as_mut())
                    .await?;
                Ok(rows.iter().map(authored_from_row).collect())
            })
        })
        .await
    }

    /// Append the point-decisions an authorize call just computed to the tenant's
    /// append-only `cedar_decision_log` (the Integrity feed source). One tx, all
    /// rows or none. The log IS the audit record here (append-only + FORCE-RLS),
    /// so no second `audit_events` row is emitted per decision.
    // ponytail: one INSERT per decision under one conn; batch into a single
    // multi-row INSERT if the bulk feed ever gets hot.
    pub async fn record_decisions(
        &self,
        actor: Uuid,
        entries: Vec<DecisionLogEntry>,
    ) -> Result<(), PgCedarError> {
        if entries.is_empty() {
            return Ok(());
        }
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        with_org_conn::<_, (), PgCedarError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                for entry in &entries {
                    let determining = serde_json::Value::Array(
                        entry
                            .determining_policies
                            .iter()
                            .map(|p| serde_json::Value::String(p.clone()))
                            .collect(),
                    );
                    sqlx::query(
                        r#"
                        INSERT INTO cedar_decision_log (
                            org_id, actor, subject_ref, action, resource_type,
                            resource_id, effect, determining_policies, reason
                        )
                        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                        "#,
                    )
                    .bind(org_uuid)
                    .bind(actor)
                    .bind(&entry.subject_ref)
                    .bind(&entry.action)
                    .bind(&entry.resource_type)
                    .bind(entry.resource_id.as_deref())
                    .bind(&entry.effect)
                    .bind(determining)
                    .bind(&entry.reason)
                    .execute(tx.as_mut())
                    .await?;
                }
                Ok(())
            })
        })
        .await
    }

    /// Recent decisions for the tenant, newest first, optionally since a cursor
    /// instant. RLS-scoped; `limit` is capped by the caller.
    pub async fn recent_decisions(
        &self,
        since: Option<OffsetDateTime>,
        limit: i64,
    ) -> Result<Vec<DecisionLogRow>, PgCedarError> {
        let org = current_org().map_err(KernelError::from)?;
        with_org_conn::<_, Vec<DecisionLogRow>, PgCedarError>(&self.pool, org, move |tx| {
            Box::pin(async move {
                let rows = sqlx::query(
                    r#"
                    SELECT id, decided_at, subject_ref, action, resource_type,
                           resource_id, effect, determining_policies, reason
                    FROM cedar_decision_log
                    WHERE ($1::timestamptz IS NULL OR decided_at > $1)
                    ORDER BY decided_at DESC
                    LIMIT $2
                    "#,
                )
                .bind(since)
                .bind(limit)
                .fetch_all(tx.as_mut())
                .await?;
                rows.iter().map(decision_row_from_row).collect()
            })
        })
        .await
    }
}

/// One decision to append to `cedar_decision_log`.
#[derive(Debug, Clone)]
pub struct DecisionLogEntry {
    pub subject_ref: String,
    pub action: String,
    pub resource_type: String,
    pub resource_id: Option<String>,
    /// `"allow"` | `"deny"` (matches the table CHECK).
    pub effect: String,
    pub determining_policies: Vec<String>,
    pub reason: String,
}

/// A recorded decision, as read back for the Integrity feed.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DecisionLogRow {
    pub id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub decided_at: OffsetDateTime,
    pub subject_ref: String,
    pub action: String,
    pub resource_type: String,
    pub resource_id: Option<String>,
    pub effect: String,
    pub determining_policies: Vec<String>,
    pub reason: String,
}

fn decision_row_from_row(row: &sqlx::postgres::PgRow) -> Result<DecisionLogRow, PgCedarError> {
    let determining: serde_json::Value = row.try_get("determining_policies")?;
    let determining_policies = determining
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    Ok(DecisionLogRow {
        id: row.try_get("id")?,
        decided_at: row.try_get("decided_at")?,
        subject_ref: row.try_get("subject_ref")?,
        action: row.try_get("action")?,
        resource_type: row.try_get("resource_type")?,
        resource_id: row.try_get("resource_id")?,
        effect: row.try_get("effect")?,
        determining_policies,
        reason: row.try_get("reason")?,
    })
}

const OBJECT_POLICY_SELECT: &str = r#"
    SELECT c.id AS id, c.generated_policy_text AS generated_policy_text
    FROM ont_object_policies a
    JOIN cedar_policy_catalog_entries c
      ON c.id = a.cedar_policy_id AND c.org_id = a.org_id
    WHERE a.object_type_id = $1 AND c.generated_policy_text IS NOT NULL
"#;

const PROPERTY_POLICY_SELECT: &str = r#"
    SELECT c.id AS id, c.generated_policy_text AS generated_policy_text
    FROM ont_property_policies a
    JOIN cedar_policy_catalog_entries c
      ON c.id = a.cedar_policy_id AND c.org_id = a.org_id
    WHERE a.property_def_id = $1 AND c.generated_policy_text IS NOT NULL
"#;

const DRAFT_SELECT_BY_ID: &str = r#"
    SELECT id, draft_key, title, normalized_row, generated_policy_text,
           validation_status, validation_errors, review_status, reviewer_id,
           created_by, created_at, updated_at
    FROM cedar_policy_drafts WHERE id = $1
"#;

const DRAFT_SELECT_ALL: &str = r#"
    SELECT id, draft_key, title, normalized_row, generated_policy_text,
           validation_status, validation_errors, review_status, reviewer_id,
           created_by, created_at, updated_at
    FROM cedar_policy_drafts ORDER BY updated_at DESC
"#;

async fn draft_row_conn(conn: &mut PgConnection, id: Uuid) -> Result<DraftRecord, PgCedarError> {
    let row = sqlx::query(DRAFT_SELECT_BY_ID)
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or_else(|| KernelError::not_found("draft was not found"))?;
    draft_from_row(&row)
}

async fn blocks_json(conn: &mut PgConnection, id: Uuid) -> Result<serde_json::Value, PgCedarError> {
    let row = sqlx::query("SELECT blocks FROM cedar_policy_drafts WHERE id = $1")
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or_else(|| KernelError::not_found("draft was not found"))?;
    Ok(row.try_get("blocks")?)
}

fn draft_from_row(row: &sqlx::postgres::PgRow) -> Result<DraftRecord, PgCedarError> {
    Ok(DraftRecord {
        id: row.try_get("id")?,
        draft_key: row.try_get("draft_key")?,
        title: row.try_get("title")?,
        normalized_row: row.try_get("normalized_row")?,
        generated_policy_text: row.try_get("generated_policy_text")?,
        validation_status: row.try_get("validation_status")?,
        validation_errors: row.try_get("validation_errors")?,
        review_status: row.try_get("review_status")?,
        reviewer_id: row.try_get("reviewer_id")?,
        created_by: row.try_get("created_by")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn catalog_from_row(row: &sqlx::postgres::PgRow) -> CatalogEntry {
    CatalogEntry {
        id: row.get("id"),
        stable_key: row.get("stable_key"),
        title: row.get("title"),
        effect: row.get("effect"),
        status: row.get("status"),
        source: row.get("source"),
        validation_status: row.get("validation_status"),
        updated_at: row.get("updated_at"),
    }
}

fn authored_from_row(row: &sqlx::postgres::PgRow) -> AuthoredPolicy {
    let id: Uuid = row.get("id");
    let text: String = row.get("generated_policy_text");
    AuthoredPolicy::new(id.to_string(), text)
}

fn validation_status_str(validation: &DraftValidation) -> &'static str {
    if validation.valid { "valid" } else { "invalid" }
}

fn errors_json(validation: &DraftValidation) -> serde_json::Value {
    serde_json::Value::Array(
        validation
            .errors
            .iter()
            .map(|e| serde_json::Value::String(e.clone()))
            .collect(),
    )
}

#[cfg(test)]
mod serde_tests {
    use super::*;

    // Regression guard (R9 policy HARD FAIL): `time::OffsetDateTime` with serde's
    // default impl serializes as a numeric `[year, ordinal, hour, …]` array, which
    // the web client's `str(row, "updated_at")` parser rejects → the whole 권한·정책
    // surface collapsed to its error banner. Every timestamp the policy REST reads
    // back MUST serialize as an RFC3339 string (the openapi/client contract).
    #[test]
    fn policy_timestamps_serialize_as_rfc3339_strings() {
        let now = OffsetDateTime::now_utc();

        let catalog = CatalogEntry {
            id: Uuid::nil(),
            stable_key: "policy.wo_view".into(),
            title: "Work order view".into(),
            effect: "permit".into(),
            status: "enforced".into(),
            source: "seed".into(),
            validation_status: "valid".into(),
            updated_at: now,
        };
        let value = serde_json::to_value(&catalog).expect("catalog serializes");
        assert!(
            value["updated_at"].is_string(),
            "catalog updated_at must be a string, got {}",
            value["updated_at"]
        );

        let draft = DraftRecord {
            id: Uuid::nil(),
            draft_key: "policy.x".into(),
            title: "Draft".into(),
            normalized_row: serde_json::json!({}),
            generated_policy_text: "permit(principal, action, resource);".into(),
            validation_status: "valid".into(),
            validation_errors: serde_json::json!([]),
            review_status: "draft".into(),
            reviewer_id: None,
            created_by: Uuid::nil(),
            created_at: now,
            updated_at: now,
        };
        let value = serde_json::to_value(&draft).expect("draft serializes");
        assert!(
            value["created_at"].is_string(),
            "draft created_at must be a string"
        );
        assert!(
            value["updated_at"].is_string(),
            "draft updated_at must be a string"
        );
    }
}
