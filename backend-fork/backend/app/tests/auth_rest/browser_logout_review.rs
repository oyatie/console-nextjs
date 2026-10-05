//! Supplemental regression probes for independent integration review findings.
//! The original approved tests and assertions remain intact.
use super::*;

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn recovered_family_logout_audit_failure_rolls_back_then_reconciles_once(pool: PgPool) {
    let f = fixture(&pool, "browser-recovered-logout-audit").await;
    let mut subject = actor(&pool, &f, "복구 후 종료").await;
    give_recovery_subject_branch(&pool, &subject).await;
    let mut admin = actor_in_org(&pool, &f, "독립 복구 담당", subject.org).await;
    sqlx::query("UPDATE users SET roles=ARRAY['SUPER_ADMIN'] WHERE id=$1")
        .bind(*admin.user.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    let session = login(&f, &mut subject).await;
    let administrator = login(&f, &mut admin).await;
    let bearer = private_original_proof(&pool, &f, &administrator).await;
    assert_eq!(
        post_raw(
            f.router.clone(),
            "/api/v1/auth/admin/credential-reset",
            Some(&bearer),
            json!({"user_id":subject.user.as_uuid()})
        )
        .await
        .status(),
        StatusCode::OK
    );
    let bound = family(&pool, &session).await;
    let prior: (OffsetDateTime, String) = sqlx::query_as(
        "SELECT revoked_at,revoked_reason FROM auth_refresh_token_families WHERE id=$1",
    )
    .bind(bound)
    .fetch_one(&pool)
    .await
    .unwrap();
    let before = effects(&pool).await;
    sqlx::raw_sql("CREATE FUNCTION public.test_recovered_logout_failure() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.action='auth.logout' THEN RAISE EXCEPTION 'forced recovered logout failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER test_recovered_logout_failure BEFORE INSERT ON public.audit_events FOR EACH ROW EXECUTE FUNCTION public.test_recovered_logout_failure();")
        .execute(&pool).await.unwrap();
    assert_eq!(
        post_raw(f.router.clone(), LOGOUT, None, handle(&session))
            .await
            .status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(
        effects(&pool).await,
        before,
        "failed audit must not close or rewrite revoked custody"
    );
    sqlx::query("DROP TRIGGER test_recovered_logout_failure ON public.audit_events")
        .execute(&pool)
        .await
        .unwrap();
    let immutable = immutable_mapping(&pool, &session).await;
    assert_eq!(
        post_raw(f.router.clone(), LOGOUT, None, handle(&session))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_closed_mapping(&pool, &session, immutable).await;
    let after: (OffsetDateTime, String) = sqlx::query_as(
        "SELECT revoked_at,revoked_reason FROM auth_refresh_token_families WHERE id=$1",
    )
    .bind(bound)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        after, prior,
        "explicit logout must preserve prior recovery facts"
    );
    let audited_reason: String = sqlx::query_scalar("SELECT after_snap->>'revoked_reason' FROM audit_events WHERE action='auth.logout' AND target_id=$1")
        .bind(bound.to_string()).fetch_one(&pool).await.unwrap();
    assert_eq!(
        audited_reason, prior.1,
        "audit must describe the persisted revocation reason"
    );
    let committed = effects(&pool).await;
    assert_eq!(
        post_raw(f.router.clone(), LOGOUT, None, handle(&session))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(effects(&pool).await, committed);
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE action='auth.logout' AND target_id=$1",
    )
    .bind(bound.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audits, 1);
    own_history(&f, &administrator).await;
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn cleanup_isolates_company_failure_then_retries_without_starving_other_custody(
    pool: PgPool,
) {
    let mut f = fixture(&pool, "browser-cleanup-fault").await;
    let mut a = actor(&pool, &f, "정리 실패 회사").await;
    let mut b = actor(&pool, &f, "정리 계속 회사").await;
    f.router = configured_router_with(
        f.runtime.clone(),
        &f.private,
        &f.public,
        Some(&f.storage_key),
        &[("CONSOLE_REFRESH_FAMILY_ABSOLUTE_TTL_SECS", "6".into())],
    );
    let one = login(&f, &mut a).await;
    let two = login(&f, &mut b).await;
    let deadline = [&one, &two]
        .into_iter()
        .map(|session| {
            OffsetDateTime::parse(
                session["expires_at"].as_str().unwrap(),
                &time::format_description::well_known::Rfc3339,
            )
            .unwrap()
        })
        .max()
        .unwrap();
    sqlx::query(
        "SELECT pg_sleep(greatest(0,extract(epoch FROM ($1::timestamptz-clock_timestamp())))+0.1)",
    )
    .bind(deadline)
    .execute(&pool)
    .await
    .unwrap();
    let (failed, healthy) = if a.org.as_uuid() < b.org.as_uuid() {
        (a.org, b.org)
    } else {
        (b.org, a.org)
    };
    assert!(
        failed.as_uuid() < healthy.as_uuid(),
        "fault must precede healthy scope in discovery"
    );
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "CREATE FUNCTION public.test_cleanup_failure() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF OLD.company_id='{}'::uuid AND OLD.ciphertext IS NOT NULL AND NEW.ciphertext IS NULL THEN RAISE EXCEPTION 'forced scoped cleanup failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER test_cleanup_failure BEFORE UPDATE ON auth_security.browser_sessions FOR EACH ROW EXECUTE FUNCTION public.test_cleanup_failure();",
        failed.as_uuid(),
    ))).execute(&pool).await.unwrap();
    let identity = custody_digest(&pool, true).await;
    prepare_real_worker_storage(&pool).await;
    let mut worker = RealWorker::start(&pool, "cleanup-fault-isolation").await;
    wait_for_proof_count(&pool, healthy, 0, &mut [&mut worker]).await;
    let pending: i64 = sqlx::query_scalar("SELECT count(*) FROM auth_security.browser_sessions WHERE company_id=$1 AND ciphertext IS NOT NULL")
        .bind(*failed.as_uuid()).fetch_one(&pool).await.unwrap();
    assert_eq!(pending, 1, "failed scope must roll back its proof clearing");
    assert_eq!(custody_digest(&pool, true).await, identity);
    sqlx::query("DROP TRIGGER test_cleanup_failure ON auth_security.browser_sessions")
        .execute(&pool)
        .await
        .unwrap();
    wait_for_proof_count(&pool, failed, 0, &mut [&mut worker]).await;
    assert_eq!(
        custody_digest(&pool, true).await,
        identity,
        "retry must only clear expired proof"
    );
    worker.require_alive();
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn closed_mapping_without_family_or_owner_removal_cannot_certify_retry(pool: PgPool) {
    let f = fixture(&pool, "browser-closed-parent-loss").await;
    let mut subject = actor(&pool, &f, "종료 후 부모 증거 상실").await;
    let session = login(&f, &mut subject).await;
    let other = login(&f, &mut subject).await;
    let immutable = immutable_mapping(&pool, &session).await;
    assert_eq!(
        post_raw(f.router.clone(), LOGOUT, None, handle(&session))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_closed_mapping(&pool, &session, immutable).await;
    let mapping_before = complete_mapping_digest(&pool, &session).await;
    let bound = family(&pool, &session).await;
    assert_eq!(
        sqlx::query("DELETE FROM public.auth_refresh_token_families WHERE id=$1")
            .bind(bound)
            .execute(&pool)
            .await
            .unwrap()
            .rows_affected(),
        1
    );
    let before = effects(&pool).await;
    let response = post_raw(f.router.clone(), LOGOUT, None, handle(&session)).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(set_cookie_values(&response).is_empty());
    assert_eq!(effects(&pool).await, before);
    assert_eq!(
        complete_mapping_digest(&pool, &session).await,
        mapping_before
    );
    own_history(&f, &other).await;
}
