//! Read-only browser transport for recipient-owned, already-issued artifacts.
//! Inbox owns custody; this adapter neither confirms notices nor issues/pays wages.
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use console_inbox_adapter_postgres::{PgInboxError, PgInboxStore};
use console_inbox_application::GetInboxDocQuery;
use console_kernel_core::{ErrorKind, InboxDocId};
use console_platform_auth_rest::AuthRestState;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

pub const PATH: &str = "/api/v1/me/browser-session/payslips";
pub const DETAIL_PATH: &str = "/api/v1/me/browser-session/payslips/{id}";

pub fn router(store: PgInboxStore, auth: AuthRestState) -> Router {
    console_platform_auth_rest::with_browser_ingress(
        Router::new()
            .route(PATH, post(list))
            .route(DETAIL_PATH, post(detail))
            .layer(DefaultBodyLimit::max(4096))
            .with_state((store, auth.clone())),
        auth,
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListRequest {
    session_token: String,
    browser_context: String,
    before: Option<InboxDocId>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadRequest {
    session_token: String,
    browser_context: String,
}

fn store_error(error: PgInboxError) -> Response {
    let status = if error.kind() == ErrorKind::NotFound {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, "document unavailable").into_response()
}

async fn list(
    State((store, auth)): State<(PgInboxStore, AuthRestState)>,
    Json(body): Json<ListRequest>,
) -> Response {
    let first = match auth
        .resolve_browser_session(&body.session_token, &body.browser_context)
        .await
    {
        Ok(session) => session,
        Err(error) => return error.into_response(),
    };
    let page = match console_platform_request_context::scope_org(
        first.principal.org_id,
        store.list_payslips(first.principal.user_id, body.before, 25),
    )
    .await
    {
        Ok(page) => page,
        Err(error) => return store_error(error),
    };
    let current = match auth
        .resolve_browser_session(&body.session_token, &body.browser_context)
        .await
    {
        Ok(session) => session,
        Err(error) => return error.into_response(),
    };
    if current.principal.org_id != first.principal.org_id
        || current.principal.user_id != first.principal.user_id
        || current.expires_at <= OffsetDateTime::now_utc()
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let items: Result<Vec<_>, _> = page
        .items
        .into_iter()
        .map(|row| {
            row.created_at.format(&time::format_description::well_known::Rfc3339).map(|created_at|
            json!({"id": row.id, "title": row.title, "created_at": created_at}))
        })
        .collect();
    let (Ok(items), Ok(expires_at)) = (
        items,
        current
            .expires_at
            .format(&time::format_description::well_known::Rfc3339),
    ) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "document format unavailable",
        )
            .into_response();
    };
    Json(
        json!({"company_id": current.principal.org_id, "browser_context": current.browser_context,
        "expires_at": expires_at,
        "items": items, "next_cursor": page.next_cursor}),
    )
    .into_response()
}

async fn detail(
    State((store, auth)): State<(PgInboxStore, AuthRestState)>,
    Path(id): Path<InboxDocId>,
    Json(body): Json<ReadRequest>,
) -> Response {
    let first = match auth
        .resolve_browser_session(&body.session_token, &body.browser_context)
        .await
    {
        Ok(session) => session,
        Err(error) => return error.into_response(),
    };
    let doc = match console_platform_request_context::scope_org(
        first.principal.org_id,
        store.get_payslip(GetInboxDocQuery {
            recipient: first.principal.user_id,
            id,
        }),
    )
    .await
    {
        Ok(doc) => doc,
        Err(error) => return store_error(error),
    };
    let payload = match doc.payload.and_then(|value| {
        project(
            value,
            doc.summary.source_kind.as_deref(),
            doc.summary.source_id.as_deref(),
        )
        .ok()
    }) {
        Some(payload) => payload,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "document format unavailable",
            )
                .into_response();
        }
    };
    let current = match auth
        .resolve_browser_session(&body.session_token, &body.browser_context)
        .await
    {
        Ok(session) => session,
        Err(error) => return error.into_response(),
    };
    if current.principal.org_id != first.principal.org_id
        || current.principal.user_id != first.principal.user_id
        || current.expires_at <= OffsetDateTime::now_utc()
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let (Ok(created_at), Ok(expires_at)) = (
        doc.summary
            .created_at
            .format(&time::format_description::well_known::Rfc3339),
        current
            .expires_at
            .format(&time::format_description::well_known::Rfc3339),
    ) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "document format unavailable",
        )
            .into_response();
    };
    Json(
        json!({"company_id": current.principal.org_id, "browser_context": current.browser_context,
        "expires_at": expires_at,
        "document": {"id": id, "title": doc.summary.title,
        "created_at": created_at, "payload": payload}}),
    )
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn payload() -> Value {
        json!({"run_id": Uuid::nil(), "line_id": Uuid::nil(), "period_start": "2026-09-01", "period_end": "2026-09-30",
            "gross_won": 9_007_199_254_740_993_i64, "total_deductions_won": 1, "net_won": 9_007_199_254_740_992_i64,
            "deductions": [{"code": "INCOME_TAX", "label_ko": "소득세", "amount_won": 1, "source_url": "https://www.nts.go.kr/"}],
            "tax_table_version": "test-only", "calculation_version": 1})
    }
    #[test]
    fn issued_money_is_exact_and_source_bound() {
        let source = Uuid::nil().to_string();
        let result = project(payload(), Some("payroll_run"), Some(&source)).unwrap();
        assert_eq!(result["gross_won"], "9007199254740993");
        assert_eq!(result["net_won"], "9007199254740992");
        assert_eq!(result["deductions"][0]["amount_won"], "1");
        assert!(project(payload(), Some("other"), Some(&source)).is_err());
        assert!(
            project(
                payload(),
                Some("payroll_run"),
                Some(&Uuid::new_v4().to_string())
            )
            .is_err()
        );
    }
    #[test]
    fn malformed_issuance_never_becomes_zero_or_a_balanced_statement() {
        let source = Uuid::nil().to_string();
        for (key, value) in [
            ("gross_won", json!(1.0)),
            ("gross_won", json!("1")),
            ("net_won", Value::Null),
            ("net_won", json!(0)),
            ("period_end", json!("2026-02-31")),
            ("period_end", json!("2025-01-01")),
            ("calculation_version", json!(0)),
            ("unknown", json!(1)),
        ] {
            let mut input = payload();
            input[key] = value;
            assert!(
                project(input, Some("payroll_run"), Some(&source)).is_err(),
                "invalid field {key}"
            );
        }
        let mut duplicate = payload();
        let row = duplicate["deductions"][0].clone();
        duplicate["deductions"].as_array_mut().unwrap().push(row);
        assert!(project(duplicate, Some("payroll_run"), Some(&source)).is_err());
        let mut overflow = payload();
        overflow["gross_won"] = json!(i64::MAX);
        overflow["total_deductions_won"] = json!(-1);
        overflow["deductions"][0]["amount_won"] = json!(-1);
        overflow["net_won"] = json!(i64::MIN);
        assert!(project(overflow, Some("payroll_run"), Some(&source)).is_err());
        for (gross, total, net, amount) in [(-1, 0, -1, 0), (1, -1, 2, -1)] {
            let mut negative = payload();
            negative["gross_won"] = json!(gross);
            negative["total_deductions_won"] = json!(total);
            negative["net_won"] = json!(net);
            negative["deductions"][0]["amount_won"] = json!(amount);
            assert!(project(negative, Some("payroll_run"), Some(&source)).is_err());
        }
        let mut negative_net = payload();
        negative_net["gross_won"] = json!(0);
        negative_net["net_won"] = json!(-1);
        assert!(project(negative_net, Some("payroll_run"), Some(&source)).is_ok());
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    run_id: Uuid,
    line_id: Uuid,
    period_start: String,
    period_end: String,
    gross_won: i64,
    total_deductions_won: i64,
    net_won: i64,
    deductions: Vec<Deduction>,
    tax_table_version: String,
    calculation_version: i32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Deduction {
    code: String,
    label_ko: String,
    amount_won: i64,
    source_url: String,
}

#[derive(Serialize)]
struct ProjectedDeduction {
    code: String,
    label_ko: String,
    amount_won: String,
    source_url: String,
}

fn project(value: Value, source_kind: Option<&str>, source_id: Option<&str>) -> Result<Value, ()> {
    let data: Payload = serde_json::from_value(value).map_err(|_| ())?;
    let format = time::macros::format_description!("[year]-[month]-[day]");
    let start = Date::parse(&data.period_start, format).map_err(|_| ())?;
    let end = Date::parse(&data.period_end, format).map_err(|_| ())?;
    if source_kind != Some("payroll_run")
        || source_id != Some(data.run_id.to_string().as_str())
        || start > end
        || data.calculation_version <= 0
        || data.tax_table_version.trim().is_empty()
        || data.gross_won < 0
        || data.total_deductions_won < 0
        || data.deductions.len() > 6
    {
        return Err(());
    }
    let mut codes = std::collections::BTreeSet::new();
    let mut total = 0_i128;
    let mut deductions = Vec::new();
    for row in data.deductions {
        if !matches!(
            row.code.as_str(),
            "NATIONAL_PENSION"
                | "HEALTH_INSURANCE"
                | "LONG_TERM_CARE"
                | "EMPLOYMENT_INSURANCE"
                | "INCOME_TAX"
                | "LOCAL_INCOME_TAX"
        ) || !codes.insert(row.code.clone())
            || row.label_ko.trim().is_empty()
            || row.source_url.trim().is_empty()
            || row.amount_won < 0
        {
            return Err(());
        }
        total += i128::from(row.amount_won);
        deductions.push(ProjectedDeduction {
            code: row.code,
            label_ko: row.label_ko,
            amount_won: row.amount_won.to_string(),
            source_url: row.source_url,
        });
    }
    if total != i128::from(data.total_deductions_won)
        || i128::from(data.gross_won) - total != i128::from(data.net_won)
    {
        return Err(());
    }
    Ok(
        json!({"run_id": data.run_id, "line_id": data.line_id, "period_start": data.period_start,
        "period_end": data.period_end, "gross_won": data.gross_won.to_string(),
        "deductions": deductions, "total_deductions_won": data.total_deductions_won.to_string(),
        "net_won": data.net_won.to_string(), "tax_table_version": data.tax_table_version,
        "calculation_version": data.calculation_version}),
    )
}
