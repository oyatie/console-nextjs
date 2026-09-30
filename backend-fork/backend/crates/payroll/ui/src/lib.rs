//! Payroll `Layer::Ui` surface. SSR HTML for `/_ui`; no payroll math.
use leptos::prelude::*;
use serde::{Deserialize, Serialize};

const PKG_JS: &str = "/_ui/pkg/console_payroll_ui.js";
const PKG_WASM: &str = "/_ui/pkg/console_payroll_ui_bg.wasm";
const ISLAND_BOOTSTRAP: &str = concat!(
    include_str!("island_script.js"),
    "(\"/_ui\", \"pkg\", \"console_payroll_ui\", \"console_payroll_ui_bg\");"
);

/// Contract-shaped run summary for SSR composition. Field names match
/// `PayrollRunSummary.yaml` required keys; values are already-authorized.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunSummary {
    pub id: String,
    pub period_start: String,
    pub period_end: String,
    pub source_label: String,
    pub status: String,
    pub calculation_enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// Contract-shaped org-entity row. Field names match `OrgEntitySummary.yaml`
/// required keys; values are already-authorized.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgEntityView {
    pub org_id: String,
    pub slug: String,
    pub name: String,
    pub status: String,
}

/// Contract-shaped directory person. Field names match `DirectoryPerson.yaml`
/// required keys; values are already-authorized. No phone.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonView {
    pub id: String,
    pub display_name: String,
    pub employee_id: String,
    pub employee_name: String,
    pub employee_number: String,
    pub employee_company: String,
    pub employee_org_unit: String,
    pub employee_position: String,
    pub employee_identity_review_required: String,
    pub employee_identity_resolution_confidence: String,
    pub employee_link_status: String,
    pub team: String,
    pub roles: String,
    pub branch_ids: String,
    pub is_active: String,
    pub has_passkey: String,
    pub account_status: String,
    pub created_at: String,
}

/// One shipping-screen listing after server composition.
///
/// `Omitted` is deny-by-omission (unauth / forbidden). `Empty` is an authorized
/// listing that returned zero rows. `Failure` is an authorized listing that
/// failed. Unauthorized must never become `Empty` or `Failure`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ScreenSection<T> {
    #[default]
    Omitted,
    Empty,
    Failure,
    Rows(Vec<T>),
}

impl<T> ScreenSection<T> {
    /// Map an already-authorized listing. Non-empty rows always win; empty rows
    /// are `Empty` only when the same listing floor would have allowed the GET.
    #[must_use]
    pub fn from_authorized_listing(rows: Vec<T>, listing_authorized: bool) -> Self {
        if !rows.is_empty() {
            Self::Rows(rows)
        } else if listing_authorized {
            Self::Empty
        } else {
            Self::Omitted
        }
    }

    #[must_use]
    pub const fn is_offered(&self) -> bool {
        !matches!(self, Self::Omitted)
    }
}

/// Server-composed shipping screens. `Omitted` is omit, not a client decision.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShippingScreens {
    pub org_entities: ScreenSection<OrgEntityView>,
    pub people: ScreenSection<PersonView>,
    pub runs: ScreenSection<RunSummary>,
}

/// Which `/_ui` body to render. Nav still only names authorized non-empty screens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiScreen {
    Home,
    Organization,
    Hr,
    Payroll,
}

#[component]
pub fn Shell() -> impl IntoView {
    view! {
        <html>
            <head>
                <meta charset="utf-8" />
            </head>
            <body></body>
        </html>
    }
}

#[island]
pub fn AuthorizedRuns(runs: Vec<RunSummary>) -> impl IntoView {
    runs.into_iter()
        .map(|run| {
            let href = format!("/api/v1/payroll/runs/{}", run.id);
            let label = format!(
                "{}–{} {}",
                run.period_start, run.period_end, run.source_label
            );
            view! {
                <a href=href>
                    <span
                        data-run-id=run.id
                        data-period-start=run.period_start
                        data-period-end=run.period_end
                        data-source-label=run.source_label
                        data-status=run.status
                        data-calculation-enabled=run.calculation_enabled.to_string()
                        data-created-at=run.created_at
                        data-updated-at=run.updated_at
                    >
                        {label}
                    </span>
                </a>
            }
        })
        .collect_view()
}

#[component]
fn OrgEntities(entities: Vec<OrgEntityView>) -> impl IntoView {
    entities
        .into_iter()
        .map(|entity| {
            let label = format!("{} ({})", entity.name, entity.slug);
            view! {
                <span
                    data-org-id=entity.org_id
                    data-slug=entity.slug
                    data-name=entity.name
                    data-status=entity.status
                >
                    {label}
                </span>
            }
        })
        .collect_view()
}

#[component]
fn DirectoryPeople(people: Vec<PersonView>) -> impl IntoView {
    people
        .into_iter()
        .map(|person| {
            let label = person.display_name.clone();
            view! {
                <span
                    data-person-id=person.id
                    data-display-name=person.display_name
                    data-employee-id=person.employee_id
                    data-employee-name=person.employee_name
                    data-employee-number=person.employee_number
                    data-employee-company=person.employee_company
                    data-employee-org-unit=person.employee_org_unit
                    data-employee-position=person.employee_position
                    data-employee-identity-review-required=person.employee_identity_review_required
                    data-employee-identity-resolution-confidence=person.employee_identity_resolution_confidence
                    data-employee-link-status=person.employee_link_status
                    data-team=person.team
                    data-roles=person.roles
                    data-branch-ids=person.branch_ids
                    data-is-active=person.is_active
                    data-has-passkey=person.has_passkey
                    data-account-status=person.account_status
                    data-created-at=person.created_at
                >
                    {label}
                </span>
            }
        })
        .collect_view()
}

#[component]
fn ShippingNav(has_org: bool, has_hr: bool, has_payroll: bool) -> impl IntoView {
    view! {
        <nav>
            {has_org.then(|| view! { <a href="/_ui/organization">"조직"</a> })}
            {has_hr.then(|| view! { <a href="/_ui/hr">"인사"</a> })}
            {has_payroll.then(|| view! { <a href="/_ui/payroll">"급여"</a> })}
        </nav>
    }
}

#[component]
pub fn AuthorizedShell(runs: Vec<RunSummary>) -> impl IntoView {
    let nav_payroll = !runs.is_empty();
    view! {
        <ShippingShell
            org=ScreenSection::Omitted
            people=ScreenSection::Omitted
            runs=ScreenSection::from_authorized_listing(runs, false)
            nav_org=false
            nav_hr=false
            nav_payroll=nav_payroll
        />
    }
}

fn org_body(org: ScreenSection<OrgEntityView>) -> impl IntoView {
    match org {
        ScreenSection::Omitted => ().into_any(),
        ScreenSection::Empty => view! {
            <section data-screen="organization" data-state="empty">
                "표시할 조직이 없습니다"
            </section>
        }
        .into_any(),
        ScreenSection::Failure => view! {
            <section data-screen="organization" data-state="failure">
                "목록을 불러오지 못했습니다"
            </section>
        }
        .into_any(),
        ScreenSection::Rows(entities) => view! {
            <section data-screen="organization">
                <OrgEntities entities=entities />
            </section>
        }
        .into_any(),
    }
}

fn hr_body(people: ScreenSection<PersonView>) -> impl IntoView {
    match people {
        ScreenSection::Omitted => ().into_any(),
        ScreenSection::Empty => view! {
            <section data-screen="hr" data-state="empty">
                "표시할 사람이 없습니다"
            </section>
        }
        .into_any(),
        ScreenSection::Failure => view! {
            <section data-screen="hr" data-state="failure">
                "목록을 불러오지 못했습니다"
            </section>
        }
        .into_any(),
        ScreenSection::Rows(people) => view! {
            <section data-screen="hr">
                <DirectoryPeople people=people />
            </section>
        }
        .into_any(),
    }
}

fn payroll_body(runs: ScreenSection<RunSummary>) -> impl IntoView {
    match runs {
        ScreenSection::Omitted => ().into_any(),
        ScreenSection::Empty => view! {
            <section data-screen="payroll" data-state="empty">
                "표시할 급여 이력이 없습니다"
            </section>
        }
        .into_any(),
        ScreenSection::Failure => view! {
            <section data-screen="payroll" data-state="failure">
                "목록을 불러오지 못했습니다"
            </section>
        }
        .into_any(),
        ScreenSection::Rows(runs) => view! {
            <section data-screen="payroll">
                <AuthorizedRuns runs=runs />
            </section>
        }
        .into_any(),
    }
}

#[component]
fn ShippingShell(
    org: ScreenSection<OrgEntityView>,
    people: ScreenSection<PersonView>,
    runs: ScreenSection<RunSummary>,
    nav_org: bool,
    nav_hr: bool,
    nav_payroll: bool,
) -> impl IntoView {
    let hydrate_payroll = matches!(&runs, ScreenSection::Rows(rows) if !rows.is_empty());
    view! {
        <html>
            <head>
                <meta charset="utf-8" />
                {hydrate_payroll.then(|| {
                    view! {
                        <link rel="modulepreload" href=PKG_JS />
                        <link rel="preload" href=PKG_WASM r#as="fetch" r#type="application/wasm" />
                        <script type="module">{ISLAND_BOOTSTRAP}</script>
                    }
                })}
            </head>
            <body>
                <ShippingNav has_org=nav_org has_hr=nav_hr has_payroll=nav_payroll />
                {org_body(org)}
                {hr_body(people)}
                {payroll_body(runs)}
            </body>
        </html>
    }
}

pub fn render_shell() -> String {
    let mut html = String::from("<!DOCTYPE html>");
    html.push_str(&Shell().to_html());
    html
}

pub fn render_shell_with(runs: &[RunSummary]) -> String {
    render_screens(
        &ShippingScreens {
            runs: ScreenSection::from_authorized_listing(runs.to_vec(), false),
            ..ShippingScreens::default()
        },
        UiScreen::Home,
    )
}

/// SSR compose org / HR / payroll. `Omitted` is deny-by-omission.
/// Nav names every offered screen; the body is the focused route.
pub fn render_screens(screens: &ShippingScreens, focus: UiScreen) -> String {
    let nav_org = screens.org_entities.is_offered();
    let nav_hr = screens.people.is_offered();
    let nav_payroll = screens.runs.is_offered();
    let org = match focus {
        UiScreen::Home | UiScreen::Organization => screens.org_entities.clone(),
        UiScreen::Hr | UiScreen::Payroll => ScreenSection::Omitted,
    };
    let people = match focus {
        UiScreen::Home | UiScreen::Hr => screens.people.clone(),
        UiScreen::Organization | UiScreen::Payroll => ScreenSection::Omitted,
    };
    let runs = match focus {
        UiScreen::Home | UiScreen::Payroll => screens.runs.clone(),
        UiScreen::Organization | UiScreen::Hr => ScreenSection::Omitted,
    };
    let focus_denied = match focus {
        UiScreen::Home => !nav_org && !nav_hr && !nav_payroll,
        UiScreen::Organization => !nav_org,
        UiScreen::Hr => !nav_hr,
        UiScreen::Payroll => !nav_payroll,
    };
    if focus_denied {
        return render_shell();
    }
    let mut html = String::from("<!DOCTYPE html>");
    html.push_str(
        &view! {
            <ShippingShell
                org=org
                people=people
                runs=runs
                nav_org=nav_org
                nav_hr=nav_hr
                nav_payroll=nav_payroll
            />
        }
        .to_html(),
    );
    html
}

#[cfg(feature = "ssr")]
mod ssr {
    use super::{
        RunSummary, ShippingScreens, UiScreen, render_screens, render_shell, render_shell_with,
    };
    use axum::Router;
    use axum::http::header;
    use axum::response::{Html, IntoResponse};
    use axum::routing::get;

    pub fn html_shell() -> Html<String> {
        Html(render_shell())
    }

    pub fn html_shell_with(runs: &[RunSummary]) -> Html<String> {
        Html(render_shell_with(runs))
    }

    pub fn html_shell_with_screens(screens: &ShippingScreens, focus: UiScreen) -> Html<String> {
        Html(render_screens(screens, focus))
    }

    pub fn payroll_ui_js() -> &'static [u8] {
        include_bytes!("../pkg/console_payroll_ui.js")
    }

    pub fn payroll_ui_wasm() -> &'static [u8] {
        include_bytes!("../pkg/console_payroll_ui_bg.wasm")
    }

    async fn pkg_js() -> impl IntoResponse {
        (
            [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
            payroll_ui_js(),
        )
    }

    async fn pkg_wasm() -> impl IntoResponse {
        (
            [(header::CONTENT_TYPE, "application/wasm")],
            payroll_ui_wasm(),
        )
    }

    pub fn pkg_router<S>() -> Router<S>
    where
        S: Clone + Send + Sync + 'static,
    {
        Router::new()
            .route("/pkg/console_payroll_ui.js", get(pkg_js))
            .route("/pkg/console_payroll_ui_bg.wasm", get(pkg_wasm))
    }
}

#[cfg(feature = "ssr")]
pub use ssr::{
    html_shell, html_shell_with, html_shell_with_screens, payroll_ui_js, payroll_ui_wasm,
    pkg_router,
};

#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate() {
    leptos::mount::hydrate_islands();
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAYROLL_RUN_SUMMARY_SCHEMA: &str =
        include_str!("../../rest/openapi/schemas/PayrollRunSummary.yaml");
    const ORG_ENTITY_SUMMARY_SCHEMA: &str =
        include_str!("../../../orgchange/rest/openapi/schemas/OrgEntitySummary.yaml");
    const DIRECTORY_PERSON_SCHEMA: &str =
        include_str!("../../../identity/rest/openapi/schemas/DirectoryPerson.yaml");

    fn sample_run() -> RunSummary {
        RunSummary {
            id: "00000000-0000-0000-0000-000000000001".to_owned(),
            period_start: "2026-06-01".to_owned(),
            period_end: "2026-06-30".to_owned(),
            source_label: "workflow_runtime_m2:run:example".to_owned(),
            status: "BLOCKED_LEGAL_GATE".to_owned(),
            calculation_enabled: false,
            created_at: "2026-06-01T00:00:00Z".to_owned(),
            updated_at: "2026-06-01T00:00:00Z".to_owned(),
        }
    }

    fn yaml_required_keys(schema: &str) -> Vec<&str> {
        let mut keys = Vec::new();
        let mut in_required = false;
        for line in schema.lines() {
            let trimmed = line.trim();
            if trimmed == "required:" {
                in_required = true;
                continue;
            }
            if in_required {
                if let Some(key) = trimmed.strip_prefix("- ") {
                    keys.push(key);
                } else if trimmed.ends_with(':') {
                    break;
                }
            }
        }
        keys
    }

    #[test]
    fn empty_runs_match_unauthenticated_shell() {
        assert_eq!(render_shell_with(&[]), render_shell());
        assert!(
            !render_shell().contains("data-run-id"),
            "empty shell must omit run markup: {}",
            render_shell()
        );
        assert!(
            !render_shell().contains("/_ui/pkg/"),
            "empty shell must not load WASM: {}",
            render_shell()
        );
    }

    #[test]
    fn authorized_runs_carry_contract_keys_and_omit_won() {
        let html = render_shell_with(&[sample_run()]);
        let lowered = html.to_ascii_lowercase();
        assert!(
            html.contains("data-run-id=\"00000000-0000-0000-0000-000000000001\""),
            "{html}"
        );
        for key in yaml_required_keys(PAYROLL_RUN_SUMMARY_SCHEMA) {
            let attr = if key == "id" {
                "data-run-id".to_owned()
            } else {
                format!("data-{}", key.replace('_', "-"))
            };
            assert!(
                html.contains(&attr),
                "authorized markup must carry contract key {key} as {attr}: {html}"
            );
        }
        assert!(!lowered.contains("won"), "won leaked: {html}");
        assert!(!html.contains("291_520"), "golden won leaked: {html}");
        assert!(!lowered.contains("payslip"), "payslip leaked: {html}");
        assert!(
            html.contains("rel=\"modulepreload\"") && html.contains(PKG_JS),
            "authorized markup must preload bindgen js: {html}"
        );
        assert!(
            html.contains(PKG_WASM),
            "authorized markup must preload wasm: {html}"
        );
        assert!(
            html.contains("(\"/_ui\", \"pkg\", \"console_payroll_ui\", \"console_payroll_ui_bg\")"),
            "authorized markup must invoke leptos island_script: {html}"
        );
        assert!(
            html.contains("href=\"/api/v1/payroll/runs/00000000-0000-0000-0000-000000000001\""),
            "payroll drill-through must use the existing run GET: {html}"
        );
        assert!(
            html.contains("2026-06-01–2026-06-30 workflow_runtime_m2:run:example"),
            "payroll drill-through must show human-safe period and source_label: {html}"
        );
        assert!(
            !html.contains("/api/v1/employees/") && !html.contains("/api/v1/users/"),
            "must not drill through privileged employee/user GET: {html}"
        );
        let component = island_component(&html).unwrap_or("");
        assert!(
            component.starts_with("AuthorizedRuns_"),
            "island data-component must be the wasm-bindgen export: {html}"
        );
        let js = std::str::from_utf8(payroll_ui_js()).unwrap_or("");
        assert!(
            js.contains(&format!("export function {component}")),
            "committed bindgen js must export {component}"
        );
        assert_eq!(
            &payroll_ui_wasm()[..4],
            b"\0asm",
            "committed wasm must be a Wasm module"
        );
        assert!(
            !render_shell().contains("AuthorizedRuns"),
            "empty shell must not emit an island: {}",
            render_shell()
        );
    }

    fn island_component(html: &str) -> Option<&str> {
        html.split_once("data-component=\"")
            .and_then(|(_, rest)| rest.split_once('"').map(|(id, _)| id))
    }

    fn data_attr(key: &str, id_alias: &str) -> String {
        if key == "id" {
            return format!("data-{id_alias}");
        }
        let mut out = String::from("data-");
        for (i, ch) in key.chars().enumerate() {
            if ch == '_' {
                out.push('-');
            } else if ch.is_ascii_uppercase() {
                if i > 0 {
                    out.push('-');
                }
                out.push(ch.to_ascii_lowercase());
            } else {
                out.push(ch);
            }
        }
        out
    }

    fn sample_org() -> OrgEntityView {
        OrgEntityView {
            org_id: "00000000-0000-0000-0000-0000000000aa".to_owned(),
            slug: "knl".to_owned(),
            name: "KNL".to_owned(),
            status: "ACTIVE".to_owned(),
        }
    }

    fn sample_person() -> PersonView {
        PersonView {
            id: "00000000-0000-0000-0000-0000000000bb".to_owned(),
            display_name: "홍길동".to_owned(),
            employee_id: "00000000-0000-0000-0000-0000000000cc".to_owned(),
            employee_name: "홍길동".to_owned(),
            employee_number: "E-1".to_owned(),
            employee_company: "KNL".to_owned(),
            employee_org_unit: "본사".to_owned(),
            employee_position: "사원".to_owned(),
            employee_identity_review_required: "false".to_owned(),
            employee_identity_resolution_confidence: "HIGH".to_owned(),
            employee_link_status: "LINKED".to_owned(),
            team: "MANAGEMENT".to_owned(),
            roles: "MEMBER".to_owned(),
            branch_ids: String::new(),
            is_active: "true".to_owned(),
            has_passkey: "false".to_owned(),
            account_status: "PENDING_SETUP".to_owned(),
            created_at: "2026-06-01T00:00:00Z".to_owned(),
        }
    }

    fn assert_shipping_invariants(html: &str) {
        let lowered = html.to_ascii_lowercase();
        assert!(!lowered.contains("won"), "won leaked: {html}");
        assert!(!html.contains("291_520"), "golden won leaked: {html}");
        assert!(!lowered.contains("payslip"), "payslip leaked: {html}");
        assert!(
            !lowered.contains("group-switcher") && !lowered.contains("data-group-switch"),
            "Group switcher is not admitted unless it only displays authorized scope: {html}"
        );
        assert!(
            !lowered.contains("comms-rail") && !html.contains("data-comms"),
            "comms rail is out of this slice: {html}"
        );
        assert!(
            !html.contains("type=\"file\"")
                && !lowered.contains("import/export")
                && !html.contains("자료실"),
            "import/export is not the data-entry base: {html}"
        );
        assert!(
            !html.contains("webpack") && !html.contains("vite") && !html.contains("innerHTML"),
            "must stay Rust-native Leptos SSR, not a JS wrapper: {html}"
        );
    }

    #[test]
    fn shipping_screens_empty_matches_unauthenticated_shell() {
        let empty = ShippingScreens::default();
        assert_eq!(render_screens(&empty, UiScreen::Home), render_shell());
        assert_eq!(
            render_screens(&empty, UiScreen::Organization),
            render_shell()
        );
        assert_eq!(render_screens(&empty, UiScreen::Hr), render_shell());
        assert_eq!(render_screens(&empty, UiScreen::Payroll), render_shell());
        assert!(
            !render_shell().contains("data-screen="),
            "empty shell must omit screen markup: {}",
            render_shell()
        );
        assert!(
            !render_shell().contains("href=\"/_ui/"),
            "empty shell must omit nav: {}",
            render_shell()
        );
        assert_shipping_invariants(&render_shell());

        let authorized_empty = ShippingScreens {
            org_entities: ScreenSection::Empty,
            people: ScreenSection::Empty,
            runs: ScreenSection::Empty,
        };
        let empty_org = render_screens(&authorized_empty, UiScreen::Organization);
        assert_ne!(
            empty_org,
            render_shell(),
            "authorized-empty org must not reuse the silent deny shell"
        );
        assert!(
            empty_org.contains("data-screen=\"organization\"")
                && empty_org.contains("data-state=\"empty\"")
                && empty_org.contains("표시할 조직이 없습니다"),
            "authorized-empty org must mount Korean empty copy: {empty_org}"
        );
        assert!(
            !empty_org.contains("data-org-id"),
            "authorized-empty must not leak rows: {empty_org}"
        );
        assert_shipping_invariants(&empty_org);

        let empty_pay = render_screens(&authorized_empty, UiScreen::Payroll);
        assert_ne!(empty_pay, render_shell(), "{empty_pay}");
        assert!(
            empty_pay.contains("data-screen=\"payroll\"")
                && empty_pay.contains("data-state=\"empty\"")
                && empty_pay.contains("표시할 급여 이력이 없습니다")
                && !empty_pay.contains("/_ui/pkg/")
                && !empty_pay.contains("data-run-id"),
            "authorized-empty payroll is SSR empty copy, not omit and not WASM: {empty_pay}"
        );
        assert_eq!(
            render_screens(&ShippingScreens::default(), UiScreen::Payroll),
            render_shell(),
            "unauthorized payroll must stay deny-by-omission"
        );

        let failed = ShippingScreens {
            org_entities: ScreenSection::Failure,
            people: ScreenSection::Omitted,
            runs: ScreenSection::Failure,
        };
        let fail_html = render_screens(&failed, UiScreen::Payroll);
        assert!(
            fail_html.contains("data-screen=\"payroll\"")
                && fail_html.contains("data-state=\"failure\"")
                && fail_html.contains("목록을 불러오지 못했습니다")
                && !fail_html.contains("data-run-id")
                && !fail_html.contains("/_ui/pkg/"),
            "listing failure marks the section without object ids: {fail_html}"
        );
        assert_shipping_invariants(&fail_html);
    }

    #[test]
    fn shipping_screens_organization_is_ssr_contracts_and_omits_wasm() {
        let screens = ShippingScreens {
            org_entities: ScreenSection::Rows(vec![sample_org()]),
            ..ShippingScreens::default()
        };
        let html = render_screens(&screens, UiScreen::Organization);
        assert_ne!(html, render_shell(), "{html}");
        assert!(
            html.contains("data-screen=\"organization\""),
            "organization body must be a mounted SSR screen: {html}"
        );
        for key in yaml_required_keys(ORG_ENTITY_SUMMARY_SCHEMA) {
            let attr = data_attr(key, "org-id");
            assert!(
                html.contains(&attr),
                "org markup must carry contract key {key} as {attr}: {html}"
            );
        }
        assert!(
            html.contains("data-org-id=\"00000000-0000-0000-0000-0000000000aa\""),
            "{html}"
        );
        assert!(
            html.contains("KNL (knl)"),
            "org row must show human-safe name/slug: {html}"
        );
        assert!(
            !html.contains("/api/v1/org-entities/")
                && !html.contains("/api/v1/employees/")
                && !html.contains("/api/v1/users/"),
            "org has no same-capability object GET; must not invent privileged hrefs: {html}"
        );
        assert!(
            html.contains("href=\"/_ui/organization\""),
            "authorized org nav is SSR: {html}"
        );
        assert!(
            !html.contains("href=\"/_ui/hr\"") && !html.contains("href=\"/_ui/payroll\""),
            "nav must omit unauthorized/empty screens: {html}"
        );
        assert!(
            !html.contains("/_ui/pkg/"),
            "org read projection is SSR, not an island: {html}"
        );
        assert!(
            island_component(&html).is_none(),
            "org screen must not emit an island: {html}"
        );
        assert_shipping_invariants(&html);
    }

    #[test]
    fn shipping_screens_hr_is_ssr_contracts_and_omits_phone() {
        let screens = ShippingScreens {
            people: ScreenSection::Rows(vec![sample_person()]),
            ..ShippingScreens::default()
        };
        let html = render_screens(&screens, UiScreen::Hr);
        assert_ne!(html, render_shell(), "{html}");
        assert!(
            html.contains("data-screen=\"hr\""),
            "HR body must be a mounted SSR screen: {html}"
        );
        for key in yaml_required_keys(DIRECTORY_PERSON_SCHEMA) {
            let attr = data_attr(key, "person-id");
            assert!(
                html.contains(&attr),
                "HR markup must carry contract key {key} as {attr}: {html}"
            );
        }
        assert!(
            html.contains("data-person-id=\"00000000-0000-0000-0000-0000000000bb\""),
            "{html}"
        );
        assert!(
            html.contains("홍길동"),
            "HR row must show human-safe display_name: {html}"
        );
        assert!(
            !html.contains("/api/v1/employees/") && !html.contains("/api/v1/users/"),
            "HR must not drill through privileged employee/user GET: {html}"
        );
        assert!(
            html.contains("href=\"/_ui/hr\""),
            "authorized HR nav is SSR: {html}"
        );
        let lowered = html.to_ascii_lowercase();
        assert!(!lowered.contains("phone"), "directory phone leaked: {html}");
        assert!(
            !html.contains("/_ui/pkg/"),
            "HR read projection is SSR, not an island: {html}"
        );
        assert!(
            island_component(&html).is_none(),
            "HR screen must not emit an island: {html}"
        );
        assert_shipping_invariants(&html);
    }

    #[test]
    fn shipping_screens_home_composes_authorized_sections_only() {
        let screens = ShippingScreens {
            org_entities: ScreenSection::Rows(vec![sample_org()]),
            people: ScreenSection::Rows(vec![sample_person()]),
            runs: ScreenSection::Rows(vec![sample_run()]),
        };
        let html = render_screens(&screens, UiScreen::Home);
        assert!(html.contains("data-screen=\"organization\""), "{html}");
        assert!(html.contains("data-screen=\"hr\""), "{html}");
        assert!(html.contains("data-screen=\"payroll\""), "{html}");
        assert!(html.contains("href=\"/_ui/organization\""), "{html}");
        assert!(html.contains("href=\"/_ui/hr\""), "{html}");
        assert!(html.contains("href=\"/_ui/payroll\""), "{html}");
        assert!(
            html.contains("data-run-id=\"00000000-0000-0000-0000-000000000001\""),
            "{html}"
        );
        let component = island_component(&html).unwrap_or("");
        assert!(
            component.starts_with("AuthorizedRuns_"),
            "payroll interaction stays the AuthorizedRuns island: {html}"
        );
        assert!(
            html.contains("/_ui/pkg/console_payroll_ui.js"),
            "WASM hydrate only when the payroll island is composed: {html}"
        );
        assert_shipping_invariants(&html);

        let payroll_only = render_screens(
            &ShippingScreens {
                runs: ScreenSection::Rows(vec![sample_run()]),
                ..ShippingScreens::default()
            },
            UiScreen::Payroll,
        );
        assert!(
            payroll_only.contains("data-screen=\"payroll\""),
            "{payroll_only}"
        );
        assert!(
            !payroll_only.contains("data-screen=\"organization\"")
                && !payroll_only.contains("data-screen=\"hr\""),
            "focused payroll must omit other bodies: {payroll_only}"
        );
        assert_shipping_invariants(&payroll_only);
    }

    #[test]
    fn shipping_screens_focus_keeps_authorized_nav_and_omits_wasm_off_payroll() {
        let screens = ShippingScreens {
            org_entities: ScreenSection::Rows(vec![sample_org()]),
            people: ScreenSection::Rows(vec![sample_person()]),
            runs: ScreenSection::Rows(vec![sample_run()]),
        };

        let org_html = render_screens(&screens, UiScreen::Organization);
        assert!(
            org_html.contains("data-screen=\"organization\""),
            "{org_html}"
        );
        assert!(
            !org_html.contains("data-screen=\"hr\"")
                && !org_html.contains("data-screen=\"payroll\""),
            "focused org must omit other bodies: {org_html}"
        );
        assert!(
            org_html.contains("href=\"/_ui/organization\""),
            "{org_html}"
        );
        assert!(org_html.contains("href=\"/_ui/hr\""), "{org_html}");
        assert!(org_html.contains("href=\"/_ui/payroll\""), "{org_html}");
        assert!(
            !org_html.contains("/_ui/pkg/"),
            "org body must not hydrate WASM: {org_html}"
        );
        assert_shipping_invariants(&org_html);

        let payroll_html = render_screens(&screens, UiScreen::Payroll);
        assert!(
            payroll_html.contains("data-screen=\"payroll\""),
            "{payroll_html}"
        );
        assert!(
            !payroll_html.contains("data-screen=\"organization\"")
                && !payroll_html.contains("data-screen=\"hr\""),
            "focused payroll must omit other bodies: {payroll_html}"
        );
        assert!(
            payroll_html.contains("href=\"/_ui/organization\"")
                && payroll_html.contains("href=\"/_ui/hr\"")
                && payroll_html.contains("href=\"/_ui/payroll\""),
            "authorized nav must stay reachable from a focused screen: {payroll_html}"
        );
        assert!(
            payroll_html.contains("/_ui/pkg/console_payroll_ui.js"),
            "{payroll_html}"
        );
        assert_shipping_invariants(&payroll_html);

        let denied_payroll = render_screens(
            &ShippingScreens {
                org_entities: ScreenSection::Rows(vec![sample_org()]),
                ..ShippingScreens::default()
            },
            UiScreen::Payroll,
        );
        assert_eq!(
            denied_payroll,
            render_shell(),
            "unauthorized payroll route must omit, not leak sibling nav"
        );
    }
}
