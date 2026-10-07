use super::*;
use console_inbox_adapter_postgres::PgInboxStore;
use console_inbox_application::EmitInboxDocCommand;
use console_inbox_domain::{InboxDocKind, NewInboxDoc};
use console_kernel_core::InboxDocId;

const PAYSLIPS: &str = "/api/v1/me/browser-session/payslips";

async fn emit(f: &Fixture, actor: &Actor, kind: InboxDocKind, index: usize) -> InboxDocId {
    let (run, payload) = issued_payslip_payload();
    console_platform_request_context::scope_org(
        actor.org,
        PgInboxStore::new(f.runtime.clone()).emit_inbox_doc(EmitInboxDocCommand {
            actor: None,
            recipient: actor.user,
            doc: NewInboxDoc::new(
                kind,
                &format!("검증 명세서 {index}"),
                (kind == InboxDocKind::LegalNotice).then_some("연차촉진"),
                None,
                Some("payroll_run"),
                Some(&run.to_string()),
                payload,
            )
            .unwrap(),
            dedup_key: None,
            trace: TraceContext::generate(),
            occurred_at: OffsetDateTime::now_utc(),
        }),
    )
    .await
    .unwrap()
    .id
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn issued_payslips_are_recipient_company_and_kind_scoped_without_read_effects(pool: PgPool) {
    let f = fixture(&pool, "issued-payslips").await;
    let mut a = actor(&pool, &f, "수신인 A").await;
    let mut b = actor_in_org(&pool, &f, "동료 B", a.org).await;
    let mut c = actor(&pool, &f, "다른 법인 C").await;
    let sa = login(&f, &mut a).await;
    let sb = login(&f, &mut b).await;
    let sc = login(&f, &mut c).await;
    let mut ids = Vec::new();
    for index in 0..26 {
        ids.push(emit(&f, &a, InboxDocKind::Payslip, index).await);
    }
    let legal = emit(&f, &a, InboxDocKind::LegalNotice, 99).await;
    let before = effects(&pool).await;
    let before_docs: Value =
        sqlx::query_scalar("SELECT jsonb_agg(to_jsonb(d) ORDER BY id) FROM inbox_docs d")
            .fetch_one(&pool)
            .await
            .unwrap();

    let first = post_raw(f.router.clone(), PAYSLIPS, None, handle(&sa)).await;
    assert_eq!(first.status(), StatusCode::OK);
    assert!(set_cookie_values(&first).is_empty());
    let first = body_json(first).await;
    assert_eq!(first["items"].as_array().unwrap().len(), 25);
    assert_eq!(first["expires_at"], sa["expires_at"]);
    let mut next = handle(&sa);
    next["before"] = first["next_cursor"].clone();
    let second = body_json(post_raw(f.router.clone(), PAYSLIPS, None, next).await).await;
    assert_eq!(second["items"].as_array().unwrap().len(), 1);
    assert!(second["next_cursor"].is_null());
    let mut nonpay = handle(&sa);
    nonpay["before"] = json!(legal);
    let nonpay = body_json(post_raw(f.router.clone(), PAYSLIPS, None, nonpay).await).await;
    assert!(nonpay["items"].as_array().unwrap().is_empty());
    let uri = format!("{PAYSLIPS}/{}", ids[0]);
    let own = body_json(post_raw(f.router.clone(), &uri, None, handle(&sa)).await).await;
    assert_eq!(own["document"]["payload"]["gross_won"], "9007199254740993");
    assert_eq!(own["expires_at"], sa["expires_at"]);
    for (uri, session) in [
        (uri.as_str(), &sb),
        (uri.as_str(), &sc),
        (&format!("{PAYSLIPS}/{legal}"), &sa),
        (&format!("{PAYSLIPS}/{}", InboxDocId::new()), &sa),
    ] {
        let response = post_raw(f.router.clone(), uri, None, handle(session)).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            to_bytes(response.into_body(), 1024).await.unwrap().as_ref(),
            b"document unavailable"
        );
    }
    let empty = body_json(post_raw(f.router.clone(), PAYSLIPS, None, handle(&sb)).await).await;
    assert!(empty["items"].as_array().unwrap().is_empty());
    assert_eq!(
        effects(&pool).await,
        before,
        "reads preserve auth, audit and identity custody"
    );
    let mut hostile = handle(&sa);
    hostile["recipient"] = json!(b.user);
    assert_eq!(
        post_raw(f.router.clone(), PAYSLIPS, None, hostile)
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        post_raw(f.router.clone(), LOGOUT, None, handle(&sa))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        post_raw(f.router.clone(), &uri, None, handle(&sa))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let after_docs: Value =
        sqlx::query_scalar("SELECT jsonb_agg(to_jsonb(d) ORDER BY id) FROM inbox_docs d")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        before_docs, after_docs,
        "neither reads nor logout change issued artifacts or confirm notices"
    );
}

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn malformed_issued_artifacts_fail_unavailable_without_successful_null_projection(
    pool: PgPool,
) {
    let f = fixture(&pool, "malformed-issued-payslip").await;
    let mut a = actor(&pool, &f, "수신인").await;
    let session = login(&f, &mut a).await;
    let id = emit(&f, &a, InboxDocKind::Payslip, 0).await;
    let path = format!("{PAYSLIPS}/{id}");
    sqlx::query("UPDATE inbox_docs SET created_at='0002-01-01 BC'::timestamptz WHERE id=$1")
        .bind(*id.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    for uri in [PAYSLIPS, path.as_str()] {
        assert_eq!(
            post_raw(f.router.clone(), uri, None, handle(&session))
                .await
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
    sqlx::query("UPDATE inbox_docs SET created_at=now(), payload=jsonb_set(payload, '{gross_won}', '\"invalid\"'::jsonb) WHERE id=$1")
        .bind(*id.as_uuid()).execute(&pool).await.unwrap();
    assert_eq!(
        post_raw(f.router.clone(), &path, None, handle(&session))
            .await
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
}
