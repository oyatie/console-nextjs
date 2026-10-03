//! `console-app` composition root.
//!
//! This crate owns the process boundary: 12-factor configuration, health and
//! readiness endpoints, telemetry, database dependency wiring, and graceful
//! shutdown. Domain behavior lands in narrower crates and is composed here.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::collections::{BTreeSet, HashMap};
use std::env;
use std::fmt::{Display, Formatter};
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Extension, MatchedPath, Query, State};
use axum::http::{HeaderMap, Request, Response, StatusCode, header};
use axum::middleware::Next;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use base64::Engine as _;
use console_analytics_quant_rest::AnalyticsQuantState;
use console_attendance_adapter_postgres::PgAttendanceStore;
use console_attendance_rest::AttendanceRestState;
use console_benefit_adapter_postgres::PgBenefitCatalogStore;
use console_benefit_rest::BenefitRestState;
use console_comms_adapter_postgres::PgMailStore;
use console_comms_credential_cipher::EnvelopeCredentialCipher;
use console_comms_rest::CommsRestState;
use console_compliance_adapter_postgres::PgComplianceStore;
use console_compliance_rest::ComplianceRestState;
use console_consulting_rest::ConsultingRestState;
use console_dispatch_adapter_postgres::PgDispatchStore;
use console_dispatch_domain::DispatchTimerConfig;
use console_dispatch_rest::DispatchRestState;
use console_dispatch_worker::{AlimtalkEscalationPolicy, DispatchWorker};
use console_docs_adapter_postgres::PgDocsStore;
use console_docs_rest::DocsRestState;
use console_equipment_adapter_postgres::PgEquipment3rStore;
use console_equipment_rest::EquipmentRestState;
use console_evaluation_adapter_postgres::PgEvaluationStore;
use console_evaluation_rest::EvaluationRestState;
use console_facilities_rest::FacilitiesRestState;
use console_finance_gl_adapter_postgres::PgVoucherStore;
use console_finance_gl_rest::FinanceGlRestState;
use console_financial_adapter_postgres::PgFinancialStore;
use console_financial_rest::FinancialRestState;
use console_governance_adapter_postgres::PgGovernanceStore;
use console_governance_rest::GovernanceRestState;
use console_identity_adapter_postgres::PgOrgStore;
use console_identity_rest::IdentityRestState;
use console_inbox_adapter_postgres::PgInboxStore;
use console_inbox_rest::InboxRestState;
use console_inspection_adapter_postgres::PgInspectionStore;
use console_inspection_rest::InspectionRestState;
use console_integrity::{IntegrityRestState, PgIntegrityStore};
use console_inventory_adapter_postgres::PgInventoryStore;
use console_inventory_rest::InventoryRestState;
use console_kernel_core::{
    AuditAction, AuditEvent, BranchId, BranchScope, EquipmentId, ErrorKind, EvidenceId,
    KernelError, OrgId, TraceContext, UserId,
};
use console_logistics_adapter_postgres::PgLogisticsStore;
use console_logistics_rest::LogisticsRestState;
use console_messenger_adapter_postgres::PgMessengerStore;
use console_messenger_rest::MessengerRestState;
use console_notices_adapter_postgres::PgNoticeStore;
use console_notices_rest::NoticeRestState;
use console_notifications_adapter_postgres::PgNotificationStore;
use console_notifications_rest::NotificationRestState;
use console_ontology_adapter_postgres::PgOntologyStore;
use console_ontology_adapter_postgres::instances::PgInstanceStore;
use console_ontology_canonical_adapter_postgres::company::PgCompanyPort;
use console_ontology_canonical_adapter_postgres::employment::{
    EmploymentError, PgEmploymentPort, reassign_org_unit_via_transfers_in_tx,
};
use console_ontology_canonical_adapter_postgres::job_position::PgJobPositionPort;
use console_ontology_canonical_adapter_postgres::org_unit::PgOrgUnitPort;
use console_ontology_canonical_adapter_postgres::person::PgPersonPort;
use console_ontology_rest::{
    ActionError, OntologyRestState, ProjectedDispatch, ProjectedDispatchRegistry, ProjectedHandler,
};
use console_orgchange_adapter_postgres::{
    EmploymentTransferPort, PgOrgChangeStore, TransferPortFuture,
};
use console_orgchange_rest::OrgChangeRestState;
use console_payroll_adapter_postgres::PgPayrollStore;
use console_payroll_adapter_postgres::pay_run::PgPayRunPort;
use console_payroll_rest::PayrollRestState;
use console_platform_audit_chain::{ChainReport, SealConfig, SealSigner, verify_org_chain};
use console_platform_auth::{
    AndroidAssetLinksConfig, AppleAppSiteAssociationConfig, JwtIssuer, JwtSettings, JwtVerifier,
    PasskeyService, WELL_KNOWN_AASA_PATH, WELL_KNOWN_ASSETLINKS_PATH, WebauthnSettings,
    android_assetlinks_json, apple_app_site_association_json,
};
use console_platform_auth_rest::{AuthRestConfig, AuthRestState};
use console_platform_authz::{
    Action, Feature, PermissionLevel, Principal, Role, authorize_capability, authorize_org_wide,
    permission_for,
};
use console_platform_authz_rest::{CedarPolicyRestState, PgCedarPolicyStore};
use console_platform_db::{DbError, with_audit};
use console_platform_email::{
    DisabledEmailSender, EmailSender, LettreSmtpSender, SmtpEmailConfig, StubEmailMode,
    StubEmailSender,
};
use console_platform_jobs::{
    ApalisPostgresJobQueue, BoxFuture, JobQueue, JobQueueError, PlatformJob, PlatformJobHandler,
    migrate_and_reconcile_apalis_postgres, run_apalis_worker_until_shutdown,
};
use console_platform_provisioning::{BootstrapCredentialStore, PlatformProvisioner};
use console_platform_push::{
    FcmConfig, FcmHttpV1Client, ProviderPushNotifier, PushNotifier, SolapiAlimtalkClient,
    SolapiConfig,
};
use console_platform_realtime::{
    PgRealtimeHub, PostgresBridgeHandle, PostgresMessageNotifier, PostgresNotificationNotifier,
    RealtimeRestState,
};
use console_platform_rest::PlatformRestState;
use console_platform_storage::{
    EvidenceService, FfmpegMediaProcessor, S3ObjectStore, S3StorageConfig, SeaweedS3Storage,
    StorageError,
};
use console_production_rest::ProductionRestState;
use console_recruiting_adapter_postgres::PgRecruitingStore;
use console_recruiting_rest::RecruitingRestState;
use console_registry_adapter_postgres::{PgRegistryError, PgRegistryStore};
use console_registry_application::{UpdateEquipmentCommand, UpdateEquipmentFields};
use console_registry_domain::EquipmentStatus;
use console_registry_rest::RegistryRestState;
use console_reporting_adapter_postgres::PgKpiRepository;
use console_reporting_rest::KpiRestState;
use console_sales_adapter_postgres::PgSalesStore;
use console_sales_rest::SalesRestState;
use console_support_adapter_postgres::PgSupportStore;
use console_support_rest::SupportRestState;
use console_todos_adapter_postgres::PgTodoStore;
use console_todos_rest::TodoRestState;
use console_workorder_adapter_postgres::PgWorkOrderStore;
use console_workorder_rest::{MobileRestState, WorkOrderRestState};
use ipnet::IpNet;
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use opentelemetry::global;
use opentelemetry::trace::{TraceContextExt, TracerProvider};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::SdkTracerProvider;
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgConnection, PgPool, Postgres, QueryBuilder};
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;
use tracing::Instrument;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use url::Url;

pub mod action_inbox;
mod audit_chain_signer;
pub mod cedar_parity;
mod collaboration;
mod console_telemetry;
mod facilities_schedule;
mod hr;
pub mod lifecycle;
mod mail_sync;
pub mod objects;
pub mod office;
mod recruiting_hire;
mod workbench;
mod workbench_native;
mod workflow_drain;
mod workflow_object_context;
pub mod workflow_schedules;
mod workflow_studio;

const DEFAULT_HTTP_ADDR: &str = "0.0.0.0:8080";
const DEFAULT_SERVICE_NAME: &str = "console-app";
const DEFAULT_SHUTDOWN_TIMEOUT_SECS: u64 = 10;
// Blue/green may temporarily run four API pods and the worker rolling update
// two workers. Six runtime connections per process, plus the API's two
// 2-connection command pools, caps that surge at 52 and reserves eight of
// PostgreSQL's configured 60 for migration/topology/operator work.
const RUNTIME_DATABASE_POOL_MAX_CONNECTIONS: u32 = 6;
// These role-backed defaults are an operational correctness backstop for every
// serving pool. They limit accidental/buggy work; they are not a security
// boundary against a caller that has already compromised a database credential.
const SERVING_STATEMENT_TIMEOUT: &str = "30s";
const SERVING_IDLE_IN_TRANSACTION_SESSION_TIMEOUT: &str = "30s";
const SERVING_TRANSACTION_TIMEOUT: &str = "45s";
const DEFAULT_EVIDENCE_TRANSCODE_CONCURRENCY: usize = 2;
const DEFAULT_JWT_ISSUER: &str = "console-platform-auth";
const DEFAULT_JWT_AUDIENCE: &str = "console-api";
const DEFAULT_WEBAUTHN_RP_NAME: &str = "Console";
const DEFAULT_AUTH_CEREMONY_TTL_SECS: u64 = 300;
const DEFAULT_REFRESH_TOKEN_TTL_SECS: u64 = 60 * 60 * 24 * 30;
/// Absolute refresh-family lifetime cap (NIST 800-63B AAL2). Default 24h: past
/// this the family is revoked on rotation and a fresh primary auth is required.
const DEFAULT_REFRESH_FAMILY_ABSOLUTE_TTL_SECS: u64 = 60 * 60 * 24;
const DEFAULT_COLDSTART_OTP_TTL_SECS: u64 = 3600;
const DEFAULT_DISPATCH_ACCEPT_WINDOW_SECS: u64 = 5 * 60;
const DEFAULT_DISPATCH_FORCE_ASSIGN_ALERT_SECS: u64 = 10 * 60;
const DEFAULT_DISPATCH_ALIMTALK_NO_ACK_SECS: u64 = 2 * 60;
const DEFAULT_DISPATCH_GPS_FRESHNESS_SECS: u64 = 15 * 60;
const DEFAULT_FCM_TOKEN_URI: &str = "https://oauth2.googleapis.com/token";
const DEFAULT_FCM_SCOPE: &str = "https://www.googleapis.com/auth/firebase.messaging";
const DEFAULT_SOLAPI_BASE_URL: &str = "https://api.solapi.com";
const EMAIL_STUB_MODE_ENV: &str = "CONSOLE_EMAIL_STUB_MODE";
const DEFAULT_AUDIT_LIMIT: i64 = 50;
const MAX_AUDIT_LIMIT: i64 = 200;
/// Global request-body cap. Modest by design: the JSON APIs here carry small
/// payloads, and large evidence uploads go straight to object storage via
/// presigned URLs rather than through this process. Bounds memory per request.
const MAX_REQUEST_BODY_BYTES: usize = 2 * 1024 * 1024;
/// Default per-request timeout; sheds requests that hang on a slow upstream or
/// DB so a stuck handler cannot pin a connection indefinitely. Overridable via
/// `CONSOLE_REQUEST_TIMEOUT_SECS` (see `AppConfig::request_timeout`).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const OPENAPI_YAML: &str = include_str!("../../openapi/openapi.yaml");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfiguredRouteSurface {
    pub name: &'static str,
    pub paths: &'static [&'static str],
}

pub const AUDIT_ROUTE_PATH: &str = "/api/audit";
pub const AUDIT_ATTESTATION_ROUTE_PATH: &str = "/api/v1/audit/attestation";
pub const AUDIT_ROUTE_PATHS: &[&str] = &[AUDIT_ROUTE_PATH, AUDIT_ATTESTATION_ROUTE_PATH];
pub const CONFIGURED_ROUTE_SURFACES: &[ConfiguredRouteSurface] = &[
    ConfiguredRouteSurface {
        name: "audit",
        paths: AUDIT_ROUTE_PATHS,
    },
    ConfiguredRouteSurface {
        name: "attendance",
        paths: console_attendance_rest::ATTENDANCE_ROUTE_PATHS,
    },
    ConfiguredRouteSurface {
        name: "inventory",
        paths: console_inventory_rest::INVENTORY_ROUTE_PATHS,
    },
    ConfiguredRouteSurface {
        name: "dispatch",
        paths: console_dispatch_rest::DISPATCH_ROUTE_PATHS,
    },
    ConfiguredRouteSurface {
        name: "benefit",
        paths: console_benefit_rest::BENEFIT_ROUTE_PATHS,
    },
    ConfiguredRouteSurface {
        name: "consulting",
        paths: console_consulting_rest::CONSULTING_ROUTE_PATHS,
    },
    ConfiguredRouteSurface {
        name: "financial",
        paths: console_financial_rest::FINANCIAL_ROUTE_PATHS,
    },
    ConfiguredRouteSurface {
        name: "evaluation",
        paths: console_evaluation_rest::EVALUATION_ROUTE_PATHS,
    },
    ConfiguredRouteSurface {
        name: "inspection",
        paths: console_inspection_rest::INSPECTION_ROUTE_PATHS,
    },
    ConfiguredRouteSurface {
        name: "support",
        paths: console_support_rest::SUPPORT_ROUTE_PATHS,
    },
    ConfiguredRouteSurface {
        name: "identity",
        paths: console_identity_rest::IDENTITY_ROUTE_PATHS,
    },
    ConfiguredRouteSurface {
        name: "compliance",
        paths: console_compliance_rest::COMPLIANCE_ROUTE_PATHS,
    },
    ConfiguredRouteSurface {
        name: "integrity",
        paths: console_integrity::INTEGRITY_ROUTE_PATHS,
    },
    ConfiguredRouteSurface {
        name: "registry",
        paths: console_registry_rest::REGISTRY_ROUTE_PATHS,
    },
    ConfiguredRouteSurface {
        name: "hr",
        paths: hr::HR_ROUTE_PATHS,
    },
    ConfiguredRouteSurface {
        name: "workflow-studio",
        paths: workflow_studio::WORKFLOW_STUDIO_ROUTE_PATHS,
    },
    ConfiguredRouteSurface {
        name: "collaboration",
        paths: collaboration::COLLABORATION_ROUTE_PATHS,
    },
    ConfiguredRouteSurface {
        name: "sales",
        paths: console_sales_rest::SALES_ROUTE_PATHS,
    },
    ConfiguredRouteSurface {
        name: "reporting",
        paths: console_reporting_rest::KPI_ROUTE_PATHS,
    },
    ConfiguredRouteSurface {
        name: "workorder",
        paths: console_workorder_rest::WORKORDER_ROUTE_PATHS,
    },
    ConfiguredRouteSurface {
        name: "workorder-mobile",
        paths: console_workorder_rest::MOBILE_ROUTE_PATHS,
    },
    ConfiguredRouteSurface {
        name: "facilities",
        paths: console_facilities_rest::FACILITIES_ROUTE_PATHS,
    },
    ConfiguredRouteSurface {
        name: "production",
        paths: console_production_rest::PRODUCTION_ROUTE_PATHS,
    },
    ConfiguredRouteSurface {
        name: "messenger",
        paths: console_messenger_rest::MESSENGER_ROUTE_PATHS,
    },
    ConfiguredRouteSurface {
        name: "comms",
        paths: console_comms_rest::COMMS_ROUTE_PATHS,
    },
    ConfiguredRouteSurface {
        name: "platform",
        paths: console_platform_rest::PLATFORM_ROUTE_PATHS,
    },
    ConfiguredRouteSurface {
        name: "auth",
        paths: console_platform_auth_rest::AUTH_ROUTE_PATHS,
    },
    ConfiguredRouteSurface {
        name: "realtime",
        paths: console_platform_realtime::WS_ROUTE_PATHS,
    },
    ConfiguredRouteSurface {
        name: "ontology",
        paths: console_ontology_rest::ONTOLOGY_ROUTE_PATHS,
    },
    ConfiguredRouteSurface {
        name: "governance",
        paths: console_governance_rest::GOVERNANCE_ROUTE_PATHS,
    },
    ConfiguredRouteSurface {
        name: "orgchange",
        paths: console_orgchange_rest::ORG_CHANGE_ROUTE_PATHS,
    },
    ConfiguredRouteSurface {
        name: "policy",
        paths: console_platform_authz_rest::CEDAR_POLICY_ROUTE_PATHS,
    },
    ConfiguredRouteSurface {
        name: "evidence",
        paths: console_docs_rest::EVIDENCE_ROUTE_PATHS,
    },
    ConfiguredRouteSurface {
        name: "notices",
        paths: console_notices_rest::NOTICES_ROUTE_PATHS,
    },
    ConfiguredRouteSurface {
        name: "finance-gl",
        paths: console_finance_gl_rest::FINANCE_GL_ROUTE_PATHS,
    },
    ConfiguredRouteSurface {
        name: "payroll",
        paths: console_payroll_rest::PAYROLL_ROUTE_PATHS,
    },
    ConfiguredRouteSurface {
        name: "analytics",
        paths: console_analytics_quant_rest::ANALYTICS_QUANT_ROUTE_PATHS,
    },
    ConfiguredRouteSurface {
        name: "equipment-3r",
        paths: console_equipment_rest::EQUIPMENT_3R_ROUTE_PATHS,
    },
    ConfiguredRouteSurface {
        name: "notifications",
        paths: console_notifications_rest::NOTIFICATIONS_ROUTE_PATHS,
    },
    ConfiguredRouteSurface {
        name: "recruiting",
        paths: console_recruiting_rest::RECRUITING_ROUTE_PATHS,
    },
    // The hire handshake is the one recruiting route that lives in the app
    // crate (acceptance → employee creation must go through the HR-owned core
    // in one transaction), so it is its own surface rather than an entry the
    // recruiting crate could export.
    ConfiguredRouteSurface {
        name: "recruiting-hire",
        paths: recruiting_hire::RECRUITING_HIRE_ROUTE_PATHS,
    },
];

#[cfg(test)]
mod production_route_surface_tests {
    use super::*;
    use console_production_rest::{
        PRODUCTION_CAPACITY_SLOTS_PATH, PRODUCTION_OPERATION_RECORDS_PATH, PRODUCTION_PLAN_PATH,
        PRODUCTION_PLAN_RELEASE_PATH, PRODUCTION_PLANS_PATH, PRODUCTION_SOURCE_INGRESS_PATH,
        PRODUCTION_SOURCE_SYSTEM_DISABLE_PATH, PRODUCTION_SOURCE_SYSTEM_ROTATE_PATH,
        PRODUCTION_SOURCE_SYSTEMS_PATH,
    };

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn production_execution_routes_are_assembled_and_contract_stable() {
        let production = CONFIGURED_ROUTE_SURFACES
            .iter()
            .find(|surface| surface.name == "production")
            .expect("production router must be mounted in the application");
        assert_eq!(
            production.paths,
            [
                PRODUCTION_PLANS_PATH,
                PRODUCTION_CAPACITY_SLOTS_PATH,
                PRODUCTION_PLAN_PATH,
                PRODUCTION_PLAN_RELEASE_PATH,
                PRODUCTION_OPERATION_RECORDS_PATH,
                PRODUCTION_SOURCE_INGRESS_PATH,
                PRODUCTION_SOURCE_SYSTEMS_PATH,
                PRODUCTION_SOURCE_SYSTEM_ROTATE_PATH,
                PRODUCTION_SOURCE_SYSTEM_DISABLE_PATH,
            ]
        );
    }
}

/// Embedded schema migrations, compiled into the binary at build time from the
/// canonical `console-platform-db` migration directory (the same `0001..NNNN_*.sql`
/// files applied to prod). `migrate` mode runs these in version order; sqlx
/// tracks applied versions + per-file checksums in `_sqlx_migrations`, so re-runs
/// are idempotent and a mutated already-applied file is rejected rather than
/// silently re-run. The path is relative to this crate's manifest (`backend/app`).
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../crates/platform/db/migrations");
mod schema_install;
const MIGRATION_LOCK_TIMEOUT: &str = "5s";
const MIGRATION_STATEMENT_TIMEOUT: &str = "60s";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AppRole {
    Api,
    Worker,
    /// One-shot schema-migration mode. Connects to `DATABASE_URL` as the table
    /// OWNER, runs the embedded migrations, then exits â it never serves HTTP.
    /// Invoked out of band (an Argo CD PreSync Job) before the api/worker
    /// Deployments roll, so the runtime `console_rt` role never needs DDL.
    Migrate,
}

impl Display for AppRole {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Api => f.write_str("api"),
            Self::Worker => f.write_str("worker"),
            Self::Migrate => f.write_str("migrate"),
        }
    }
}

impl std::str::FromStr for AppRole {
    type Err = AppError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "api" => Ok(Self::Api),
            "worker" => Ok(Self::Worker),
            "migrate" => Ok(Self::Migrate),
            other => Err(AppError::Config(format!(
                "CONSOLE_APP_ROLE must be api, worker, or migrate, got {other:?}"
            ))),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub role: AppRole,
    pub service_name: String,
    pub http_addr: SocketAddr,
    pub database_url: Option<String>,
    /// Dedicated least-privilege connection used only for leave commands
    /// (`LEAVE_COMMAND_DATABASE_URL`). The API requires this whenever its
    /// general runtime `DATABASE_URL` is configured so command execution can
    /// never silently fall back to the broader `console_rt` credential. Worker and
    /// migrate roles do not open or require this pool.
    pub leave_command_database_url: Option<String>,
    /// Dedicated least-privilege connection for ontology schema commands
    /// (`ONTOLOGY_COMMAND_DATABASE_URL`). Like the leave command credential,
    /// this is API-only and never falls back to `console_rt` in a configured
    /// deployment.
    pub ontology_command_database_url: Option<String>,
    /// Dedicated least-privilege connection for destructive platform tenant
    /// removal (`PLATFORM_FORCE_COMMAND_DATABASE_URL`). This API-only pool is
    /// required whenever the runtime database is configured; it never falls
    /// back to `console_rt`.
    pub platform_force_command_database_url: Option<String>,
    pub otlp_endpoint: Option<String>,
    pub jwt: Option<JwtVerifierConfig>,
    pub auth_rest: Option<AuthRestConfig>,
    pub storage: Option<S3StorageConfig>,
    pub dispatch_timers: DispatchTimerConfig,
    pub dispatch_jobs_enabled: bool,
    /// Max concurrent evidence transcodes the worker runs at once
    /// (`CONSOLE_EVIDENCE_TRANSCODE_CONCURRENCY`, default 2). Backpressure cap so a
    /// burst of mechanic uploads can't exhaust the worker's CPU/disk.
    pub evidence_transcode_concurrency: usize,
    pub fcm: Option<FcmConfig>,
    pub solapi: Option<SolapiConfig>,
    pub solapi_disabled_reason: Option<String>,
    /// Outbound SMTP relay for transactional email (open-signup OTP). `Some`
    /// only when the live `CONSOLE_EMAIL_*` SMTP group is complete and valid.
    pub email: Option<SmtpEmailConfig>,
    /// Explicit non-production mode that allows `StubEmailSender` to log OTPs
    /// instead of sending mail (`CONSOLE_EMAIL_STUB_MODE=local|dev|development|test|e2e`).
    /// `None` means missing SMTP fails closed at send time and partial SMTP
    /// configuration fails during config parsing.
    pub email_stub_mode: Option<StubEmailMode>,
    pub shutdown_timeout: Duration,
    /// Per-request timeout applied to every non-streaming route
    /// (`CONSOLE_REQUEST_TIMEOUT_SECS`, default 30s). Deliberately NOT applied to
    /// the long-lived realtime WS route, which is merged outside this layer.
    /// Configurable primarily so tests can prove the realtime route escapes the
    /// timeout without waiting the full production budget.
    pub request_timeout: Duration,
    /// Deploy-time cold-start OTP for the cold-start SUPER_ADMIN, supplied
    /// out-of-band via `CONSOLE_COLDSTART_OTP`. `None` (or empty) means no
    /// cold-start OTP is seeded at boot â the normal state once an admin exists.
    pub coldstart_otp: Option<String>,
    /// Lifetime of a boot-seeded cold-start OTP (`CONSOLE_COLDSTART_OTP_TTL_SECS`,
    /// default 3600s).
    pub coldstart_otp_ttl: time::Duration,
    /// Number of configured reverse-proxy hops in front of the service,
    /// including the direct transport peer (`CONSOLE_TRUSTED_PROXY_COUNT`, default
    /// 0). Forwarding headers are ignored unless every configured hop is in
    /// [`Self::trusted_proxy_cidrs`].
    pub trusted_proxy_count: usize,
    /// CIDRs allowed to present forwarding headers at process ingress
    /// (`CONSOLE_TRUSTED_PROXY_CIDRS`, comma-separated). Required when the hop
    /// count is nonzero.
    pub trusted_proxy_cidrs: Vec<IpNet>,
    /// Native app-link association metadata served at `/.well-known/*`. Drives
    /// the public, unauthenticated Apple App Site Association + Android asset
    /// links documents that authorize the native apps' passkeys for the RP
    /// domain. Sourced from `CONSOLE_IOS_APP_IDS`, `CONSOLE_ANDROID_PACKAGE`, and
    /// `CONSOLE_ANDROID_CERT_SHA256` (see [`app_links_config_from_vars`]).
    pub app_links: AppLinksConfig,
    /// Whether the inbound webmail IMAP sync worker runs (`CONSOLE_MAIL_ENABLED`,
    /// default false). Even when true the worker only starts if the master KEK
    /// (`CONSOLE_MAIL_MASTER_KEY`) and object storage are both configured — it is a
    /// no-op otherwise, so a misconfiguration never crashes the app.
    pub mail_enabled: bool,
    /// Mox webapi base URL for outbound webmail transport
    /// (`CONSOLE_MAIL_MOX_BASE_URL`). `None` keeps the default SMTP/lettre path.
    pub mail_mox_base_url: Option<String>,
    /// Shared secret for the inbound Mox delivery webhook
    /// (`CONSOLE_MAIL_MOX_WEBHOOK_SECRET`). `None` leaves the webhook mounted but
    /// unavailable with 503 rather than accepting unauthenticated deliveries.
    pub mail_mox_webhook_secret: Option<String>,
    /// Whether the L20 tamper-evident audit-chain seal worker runs
    /// (`CONSOLE_AUDIT_CHAIN_SEAL_ENABLED`, default false). When true, the
    /// worker requires [`Self::audit_chain_external`] (pinned anchors + custody
    /// URL) unless [`Self::audit_chain_allow_dev_signer`] is explicitly set —
    /// enabling sealing without custody fails closed (worker does not start).
    /// Default OFF so a misconfigured deploy never writes throwaway-keyed seals.
    /// The attestation REST endpoint reads whatever is sealed regardless of
    /// this flag.
    pub audit_chain_seal_enabled: bool,
    /// Production external seal config: active `key_ref`, locally pinned trust
    /// anchors, and HTTP custody URL (`CONSOLE_AUDIT_CHAIN_SEAL_KEY_REF` +
    /// `CONSOLE_AUDIT_CHAIN_SEAL_ANCHORS` + `CONSOLE_AUDIT_CHAIN_CUSTODY_URL`).
    /// `None` keeps attestation on the in-memory dev verifier (non-evidentiary).
    pub audit_chain_external: Option<audit_chain_signer::AuditChainExternalConfig>,
    /// Explicit escape hatch (`CONSOLE_AUDIT_CHAIN_ALLOW_DEV_SIGNER`, default
    /// false) that lets the seal worker use `InMemoryEd25519Signer` when
    /// external custody is unset. Never set in production — seals under
    /// `test:ed25519:<hex>` are not tamper evidence.
    pub audit_chain_allow_dev_signer: bool,
    /// The tenant that owns the PUBLIC storefront/CX channel
    /// (`STOREFRONT_ORG_ID`). `None` keeps the legacy KNL default for public
    /// sales/support intake routes. Set it to the storefront tenant's real
    /// `organizations.id` when that tenant was re-minted via the console with a
    /// random uuid, so public inquiry/intake writes land in the SAME org the
    /// staff inbox reads under (#19.21/#398) instead of the `0x…a1` sentinel.
    pub storefront_org: Option<OrgId>,
    /// In-console office editor (ONLYOFFICE DocumentServer) integration. `None`
    /// unless all of `CONSOLE_OFFICE_JWT_SECRET` / `CONSOLE_OFFICE_CALLBACK_BASE_URL` /
    /// `CONSOLE_OFFICE_DOCSERVER_URL` are set; the office routes still mount but
    /// return `503 office_not_configured`.
    pub office: Option<office::OfficeConfig>,
    /// Base64-encoded, exactly 32-byte HMAC key for machine-only production
    /// ingress. Its absence leaves only that ingress route unavailable (503).
    pub production_service_principal_hmac_key: Option<[u8; 32]>,
}

/// Native app-link association config for the `/.well-known/*` endpoints.
///
/// All fields are optional and default to empty: a deployment that has not yet
/// provisioned its native apps serves an empty (but well-formed) association
/// document rather than failing to boot. The empty state is logged at startup so
/// the gap is visible. Once set, the documents authorize the native apps to use
/// passkeys scoped to the WebAuthn RP domain.
#[derive(Debug, Clone, Default)]
pub struct AppLinksConfig {
    /// iOS app identifiers (`<TeamID>.<bundle-id>`), e.g. `ABCDE12345.com.knl.fsm`.
    /// From `CONSOLE_IOS_APP_IDS` (comma-separated).
    pub ios_app_ids: Vec<String>,
    /// Android application id, e.g. `com.knl.fsm`. From `CONSOLE_ANDROID_PACKAGE`.
    pub android_package: Option<String>,
    /// Android signing-cert SHA-256 fingerprints (colon-separated hex). From
    /// `CONSOLE_ANDROID_CERT_SHA256` (comma-separated for multiple signing keys).
    pub android_cert_sha256: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct JwtVerifierConfig {
    pub issuer: String,
    pub audience: String,
    pub public_key_pem: String,
}

impl JwtVerifierConfig {
    fn build(&self) -> Result<JwtVerifier, AppError> {
        JwtVerifier::from_es256_public_pem(
            JwtSettings {
                issuer: self.issuer.clone(),
                audience: self.audience.clone(),
                access_token_ttl: time::Duration::minutes(15),
            },
            self.public_key_pem.as_bytes(),
        )
        .map_err(|err| AppError::Config(format!("invalid CONSOLE_JWT_PUBLIC_KEY_PEM: {err}")))
    }
}

/// Build the JWT issuer the PLATFORM view-as START path uses to mint short-lived
/// read-only impersonation tokens. It signs with the SAME ES256 keypair as the
/// auth-rest issuer (sourced from [`AuthRestConfig`]), so the tokens it mints
/// verify against the live `jwt_verifier`. The settings' `access_token_ttl` is
/// set to the view-as ceiling as a backstop; the START handler additionally
/// clamps every minted token to that ceiling explicitly.
fn build_view_as_issuer(config: &AuthRestConfig) -> Result<JwtIssuer, AppError> {
    JwtIssuer::from_es256_pem(
        JwtSettings {
            issuer: config.jwt_issuer.clone(),
            audience: config.jwt_audience.clone(),
            access_token_ttl: console_platform_rest::VIEW_AS_TOKEN_TTL,
        },
        config.jwt_private_key_pem.as_bytes(),
        config.jwt_public_key_pem.as_bytes(),
    )
    .map_err(|err| AppError::Config(format!("invalid view-as issuer key material: {err}")))
}

fn policy_step_up_from_auth_config(config: &AuthRestConfig) -> Result<PasskeyService, AppError> {
    let rp_origin = Url::parse(&config.rp_origin)
        .map_err(|err| AppError::Config(format!("invalid WebAuthn RP origin: {err}")))?;
    PasskeyService::new(WebauthnSettings {
        rp_id: config.rp_id.clone(),
        rp_origin,
        rp_name: config.rp_name.clone(),
        extra_allowed_origins: Vec::new(),
        ceremony_ttl: config.ceremony_ttl,
    })
    .map_err(|err| AppError::Config(format!("invalid policy step-up WebAuthn config: {err}")))
}

impl AppConfig {
    pub fn from_env() -> Result<Self, AppError> {
        Self::from_pairs(env::vars())
    }

    pub fn from_pairs<I, K, V>(pairs: I) -> Result<Self, AppError>
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let vars: HashMap<String, String> = pairs
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect();

        let role = vars
            .get("CONSOLE_APP_ROLE")
            .map(String::as_str)
            .unwrap_or("api")
            .parse()?;
        let service_name = vars
            .get("OTEL_SERVICE_NAME")
            .or_else(|| vars.get("CONSOLE_SERVICE_NAME"))
            .cloned()
            .unwrap_or_else(|| DEFAULT_SERVICE_NAME.to_owned());
        let http_addr = vars
            .get("CONSOLE_HTTP_ADDR")
            .map(String::as_str)
            .unwrap_or(DEFAULT_HTTP_ADDR)
            .parse::<SocketAddr>()
            .map_err(|err| AppError::Config(format!("invalid CONSOLE_HTTP_ADDR: {err}")))?;
        let database_url = non_empty(vars.get("DATABASE_URL"));
        let leave_command_database_url = non_empty(vars.get("LEAVE_COMMAND_DATABASE_URL"));
        if role == AppRole::Api && database_url.is_some() && leave_command_database_url.is_none() {
            return Err(AppError::Config(
                "LEAVE_COMMAND_DATABASE_URL is required for api role when DATABASE_URL is configured"
                    .to_owned(),
            ));
        }
        let ontology_command_database_url = non_empty(vars.get("ONTOLOGY_COMMAND_DATABASE_URL"));
        let platform_force_command_database_url =
            non_empty(vars.get("PLATFORM_FORCE_COMMAND_DATABASE_URL"));
        if role == AppRole::Api && database_url.is_some() && ontology_command_database_url.is_none()
        {
            return Err(AppError::Config(
                "ONTOLOGY_COMMAND_DATABASE_URL is required for api role when DATABASE_URL is configured"
                    .to_owned(),
            ));
        }
        if role == AppRole::Api {
            if database_url.is_some() && platform_force_command_database_url.is_none() {
                return Err(AppError::Config(
                    "PLATFORM_FORCE_COMMAND_DATABASE_URL is required for api role when DATABASE_URL is configured".to_owned(),
                ));
            }
            ensure_distinct_database_urls([
                ("DATABASE_URL", database_url.as_deref()),
                (
                    "LEAVE_COMMAND_DATABASE_URL",
                    leave_command_database_url.as_deref(),
                ),
                (
                    "ONTOLOGY_COMMAND_DATABASE_URL",
                    ontology_command_database_url.as_deref(),
                ),
                (
                    "PLATFORM_FORCE_COMMAND_DATABASE_URL",
                    platform_force_command_database_url.as_deref(),
                ),
            ])?;

            if let (Some(database_url), Some(leave_url), Some(ontology_url), Some(force_url)) = (
                database_url.as_deref(),
                leave_command_database_url.as_deref(),
                ontology_command_database_url.as_deref(),
                platform_force_command_database_url.as_deref(),
            ) {
                let runtime_password =
                    validate_database_url_identity("DATABASE_URL", database_url, "console_rt")?;
                let leave_password = validate_database_url_identity(
                    "LEAVE_COMMAND_DATABASE_URL",
                    leave_url,
                    "console_leave_cmd",
                )?;
                let ontology_password = validate_database_url_identity(
                    "ONTOLOGY_COMMAND_DATABASE_URL",
                    ontology_url,
                    "console_ontology_cmd",
                )?;
                let platform_force_password = validate_database_url_identity(
                    "PLATFORM_FORCE_COMMAND_DATABASE_URL",
                    force_url,
                    "console_platform_force_cmd",
                )?;
                ensure_distinct_database_credentials([
                    ("DATABASE_URL", Some(runtime_password.as_str())),
                    ("LEAVE_COMMAND_DATABASE_URL", Some(leave_password.as_str())),
                    (
                        "ONTOLOGY_COMMAND_DATABASE_URL",
                        Some(ontology_password.as_str()),
                    ),
                    (
                        "PLATFORM_FORCE_COMMAND_DATABASE_URL",
                        Some(platform_force_password.as_str()),
                    ),
                ])?;
            }
        } else {
            let database_password = database_url
                .as_deref()
                .map(|database_url| {
                    let expected_role = match role {
                        AppRole::Worker => "console_rt",
                        AppRole::Migrate => "console_app",
                        AppRole::Api => unreachable!("api database URLs are validated above"),
                    };
                    validate_database_url_identity("DATABASE_URL", database_url, expected_role)
                })
                .transpose()?;
            let leave_password = leave_command_database_url
                .as_deref()
                .map(|url| {
                    validate_database_url_identity(
                        "LEAVE_COMMAND_DATABASE_URL",
                        url,
                        "console_leave_cmd",
                    )
                })
                .transpose()?;
            let ontology_password = ontology_command_database_url
                .as_deref()
                .map(|url| {
                    validate_database_url_identity(
                        "ONTOLOGY_COMMAND_DATABASE_URL",
                        url,
                        "console_ontology_cmd",
                    )
                })
                .transpose()?;
            let platform_force_password = platform_force_command_database_url
                .as_deref()
                .map(|url| {
                    validate_database_url_identity(
                        "PLATFORM_FORCE_COMMAND_DATABASE_URL",
                        url,
                        "console_platform_force_cmd",
                    )
                })
                .transpose()?;
            ensure_distinct_database_credentials([
                ("DATABASE_URL", database_password.as_deref()),
                ("LEAVE_COMMAND_DATABASE_URL", leave_password.as_deref()),
                (
                    "ONTOLOGY_COMMAND_DATABASE_URL",
                    ontology_password.as_deref(),
                ),
                (
                    "PLATFORM_FORCE_COMMAND_DATABASE_URL",
                    platform_force_password.as_deref(),
                ),
            ])?;
        }
        let otlp_endpoint = non_empty(vars.get("OTEL_EXPORTER_OTLP_ENDPOINT"));
        let jwt_public_key_pem = non_empty(vars.get("CONSOLE_JWT_PUBLIC_KEY_PEM"));
        let jwt_has_partial_config = jwt_public_key_pem.is_none()
            && (non_empty(vars.get("CONSOLE_JWT_ISSUER")).is_some()
                || non_empty(vars.get("CONSOLE_JWT_AUDIENCE")).is_some());
        if jwt_has_partial_config {
            return Err(AppError::Config(
                "CONSOLE_JWT_PUBLIC_KEY_PEM is required when JWT issuer/audience is configured"
                    .to_owned(),
            ));
        }
        let jwt = jwt_public_key_pem.map(|public_key_pem| JwtVerifierConfig {
            issuer: non_empty(vars.get("CONSOLE_JWT_ISSUER"))
                .unwrap_or_else(|| DEFAULT_JWT_ISSUER.to_owned()),
            audience: non_empty(vars.get("CONSOLE_JWT_AUDIENCE"))
                .unwrap_or_else(|| DEFAULT_JWT_AUDIENCE.to_owned()),
            public_key_pem,
        });
        let auth_rest = auth_rest_config_from_vars(&vars, jwt.as_ref())?;
        let storage = storage_config_from_vars(&vars)?;
        let dispatch_timers = dispatch_timer_config_from_vars(&vars)?;
        let dispatch_jobs_enabled = match vars.get("CONSOLE_DISPATCH_JOBS_ENABLED") {
            Some(raw) => raw.parse::<bool>().map_err(|err| {
                AppError::Config(format!("invalid CONSOLE_DISPATCH_JOBS_ENABLED: {err}"))
            })?,
            None => true,
        };
        let evidence_transcode_concurrency =
            match vars.get("CONSOLE_EVIDENCE_TRANSCODE_CONCURRENCY") {
                Some(raw) => raw
                    .parse::<usize>()
                    .map_err(|err| {
                        AppError::Config(format!(
                            "invalid CONSOLE_EVIDENCE_TRANSCODE_CONCURRENCY: {err}"
                        ))
                    })
                    .map(|value| value.max(1))?,
                None => DEFAULT_EVIDENCE_TRANSCODE_CONCURRENCY,
            };
        let fcm = fcm_config_from_vars(&vars)?;
        let (solapi, solapi_disabled_reason) = solapi_config_from_vars(&vars)?;
        let email_stub_mode = email_stub_mode_from_vars(&vars)?;
        let email = email_config_from_vars(&vars, email_stub_mode)?;
        let shutdown_timeout = match vars.get("CONSOLE_SHUTDOWN_TIMEOUT_SECS") {
            Some(raw) => raw.parse::<u64>().map(Duration::from_secs).map_err(|err| {
                AppError::Config(format!("invalid CONSOLE_SHUTDOWN_TIMEOUT_SECS: {err}"))
            })?,
            None => Duration::from_secs(DEFAULT_SHUTDOWN_TIMEOUT_SECS),
        };
        let request_timeout = match vars.get("CONSOLE_REQUEST_TIMEOUT_SECS") {
            Some(raw) => raw.parse::<u64>().map(Duration::from_secs).map_err(|err| {
                AppError::Config(format!("invalid CONSOLE_REQUEST_TIMEOUT_SECS: {err}"))
            })?,
            None => REQUEST_TIMEOUT,
        };
        let coldstart_otp = non_empty(vars.get("CONSOLE_COLDSTART_OTP"));
        let coldstart_otp_ttl = parse_time_duration_secs(
            vars.get("CONSOLE_COLDSTART_OTP_TTL_SECS"),
            DEFAULT_COLDSTART_OTP_TTL_SECS,
            "CONSOLE_COLDSTART_OTP_TTL_SECS",
        )?;
        let trusted_proxy_count =
            parse_trusted_proxy_count(vars.get("CONSOLE_TRUSTED_PROXY_COUNT"))?;
        let trusted_proxy_cidrs = parse_trusted_proxy_cidrs(
            vars.get("CONSOLE_TRUSTED_PROXY_CIDRS"),
            trusted_proxy_count,
        )?;
        let app_links = app_links_config_from_vars(&vars);
        let mail_enabled = match vars.get("CONSOLE_MAIL_ENABLED") {
            Some(raw) => raw
                .parse::<bool>()
                .map_err(|err| AppError::Config(format!("invalid CONSOLE_MAIL_ENABLED: {err}")))?,
            None => false,
        };
        let mail_mox_base_url = non_empty(vars.get("CONSOLE_MAIL_MOX_BASE_URL"));
        let mail_mox_webhook_secret = non_empty(vars.get("CONSOLE_MAIL_MOX_WEBHOOK_SECRET"));
        let audit_chain_seal_enabled = match vars.get("CONSOLE_AUDIT_CHAIN_SEAL_ENABLED") {
            Some(raw) => raw.parse::<bool>().map_err(|err| {
                AppError::Config(format!("invalid CONSOLE_AUDIT_CHAIN_SEAL_ENABLED: {err}"))
            })?,
            None => false,
        };
        let audit_chain_external =
            audit_chain_signer::external_config_from_vars(|key| non_empty(vars.get(key)))?;
        let audit_chain_allow_dev_signer = match vars.get("CONSOLE_AUDIT_CHAIN_ALLOW_DEV_SIGNER") {
            Some(raw) => raw.parse::<bool>().map_err(|err| {
                AppError::Config(format!(
                    "invalid CONSOLE_AUDIT_CHAIN_ALLOW_DEV_SIGNER: {err}"
                ))
            })?,
            None => false,
        };
        let storefront_org = match non_empty(vars.get("STOREFRONT_ORG_ID")) {
            Some(raw) => Some(
                OrgId::from_str(&raw)
                    .map_err(|err| AppError::Config(format!("invalid STOREFRONT_ORG_ID: {err}")))?,
            ),
            None => None,
        };
        let office = office::office_config_from_vars(|key| non_empty(vars.get(key)))
            .map_err(AppError::Config)?;
        let production_service_principal_hmac_key =
            non_empty(vars.get("CONSOLE_PRODUCTION_SERVICE_PRINCIPAL_HMAC_KEY"))
                .map(|encoded| {
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(encoded)
                        .map_err(|_| {
                            AppError::Config(
                                "CONSOLE_PRODUCTION_SERVICE_PRINCIPAL_HMAC_KEY must be base64"
                                    .to_owned(),
                            )
                        })?;
                    bytes.try_into().map_err(|_: Vec<u8>| {
                        AppError::Config(
                    "CONSOLE_PRODUCTION_SERVICE_PRINCIPAL_HMAC_KEY must decode to exactly 32 bytes"
                        .to_owned(),
                )
                    })
                })
                .transpose()?;

        Ok(Self {
            role,
            service_name,
            http_addr,
            database_url,
            leave_command_database_url,
            ontology_command_database_url,
            platform_force_command_database_url,
            otlp_endpoint,
            jwt,
            auth_rest,
            storage,
            dispatch_timers,
            dispatch_jobs_enabled,
            evidence_transcode_concurrency,
            fcm,
            solapi,
            solapi_disabled_reason,
            email,
            email_stub_mode,
            shutdown_timeout,
            request_timeout,
            coldstart_otp,
            coldstart_otp_ttl,
            trusted_proxy_count,
            trusted_proxy_cidrs,
            app_links,
            mail_enabled,
            mail_mox_base_url,
            mail_mox_webhook_secret,
            audit_chain_seal_enabled,
            audit_chain_external,
            audit_chain_allow_dev_signer,
            storefront_org,
            office,
            production_service_principal_hmac_key,
        })
    }
}

/// Parse the native app-link association config from the environment.
///
/// Every field is optional: an unset/empty value yields an empty list (or
/// `None`), so the `/.well-known/*` endpoints serve a well-formed-but-empty
/// document instead of the process refusing to boot. The comma-separated lists
/// trim and drop blank entries so a trailing comma or stray whitespace is
/// harmless.
fn app_links_config_from_vars(vars: &HashMap<String, String>) -> AppLinksConfig {
    AppLinksConfig {
        ios_app_ids: parse_csv_list(vars.get("CONSOLE_IOS_APP_IDS")),
        android_package: non_empty(vars.get("CONSOLE_ANDROID_PACKAGE")),
        android_cert_sha256: parse_csv_list(vars.get("CONSOLE_ANDROID_CERT_SHA256")),
    }
}

/// Split a comma-separated env value into trimmed, non-empty entries. Returns an
/// empty `Vec` when unset or all-blank.
fn parse_csv_list(value: Option<&String>) -> Vec<String> {
    value
        .map(|raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn auth_rest_config_from_vars(
    vars: &HashMap<String, String>,
    jwt: Option<&JwtVerifierConfig>,
) -> Result<Option<AuthRestConfig>, AppError> {
    let Some(jwt_private_key_pem) = non_empty(vars.get("CONSOLE_JWT_PRIVATE_KEY_PEM")) else {
        return Ok(None);
    };
    let jwt = jwt.ok_or_else(|| {
        AppError::Config(
            "CONSOLE_JWT_PUBLIC_KEY_PEM is required when CONSOLE_JWT_PRIVATE_KEY_PEM is configured"
                .to_owned(),
        )
    })?;
    let rp_id = non_empty(vars.get("CONSOLE_WEBAUTHN_RP_ID")).ok_or_else(|| {
        AppError::Config(
            "CONSOLE_WEBAUTHN_RP_ID is required when auth REST is configured".to_owned(),
        )
    })?;
    let rp_origin = non_empty(vars.get("CONSOLE_WEBAUTHN_RP_ORIGIN")).ok_or_else(|| {
        AppError::Config(
            "CONSOLE_WEBAUTHN_RP_ORIGIN is required when auth REST is configured".to_owned(),
        )
    })?;

    Ok(Some(AuthRestConfig {
        rp_id,
        rp_origin,
        rp_name: non_empty(vars.get("CONSOLE_WEBAUTHN_RP_NAME"))
            .unwrap_or_else(|| DEFAULT_WEBAUTHN_RP_NAME.to_owned()),
        ceremony_ttl: parse_time_duration_secs(
            vars.get("CONSOLE_AUTH_CEREMONY_TTL_SECS"),
            DEFAULT_AUTH_CEREMONY_TTL_SECS,
            "CONSOLE_AUTH_CEREMONY_TTL_SECS",
        )?,
        jwt_issuer: jwt.issuer.clone(),
        jwt_audience: jwt.audience.clone(),
        jwt_private_key_pem,
        jwt_public_key_pem: jwt.public_key_pem.clone(),
        refresh_token_ttl: parse_time_duration_secs(
            vars.get("CONSOLE_REFRESH_TOKEN_TTL_SECS"),
            DEFAULT_REFRESH_TOKEN_TTL_SECS,
            "CONSOLE_REFRESH_TOKEN_TTL_SECS",
        )?,
        refresh_family_absolute_ttl: parse_time_duration_secs(
            vars.get("CONSOLE_REFRESH_FAMILY_ABSOLUTE_TTL_SECS"),
            DEFAULT_REFRESH_FAMILY_ABSOLUTE_TTL_SECS,
            "CONSOLE_REFRESH_FAMILY_ABSOLUTE_TTL_SECS",
        )?,
        cookie_secure: parse_cookie_secure(vars.get("CONSOLE_COOKIE_SECURE"))?,
    }))
}

/// Whether the web refresh cookie carries the `Secure` attribute. Defaults to
/// `true` (production over HTTPS); set `CONSOLE_COOKIE_SECURE=false` only for local
/// http dev, where a `Secure` cookie would be dropped on `http://localhost`.
fn parse_cookie_secure(raw: Option<&String>) -> Result<bool, AppError> {
    match non_empty(raw) {
        Some(value) => value
            .parse::<bool>()
            .map_err(|err| AppError::Config(format!("invalid CONSOLE_COOKIE_SECURE: {err}"))),
        None => Ok(true),
    }
}

/// Number of trusted reverse proxies in front of the process, including the
/// direct transport peer. Zero is the fail-closed default: raw forwarding
/// headers are ignored.
fn parse_trusted_proxy_count(raw: Option<&String>) -> Result<usize, AppError> {
    match raw {
        Some(raw) => raw
            .parse::<usize>()
            .map_err(|err| AppError::Config(format!("invalid CONSOLE_TRUSTED_PROXY_COUNT: {err}"))),
        None => Ok(0),
    }
}

fn parse_trusted_proxy_cidrs(
    raw: Option<&String>,
    trusted_proxy_count: usize,
) -> Result<Vec<IpNet>, AppError> {
    let cidrs = raw
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| {
                    value.parse::<IpNet>().map_err(|err| {
                        AppError::Config(format!(
                            "invalid CONSOLE_TRUSTED_PROXY_CIDRS entry {value:?}: {err}"
                        ))
                    })
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();
    if trusted_proxy_count > 0 && cidrs.is_empty() {
        return Err(AppError::Config(
            "CONSOLE_TRUSTED_PROXY_CIDRS is required when CONSOLE_TRUSTED_PROXY_COUNT is nonzero"
                .to_owned(),
        ));
    }
    Ok(cidrs)
}

fn storage_config_from_vars(
    vars: &HashMap<String, String>,
) -> Result<Option<S3StorageConfig>, AppError> {
    let Some(endpoint_url) = non_empty(vars.get("CONSOLE_S3_ENDPOINT_URL")) else {
        return Ok(None);
    };
    let required = |name: &'static str| {
        non_empty(vars.get(name)).ok_or_else(|| {
            AppError::Config(format!("{name} is required when S3 storage is configured"))
        })
    };
    let force_path_style = match non_empty(vars.get("CONSOLE_S3_FORCE_PATH_STYLE")) {
        Some(raw) => raw.parse::<bool>().map_err(|err| {
            AppError::Config(format!("invalid CONSOLE_S3_FORCE_PATH_STYLE: {err}"))
        })?,
        None => true,
    };

    Ok(Some(S3StorageConfig {
        endpoint_url,
        region: non_empty(vars.get("CONSOLE_S3_REGION")).unwrap_or_else(|| "us-east-1".to_owned()),
        access_key_id: required("CONSOLE_S3_ACCESS_KEY_ID")?,
        secret_access_key: required("CONSOLE_S3_SECRET_ACCESS_KEY")?,
        primary_bucket: required("CONSOLE_S3_PRIMARY_BUCKET")?,
        replica_bucket: required("CONSOLE_S3_REPLICA_BUCKET")?,
        force_path_style,
    }))
}

fn dispatch_timer_config_from_vars(
    vars: &HashMap<String, String>,
) -> Result<DispatchTimerConfig, AppError> {
    Ok(DispatchTimerConfig {
        accept_window: parse_time_duration_secs(
            vars.get("CONSOLE_DISPATCH_ACCEPT_WINDOW_SECS"),
            DEFAULT_DISPATCH_ACCEPT_WINDOW_SECS,
            "CONSOLE_DISPATCH_ACCEPT_WINDOW_SECS",
        )?,
        force_assign_alert_after: parse_time_duration_secs(
            vars.get("CONSOLE_DISPATCH_FORCE_ASSIGN_ALERT_SECS"),
            DEFAULT_DISPATCH_FORCE_ASSIGN_ALERT_SECS,
            "CONSOLE_DISPATCH_FORCE_ASSIGN_ALERT_SECS",
        )?,
        alimtalk_no_ack_after: parse_time_duration_secs(
            vars.get("CONSOLE_DISPATCH_ALIMTALK_NO_ACK_SECS"),
            DEFAULT_DISPATCH_ALIMTALK_NO_ACK_SECS,
            "CONSOLE_DISPATCH_ALIMTALK_NO_ACK_SECS",
        )?,
        gps_ping_freshness: parse_time_duration_secs(
            vars.get("CONSOLE_DISPATCH_GPS_FRESHNESS_SECS"),
            DEFAULT_DISPATCH_GPS_FRESHNESS_SECS,
            "CONSOLE_DISPATCH_GPS_FRESHNESS_SECS",
        )?,
    })
}

fn fcm_config_from_vars(vars: &HashMap<String, String>) -> Result<Option<FcmConfig>, AppError> {
    let project_id = non_empty(vars.get("CONSOLE_FCM_PROJECT_ID"));
    let client_email = non_empty(vars.get("CONSOLE_FCM_CLIENT_EMAIL"));
    let private_key_pem = non_empty(vars.get("CONSOLE_FCM_PRIVATE_KEY_PEM"));
    let configured = project_id.is_some() || client_email.is_some() || private_key_pem.is_some();
    if !configured {
        return Ok(None);
    }
    let config = FcmConfig {
        project_id: project_id.ok_or_else(|| {
            AppError::Config("CONSOLE_FCM_PROJECT_ID is required when FCM is configured".to_owned())
        })?,
        client_email: client_email.ok_or_else(|| {
            AppError::Config(
                "CONSOLE_FCM_CLIENT_EMAIL is required when FCM is configured".to_owned(),
            )
        })?,
        private_key_pem: private_key_pem.ok_or_else(|| {
            AppError::Config(
                "CONSOLE_FCM_PRIVATE_KEY_PEM is required when FCM is configured".to_owned(),
            )
        })?,
        token_uri: non_empty(vars.get("CONSOLE_FCM_TOKEN_URI"))
            .unwrap_or_else(|| DEFAULT_FCM_TOKEN_URI.to_owned()),
        scope: non_empty(vars.get("CONSOLE_FCM_SCOPE"))
            .unwrap_or_else(|| DEFAULT_FCM_SCOPE.to_owned()),
    };
    config
        .validate()
        .map_err(|err| AppError::Config(err.to_string()))?;
    Ok(Some(config))
}

fn solapi_config_from_vars(
    vars: &HashMap<String, String>,
) -> Result<(Option<SolapiConfig>, Option<String>), AppError> {
    let api_key = non_empty(vars.get("CONSOLE_SOLAPI_API_KEY"));
    let api_secret = non_empty(vars.get("CONSOLE_SOLAPI_API_SECRET"));
    let from = non_empty(vars.get("CONSOLE_SOLAPI_FROM"));
    let pf_id = non_empty(vars.get("CONSOLE_SOLAPI_PF_ID"));
    let template_id = non_empty(vars.get("CONSOLE_SOLAPI_TEMPLATE_ID"));
    let credentials_configured =
        api_key.is_some() || api_secret.is_some() || from.is_some() || pf_id.is_some();
    if !credentials_configured && template_id.is_none() {
        return Ok((None, None));
    }
    let Some(template_id) = template_id else {
        return Ok((
            None,
            Some(
                "Solapi Alimtalk disabled: CONSOLE_SOLAPI_TEMPLATE_ID is required after Kakao template approval"
                    .to_owned(),
            ),
        ));
    };
    let required = |value: Option<String>, name: &'static str| {
        value.ok_or_else(|| {
            AppError::Config(format!("{name} is required when Solapi is configured"))
        })
    };
    let config = SolapiConfig {
        base_url: non_empty(vars.get("CONSOLE_SOLAPI_BASE_URL"))
            .unwrap_or_else(|| DEFAULT_SOLAPI_BASE_URL.to_owned()),
        api_key: required(api_key, "CONSOLE_SOLAPI_API_KEY")?,
        api_secret: required(api_secret, "CONSOLE_SOLAPI_API_SECRET")?,
        from: required(from, "CONSOLE_SOLAPI_FROM")?,
        pf_id: required(pf_id, "CONSOLE_SOLAPI_PF_ID")?,
        template_id,
    };
    config
        .validate()
        .map_err(|err| AppError::Config(err.to_string()))?;
    Ok((Some(config), None))
}

/// Parse the single explicit policy that allows OTP-logging stub email.
///
/// The var is deliberately not boolean: operators must spell out a non-production
/// intent (`local`, `dev`, `development`, `test`, or `e2e`) and `production`/`true` are
/// rejected rather than accidentally enabling OTP logs.
fn email_stub_mode_from_vars(
    vars: &HashMap<String, String>,
) -> Result<Option<StubEmailMode>, AppError> {
    match non_empty(vars.get(EMAIL_STUB_MODE_ENV)) {
        Some(raw) => raw
            .parse::<StubEmailMode>()
            .map(Some)
            .map_err(|err| AppError::Config(format!("invalid {EMAIL_STUB_MODE_ENV}: {err}"))),
        None => Ok(None),
    }
}

/// Parse the outbound SMTP email relay config from the environment.
///
/// `Ok(None)` means there is no live SMTP config. The composition root may use a
/// `StubEmailSender` only when `stub_mode` is `Some`; otherwise the email sender
/// fails closed without logging OTPs. Any partial `CONSOLE_EMAIL_*` group is accepted
/// only in explicit stub mode, because production ConfigMap+missing-Secret paths
/// must not reach the OTP-logging stub.
fn email_config_from_vars(
    vars: &HashMap<String, String>,
    stub_mode: Option<StubEmailMode>,
) -> Result<Option<SmtpEmailConfig>, AppError> {
    let host = non_empty(vars.get("CONSOLE_EMAIL_SMTP_HOST"));
    let port_raw = non_empty(vars.get("CONSOLE_EMAIL_SMTP_PORT"));
    let username = non_empty(vars.get("CONSOLE_EMAIL_SMTP_USERNAME"));
    let password = non_empty(vars.get("CONSOLE_EMAIL_SMTP_PASSWORD"));
    let from_address = non_empty(vars.get("CONSOLE_EMAIL_FROM"));
    let from_name = non_empty(vars.get("CONSOLE_EMAIL_FROM_NAME"));
    let configured = host.is_some()
        || port_raw.is_some()
        || username.is_some()
        || password.is_some()
        || from_address.is_some()
        || from_name.is_some();
    if !configured {
        return Ok(None);
    }
    let missing_fields = [
        ("CONSOLE_EMAIL_SMTP_HOST", host.is_none()),
        ("CONSOLE_EMAIL_SMTP_PORT", port_raw.is_none()),
        ("CONSOLE_EMAIL_SMTP_USERNAME", username.is_none()),
        ("CONSOLE_EMAIL_SMTP_PASSWORD", password.is_none()),
        ("CONSOLE_EMAIL_FROM", from_address.is_none()),
        ("CONSOLE_EMAIL_FROM_NAME", from_name.is_none()),
    ]
    .into_iter()
    .filter_map(|(name, missing)| missing.then_some(name))
    .collect::<Vec<_>>();
    if !missing_fields.is_empty() {
        if let Some(mode) = stub_mode {
            tracing::warn!(
                email_stub_mode = %mode,
                missing = %missing_fields.join(", "),
                "CONSOLE_EMAIL_* partially configured; explicit non-production stub mode enabled, so OTP email uses the logging stub"
            );
            return Ok(None);
        }
        return Err(AppError::Config(format!(
            "CONSOLE_EMAIL_* is partially configured: missing {}; complete all SMTP fields for production startup or set {EMAIL_STUB_MODE_ENV}=local|dev|development|test|e2e only for explicit non-production stub OTP logging",
            missing_fields.join(", ")
        )));
    }
    let required = |value: Option<String>, name: &'static str| {
        value
            .ok_or_else(|| AppError::Config(format!("{name} is required when email is configured")))
    };
    let port = match port_raw {
        Some(raw) => raw
            .parse::<u16>()
            .map_err(|err| AppError::Config(format!("invalid CONSOLE_EMAIL_SMTP_PORT: {err}")))?,
        None => {
            return Err(AppError::Config(
                "CONSOLE_EMAIL_SMTP_PORT is required when email is configured".to_owned(),
            ));
        }
    };
    let config = SmtpEmailConfig {
        host: required(host, "CONSOLE_EMAIL_SMTP_HOST")?,
        port,
        username: required(username, "CONSOLE_EMAIL_SMTP_USERNAME")?,
        password: required(password, "CONSOLE_EMAIL_SMTP_PASSWORD")?,
        from_address: required(from_address, "CONSOLE_EMAIL_FROM")?,
        from_name: required(from_name, "CONSOLE_EMAIL_FROM_NAME")?,
    };
    config
        .validate()
        .map_err(|err| AppError::Config(err.to_string()))?;
    Ok(Some(config))
}

fn parse_time_duration_secs(
    raw: Option<&String>,
    default_secs: u64,
    name: &str,
) -> Result<time::Duration, AppError> {
    let secs = match raw {
        Some(raw) => raw
            .parse::<i64>()
            .map_err(|err| AppError::Config(format!("invalid {name}: {err}")))?,
        None => i64::try_from(default_secs)
            .map_err(|err| AppError::Config(format!("invalid default for {name}: {err}")))?,
    };
    Ok(time::Duration::seconds(secs))
}

fn non_empty(value: Option<&String>) -> Option<String> {
    value.and_then(|raw| {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_owned())
        }
    })
}

#[derive(Debug, Clone)]
pub enum DatabaseDependency {
    NotConfigured,
    Postgres(PgPool),
}

#[derive(Clone)]
pub struct AppState {
    config: AppConfig,
    database: DatabaseDependency,
    /// Narrow command pool for leave mutations. It is intentionally separate
    /// from the general runtime pool so PostgreSQL grants remain the final,
    /// non-bypassable authority boundary even if an API handler is compromised.
    leave_command_database: DatabaseDependency,
    /// Narrow command pool for ontology schema mutations and canonical seeding.
    ontology_command_database: DatabaseDependency,
    /// Dedicated command pool for platform tenant force removal. It is never
    /// substituted with the tenant-scoped runtime pool.
    platform_force_command_database: DatabaseDependency,
    jwt_verifier: Option<JwtVerifier>,
    /// JWT issuer used ONLY by the PLATFORM view-as START path to mint
    /// short-lived read-only impersonation tokens. Built from the same ES256
    /// keypair as the auth-rest issuer (so view-as tokens verify with the live
    /// `jwt_verifier`). `None` when no private key is configured — the view-as
    /// START endpoint then returns 503.
    view_as_issuer: Option<JwtIssuer>,
    /// Fresh passkey step-up verifier for sensitive policy-authoring actions.
    /// Built from the auth-rest WebAuthn RP settings; if auth-rest is disabled,
    /// policy lifecycle changes fail closed instead of accepting bearer-only
    /// publish/rollback requests.
    policy_step_up: Option<PasskeyService>,
    auth_rest: Option<AuthRestState>,
    evidence_storage: Option<EvidenceService<SeaweedS3Storage>>,
    /// Object store + bucket backing the public storefront media-serve route.
    sales_media_storage: Option<(SeaweedS3Storage, String)>,
    /// WORM object store + replica (compliance-locked) bucket the evidence
    /// fixity check HEADs. `None` when object storage is unconfigured — the
    /// evidence `verify` endpoint then 503s rather than green-lighting an
    /// unverifiable object.
    worm_evidence_storage: Option<(SeaweedS3Storage, String)>,
    dispatch_job_queue: Option<Arc<dyn JobQueue>>,
    push_notifier: Option<Arc<dyn PushNotifier>>,
    /// Outbound email sender for the open-signup OTP (#38). Always present: a
    /// `LettreSmtpSender` when live SMTP is configured, an explicitly-enabled
    /// non-prod `StubEmailSender` when `CONSOLE_EMAIL_STUB_MODE` is set, otherwise a
    /// fail-closed sender that never logs OTPs.
    email_sender: Arc<dyn EmailSender>,
    realtime_hub: Option<Arc<PgRealtimeHub>>,
    realtime_bridge: Option<PostgresBridgeHandle>,
    /// The webmail master-key cipher (envelope AEAD for SMTP/IMAP credentials).
    /// `None` when `CONSOLE_MAIL_MASTER_KEY` is absent at boot — the app STILL boots
    /// and the mail router still mounts. Read-only mail endpoints degrade to a
    /// clean no-account/empty state; credential-using endpoints return a clear
    /// `503 email_not_configured`. The cipher feature is lazily/optionally init'd
    /// so a missing key is never a panic.
    mail_cipher: Option<Arc<EnvelopeCredentialCipher>>,
    /// The inbound webmail IMAP sync worker handle. `None` when the worker is OFF
    /// (no KEK / no storage / `CONSOLE_MAIL_ENABLED` unset). Held so its lifetime is
    /// tied to the running `AppState` and it stops on shutdown.
    mail_sync_handle: Option<Arc<mail_sync::MailSyncHandle>>,
    /// Shared verifier for the read-only audit-chain attestation endpoint.
    /// When [`AppConfig::audit_chain_external`] is set this is an
    /// `ExternalSealSigner` (pinned anchors); otherwise an in-memory dev
    /// verifier that is **not** a trust root.
    audit_attestation_signer: Arc<dyn SealSigner>,
}

impl AppState {
    pub fn new(config: AppConfig, database: DatabaseDependency) -> Result<Self, AppError> {
        let jwt_verifier = config
            .jwt
            .as_ref()
            .map(JwtVerifierConfig::build)
            .transpose()?;
        // The view-as issuer is built from the SAME ES256 keypair as auth-rest,
        // so impersonation tokens it mints verify against the live `jwt_verifier`.
        // The per-call TTL override (≤30 min) is enforced in the START handler;
        // the issuer's default TTL here is a backstop of the same length.
        let view_as_issuer = config
            .auth_rest
            .as_ref()
            .map(build_view_as_issuer)
            .transpose()?;
        let policy_step_up = config
            .auth_rest
            .as_ref()
            .map(policy_step_up_from_auth_config)
            .transpose()?;
        let email_sender: Arc<dyn EmailSender> = match config.email.clone() {
            Some(email_config) => Arc::new(
                LettreSmtpSender::new(email_config)
                    .map_err(|err| AppError::Config(format!("invalid email config: {err}")))?,
            ),
            None => match config.email_stub_mode {
                Some(mode) => Arc::new(StubEmailSender::new(mode)),
                None => Arc::new(DisabledEmailSender),
            },
        };
        let auth_rest = match &database {
            DatabaseDependency::Postgres(pool) => match &config.auth_rest {
                Some(auth_config) => Some(
                    AuthRestState::new(pool.clone(), auth_config.clone())
                        .map_err(|err| {
                            AppError::Config(format!("invalid auth REST config: {err}"))
                        })?
                        .with_email_sender(email_sender.clone()),
                ),
                None => Some(AuthRestState::disabled(pool.clone())),
            },
            DatabaseDependency::NotConfigured => None,
        };
        let audit_attestation_signer =
            audit_chain_signer::build_attestation_signer(config.audit_chain_external.as_ref())?;
        let realtime_hub = realtime_hub_from_database(&database);

        Ok(Self {
            config,
            database,
            leave_command_database: DatabaseDependency::NotConfigured,
            ontology_command_database: DatabaseDependency::NotConfigured,
            platform_force_command_database: DatabaseDependency::NotConfigured,
            jwt_verifier,
            view_as_issuer,
            policy_step_up,
            auth_rest,
            evidence_storage: None,
            sales_media_storage: None,
            worm_evidence_storage: None,
            dispatch_job_queue: None,
            push_notifier: None,
            email_sender,
            realtime_hub,
            realtime_bridge: None,
            mail_cipher: None,
            mail_sync_handle: None,
            audit_attestation_signer,
        })
    }

    /// Attach the isolated `console_leave_cmd` command pool used for protected
    /// leave and People mutations. Production configuration constructs this
    /// pool only from `LEAVE_COMMAND_DATABASE_URL`; this builder supports
    /// explicit dependency injection for integration composition.
    #[must_use]
    pub fn with_leave_command_database(mut self, pool: PgPool) -> Self {
        self.leave_command_database = DatabaseDependency::Postgres(pool);
        self
    }

    #[must_use]
    pub fn with_platform_force_command_database(mut self, pool: PgPool) -> Self {
        self.platform_force_command_database = DatabaseDependency::Postgres(pool);
        self
    }

    pub async fn from_config(config: AppConfig) -> Result<Self, AppError> {
        let database = match config.database_url.as_deref() {
            Some(url) => {
                let after_connect_role = "console_rt".to_owned();
                let pool = PgPoolOptions::new()
                    .max_connections(RUNTIME_DATABASE_POOL_MAX_CONNECTIONS)
                    .acquire_timeout(Duration::from_secs(3))
                    // Tenant-isolation backstop. The app connects as the non-owner
                    // `console_rt` role under RLS keyed on the `app.current_org` GUC.
                    // Every query sets that GUC with SET LOCAL (transaction-scoped,
                    // auto-cleared on COMMIT/ROLLBACK), so it cannot normally
                    // persist. RESET ALL on release is defense-in-depth: if any
                    // future path ever set a *session*-level GUC, this clears it
                    // before the pooled connection is reused, so a tenant's
                    // `app.current_org` can never bleed into the next request.
                    // (RESET ALL keeps prepared statements, unlike DISCARD ALL.)
                    .after_connect(move |conn, _meta| {
                        let expected_role = after_connect_role.clone();
                        Box::pin(async move {
                            validate_database_connection_identity(
                                conn,
                                "DATABASE_URL",
                                &expected_role,
                            )
                            .await
                        })
                    })
                    .after_release(|conn, _meta| {
                        Box::pin(reset_serving_database_connection(
                            conn,
                            "DATABASE_URL",
                            "console_rt",
                        ))
                    })
                    .connect(url)
                    .await
                    .map_err(AppError::Database)?;
                validate_database_pool_identity(&pool, "DATABASE_URL", "console_rt").await?;
                DatabaseDependency::Postgres(pool)
            }
            None => DatabaseDependency::NotConfigured,
        };

        let leave_command_database = match (
            config.role,
            config.database_url.as_ref(),
            config.leave_command_database_url.as_deref(),
        ) {
            (AppRole::Api, Some(_), Some(url)) => DatabaseDependency::Postgres(
                connect_command_pool(url, "console_leave_cmd", "LEAVE_COMMAND_DATABASE_URL")
                    .await?,
            ),
            _ => DatabaseDependency::NotConfigured,
        };
        let ontology_command_database = match (
            config.role,
            config.database_url.as_ref(),
            config.ontology_command_database_url.as_deref(),
        ) {
            (AppRole::Api, Some(_), Some(url)) => DatabaseDependency::Postgres(
                connect_command_pool(url, "console_ontology_cmd", "ONTOLOGY_COMMAND_DATABASE_URL")
                    .await?,
            ),
            _ => DatabaseDependency::NotConfigured,
        };
        let platform_force_command_database = match (
            config.role,
            config.database_url.as_ref(),
            config.platform_force_command_database_url.as_deref(),
        ) {
            (AppRole::Api, Some(_), Some(url)) => DatabaseDependency::Postgres(
                connect_command_pool(
                    url,
                    "console_platform_force_cmd",
                    "PLATFORM_FORCE_COMMAND_DATABASE_URL",
                )
                .await?,
            ),
            _ => DatabaseDependency::NotConfigured,
        };

        let mut state = Self::new(config.clone(), database)?;
        state.leave_command_database = leave_command_database;
        state.ontology_command_database = ontology_command_database;
        state.platform_force_command_database = platform_force_command_database;
        if let (DatabaseDependency::Postgres(pool), Some(storage_config)) =
            (&state.database, config.storage.as_ref())
        {
            let object_store = SeaweedS3Storage::from_config(storage_config)
                .await
                .map_err(AppError::Storage)?;
            state.sales_media_storage =
                Some((object_store.clone(), storage_config.primary_bucket.clone()));
            state.worm_evidence_storage =
                Some((object_store.clone(), storage_config.replica_bucket.clone()));
            state.evidence_storage = Some(EvidenceService::new(
                pool.clone(),
                object_store,
                storage_config.primary_bucket.clone(),
                storage_config.replica_bucket.clone(),
            ));
        }
        if let (DatabaseDependency::Postgres(_), Some(database_url)) =
            (&state.database, config.database_url.as_deref())
            && config.dispatch_jobs_enabled
        {
            let queue = ApalisPostgresJobQueue::connect(database_url, "console.dispatch")
                .await
                .map_err(|err| AppError::Config(format!("invalid dispatch job queue: {err}")))?;
            state.dispatch_job_queue = Some(Arc::new(queue));
        }
        let fcm = config
            .fcm
            .clone()
            .map(FcmHttpV1Client::new)
            .transpose()
            .map_err(|err| AppError::Config(format!("invalid FCM config: {err}")))?;
        let solapi = config
            .solapi
            .clone()
            .map(SolapiAlimtalkClient::new)
            .transpose()
            .map_err(|err| AppError::Config(format!("invalid Solapi config: {err}")))?;
        if let Some(reason) = &config.solapi_disabled_reason {
            tracing::warn!(reason = %reason, "Solapi Alimtalk escalation disabled");
        }
        if fcm.is_some() || solapi.is_some() {
            state.push_notifier = Some(Arc::new(ProviderPushNotifier::new(fcm, solapi)));
        }
        if let Some(email_config) = config.email.clone() {
            let sender = LettreSmtpSender::new(email_config)
                .map_err(|err| AppError::Config(format!("invalid email config: {err}")))?;
            state.email_sender = Arc::new(sender);
        } else if let Some(mode) = config.email_stub_mode {
            state.email_sender = Arc::new(StubEmailSender::new(mode));
            tracing::warn!(
                email_stub_mode = %mode,
                "CONSOLE_EMAIL_STUB_MODE enabled: outbound OTP email uses the logging stub (non-production only)"
            );
        } else {
            tracing::warn!(
                "CONSOLE_EMAIL_* unset and CONSOLE_EMAIL_STUB_MODE disabled: outbound OTP email fails closed without logging OTPs"
            );
        }
        // Hand the resolved OTP email sender to the auth REST layer so its
        // open-signup endpoint (#38) can deliver the code. Done here (not in
        // `AppState::new`) because the live SMTP sender is only known once
        // `config.email` is resolved above.
        if let Some(auth_rest) = state.auth_rest.take() {
            state.auth_rest = Some(auth_rest.with_email_sender(state.email_sender.clone()));
        }
        // Webmail master key (envelope AEAD KEK) — GRACEFULLY OPTIONAL. When
        // `CONSOLE_MAIL_MASTER_KEY` is present + valid it arms the webmail credential
        // cipher; when it is absent the app STILL boots and the mail router still
        // mounts (so the OpenAPI paths exist). Read-only mail endpoints degrade to
        // a clean no-account/empty state; credential-using endpoints return a clear
        // `503 email_not_configured`. If it is present but malformed, that's a
        // boot error so it is caught immediately rather than at first use.
        match EnvelopeCredentialCipher::from_env() {
            Ok(cipher) => state.mail_cipher = Some(Arc::new(cipher)),
            Err(_) if std::env::var(console_comms_credential_cipher::MASTER_KEY_ENV).is_err() => {
                tracing::info!(
                    "CONSOLE_MAIL_MASTER_KEY unset: credential-using webmail endpoints are unavailable; read paths stay clean and the app boots normally"
                );
            }
            Err(_) => {
                return Err(AppError::Config(
                    "CONSOLE_MAIL_MASTER_KEY is set but is not a valid base64 32-byte key"
                        .to_owned(),
                ));
            }
        }
        if let Some(hub) = state.realtime_hub.clone() {
            state.realtime_bridge = Some(
                hub.start_postgres_listener()
                    .await
                    .map_err(AppError::Realtime)?,
            );
        }
        // Inbound webmail sync worker (B-mail-3). Spawned like the realtime
        // listener: a background loop on the app pool that arms `app.current_org`
        // per tenant for each sync pass. GRACEFUL — only runs when the master KEK,
        // object storage, and `CONSOLE_MAIL_ENABLED` are all present; otherwise a
        // no-op so the app boots normally and mail endpoints still mount.
        //
        // WORKER-ROLE ONLY: this background ticker belongs on the worker, never on
        // the horizontally-scaled API pods — every API replica running its own
        // ticker would sync the same mailboxes concurrently. Even on the worker it
        // is HA-safe because the due-account claim leases each row with FOR UPDATE
        // SKIP LOCKED (migration 0116), so >1 worker replica still claim disjoint
        // batches.
        if let DatabaseDependency::Postgres(pool) = &state.database
            && config.role == AppRole::Worker
        {
            state.mail_sync_handle = mail_sync::spawn(
                pool.clone(),
                state.mail_cipher.clone(),
                state.sales_media_storage.clone(),
                config.mail_enabled,
            )
            .map(Arc::new);
        }
        Ok(state)
    }

    pub fn config(&self) -> &AppConfig {
        &self.config
    }

    /// Outbound OTP email sender. Always present: live SMTP, explicit non-prod
    /// logging stub, or fail-closed disabled sender.
    #[must_use]
    pub fn email_sender(&self) -> Arc<dyn EmailSender> {
        self.email_sender.clone()
    }

    pub async fn shutdown_realtime(&self) {
        if let Some(bridge) = &self.realtime_bridge {
            bridge.shutdown();
        }
        if let Some(hub) = &self.realtime_hub {
            hub.shutdown().await;
        }
        if let Some(handle) = &self.mail_sync_handle {
            handle.shutdown();
        }
    }
}

/// Open a deliberately small command pool and prove the URL resolves to the
/// expected PostgreSQL login. URL string inequality alone cannot establish
/// credential separation (aliases and DSN parameters can obscure identity), so
/// startup also checks both authenticated identities and role membership before
/// any router receives the pool.
async fn connect_command_pool(
    url: &str,
    expected_role: &str,
    env_name: &str,
) -> Result<PgPool, AppError> {
    let after_connect_role = expected_role.to_owned();
    let after_connect_env = env_name.to_owned();
    let after_release_role = expected_role.to_owned();
    let after_release_env = env_name.to_owned();
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(Duration::from_secs(3))
        .after_connect(move |conn, _meta| {
            let expected_role = after_connect_role.clone();
            let env_name = after_connect_env.clone();
            Box::pin(async move {
                validate_database_connection_identity(conn, &env_name, &expected_role).await
            })
        })
        .after_release(move |conn, _meta| {
            let expected_role = after_release_role.clone();
            let env_name = after_release_env.clone();
            Box::pin(async move {
                reset_serving_database_connection(conn, &env_name, &expected_role).await
            })
        })
        .connect(url)
        .await
        .map_err(AppError::Database)?;
    validate_database_pool_identity(&pool, env_name, expected_role).await?;
    Ok(pool)
}

async fn reset_database_connection_state(conn: &mut PgConnection) -> Result<(), sqlx::Error> {
    sqlx::query("RESET SESSION AUTHORIZATION")
        .execute(&mut *conn)
        .await?;
    sqlx::query("RESET ROLE").execute(&mut *conn).await?;
    sqlx::query("RESET ALL").execute(&mut *conn).await?;
    Ok(())
}

async fn reset_serving_database_connection(
    conn: &mut PgConnection,
    env_name: &str,
    expected_role: &str,
) -> Result<bool, sqlx::Error> {
    // Any error from an `after_release` hook makes SQLx hard-close the
    // connection rather than return a partially cleaned session to the pool.
    reset_database_connection_state(conn).await?;
    Ok(
        validate_database_connection_identity(conn, env_name, expected_role)
            .await
            .is_ok(),
    )
}

async fn validate_database_connection_identity(
    conn: &mut PgConnection,
    env_name: &str,
    expected_role: &str,
) -> Result<(), sqlx::Error> {
    let readback =
        sqlx::query_as::<_, ServingDatabaseConnectionReadback>(serving_database_identity_query())
            .fetch_one(conn)
            .await?;
    ensure_expected_serving_database_identity(
        env_name,
        &readback.session_user,
        &readback.current_user,
        expected_role,
        RoleAttributes {
            can_login: readback.can_login,
            is_superuser: readback.is_superuser,
            bypasses_rls: readback.bypasses_rls,
            inherits_privileges: readback.inherits_privileges,
            can_create_db: readback.can_create_db,
            can_create_role: readback.can_create_role,
            can_replicate: readback.can_replicate,
        },
        readback.has_forbidden_membership_edge,
    )
    .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
    ensure_expected_serving_database_timeouts(
        env_name,
        &readback.statement_timeout,
        &readback.idle_in_transaction_session_timeout,
        &readback.transaction_timeout,
        readback.statement_timeout_matches,
        readback.idle_in_transaction_session_timeout_matches,
        readback.transaction_timeout_matches,
    )
}

#[derive(sqlx::FromRow)]
struct ServingDatabaseConnectionReadback {
    session_user: String,
    current_user: String,
    can_login: bool,
    is_superuser: bool,
    bypasses_rls: bool,
    inherits_privileges: bool,
    can_create_db: bool,
    can_create_role: bool,
    can_replicate: bool,
    has_forbidden_membership_edge: bool,
    statement_timeout: String,
    idle_in_transaction_session_timeout: String,
    transaction_timeout: String,
    statement_timeout_matches: bool,
    idle_in_transaction_session_timeout_matches: bool,
    transaction_timeout_matches: bool,
}

fn serving_database_identity_query() -> &'static str {
    r#"SELECT session_user::text,
              current_user::text,
              authenticated.rolcanlogin AS can_login,
              authenticated.rolsuper AS is_superuser,
              authenticated.rolbypassrls AS bypasses_rls,
              authenticated.rolinherit AS inherits_privileges,
              authenticated.rolcreatedb AS can_create_db,
              authenticated.rolcreaterole AS can_create_role,
              authenticated.rolreplication AS can_replicate,
              EXISTS (
                  SELECT 1
                  FROM pg_catalog.pg_roles AS candidate
                  WHERE candidate.rolname <> session_user
                    AND pg_catalog.pg_has_role(session_user, candidate.oid, 'MEMBER')
              ) OR EXISTS (
                  SELECT 1
                  FROM pg_catalog.pg_auth_members AS membership
                  WHERE membership.roleid = authenticated.oid
              ) AS has_forbidden_membership_edge,
              current_setting('statement_timeout') AS statement_timeout,
              current_setting('idle_in_transaction_session_timeout')
                  AS idle_in_transaction_session_timeout,
              current_setting('transaction_timeout') AS transaction_timeout,
              current_setting('statement_timeout')::interval = interval '30 seconds'
                  AS statement_timeout_matches,
              current_setting('idle_in_transaction_session_timeout')::interval = interval '30 seconds'
                  AS idle_in_transaction_session_timeout_matches,
              current_setting('transaction_timeout')::interval = interval '45 seconds'
                  AS transaction_timeout_matches
       FROM pg_catalog.pg_roles AS authenticated
       WHERE authenticated.rolname = session_user"#
}

async fn validate_database_pool_identity(
    pool: &PgPool,
    env_name: &str,
    expected_role: &str,
) -> Result<(), AppError> {
    let mut conn = pool.acquire().await.map_err(AppError::Database)?;

    validate_database_connection_identity(&mut conn, env_name, expected_role)
        .await
        .map_err(AppError::Database)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RoleAttributes {
    can_login: bool,
    is_superuser: bool,
    bypasses_rls: bool,
    inherits_privileges: bool,
    can_create_db: bool,
    can_create_role: bool,
    can_replicate: bool,
}

impl RoleAttributes {
    const HARDENED_LOGIN: Self = Self {
        can_login: true,
        is_superuser: false,
        bypasses_rls: false,
        inherits_privileges: false,
        can_create_db: false,
        can_create_role: false,
        can_replicate: false,
    };

    const HARDENED_MIGRATION_LOGIN: Self = Self {
        bypasses_rls: true,
        inherits_privileges: true,
        ..Self::HARDENED_LOGIN
    };

    const HARDENED_DEFINER: Self = Self {
        can_login: false,
        ..Self::HARDENED_LOGIN
    };
}

fn ensure_expected_serving_database_identity(
    env_name: &str,
    session_user: &str,
    current_user: &str,
    expected_role: &str,
    attributes: RoleAttributes,
    has_forbidden_membership_edge: bool,
) -> Result<(), AppError> {
    if session_user != expected_role || current_user != expected_role {
        return Err(AppError::Config(format!(
            "{env_name} must authenticate directly as PostgreSQL role {expected_role:?}; \
             session_user={session_user:?}, current_user={current_user:?}"
        )));
    }
    if attributes != RoleAttributes::HARDENED_LOGIN {
        return Err(AppError::Config(format!(
            "{env_name} PostgreSQL role {expected_role:?} must be LOGIN, NOINHERIT, NOSUPERUSER, \
             NOBYPASSRLS, NOCREATEDB, NOCREATEROLE, and NOREPLICATION"
        )));
    }
    if has_forbidden_membership_edge {
        return Err(AppError::Config(format!(
            "{env_name} PostgreSQL role {expected_role:?} must not participate in any direct or inherited role membership edge"
        )));
    }
    Ok(())
}

fn ensure_expected_serving_database_timeouts(
    env_name: &str,
    statement_timeout: &str,
    idle_in_transaction_session_timeout: &str,
    transaction_timeout: &str,
    statement_timeout_matches: bool,
    idle_in_transaction_session_timeout_matches: bool,
    transaction_timeout_matches: bool,
) -> Result<(), sqlx::Error> {
    if statement_timeout_matches
        && idle_in_transaction_session_timeout_matches
        && transaction_timeout_matches
    {
        return Ok(());
    }

    Err(sqlx::Error::Protocol(format!(
        "{env_name} serving connection timeout readback failed: \
         statement_timeout={statement_timeout:?}, \
         idle_in_transaction_session_timeout={idle_in_transaction_session_timeout:?}, \
         transaction_timeout={transaction_timeout:?}; \
         required statement_timeout={SERVING_STATEMENT_TIMEOUT}, \
         idle_in_transaction_session_timeout={SERVING_IDLE_IN_TRANSACTION_SESSION_TIMEOUT}, \
         and transaction_timeout={SERVING_TRANSACTION_TIMEOUT}"
    )))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RoleMembership {
    role_name: String,
    admin_option: bool,
    inherit_option: bool,
    set_option: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SubordinateRoleContract {
    role_name: String,
    attributes: RoleAttributes,
    has_unexpected_membership: bool,
}

async fn validate_migration_database_connection(
    conn: &mut PgConnection,
) -> Result<(), sqlx::Error> {
    let (
        session_user,
        current_user,
        database_owner,
        can_login,
        is_superuser,
        bypasses_rls,
        inherits_privileges,
        can_create_db,
        can_create_role,
        can_replicate,
    ): (
        String,
        String,
        String,
        bool,
        bool,
        bool,
        bool,
        bool,
        bool,
        bool,
    ) = sqlx::query_as(
        r#"SELECT session_user::text,
                  current_user::text,
                  pg_catalog.pg_get_userbyid(database.datdba),
                  authenticated.rolcanlogin,
                  authenticated.rolsuper,
                  authenticated.rolbypassrls,
                  authenticated.rolinherit,
                  authenticated.rolcreatedb,
                  authenticated.rolcreaterole,
                  authenticated.rolreplication
           FROM pg_catalog.pg_roles AS authenticated
           JOIN pg_catalog.pg_database AS database
             ON database.datname = current_database()
           WHERE authenticated.rolname = session_user"#,
    )
    .fetch_one(&mut *conn)
    .await?;

    let memberships = sqlx::query_as::<_, (String, bool, bool, bool)>(
        r#"SELECT granted.rolname,
                  membership.admin_option,
                  membership.inherit_option,
                  membership.set_option
           FROM pg_catalog.pg_auth_members AS membership
           JOIN pg_catalog.pg_roles AS granted ON granted.oid = membership.roleid
           JOIN pg_catalog.pg_roles AS member ON member.oid = membership.member
           WHERE member.rolname = 'console_app'
           ORDER BY granted.rolname"#,
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(
        |(role_name, admin_option, inherit_option, set_option)| RoleMembership {
            role_name,
            admin_option,
            inherit_option,
            set_option,
        },
    )
    .collect::<Vec<_>>();

    let subordinate_roles =
        sqlx::query_as::<_, (String, bool, bool, bool, bool, bool, bool, bool, bool)>(
            r#"SELECT subordinate.rolname,
                  subordinate.rolcanlogin,
                  subordinate.rolsuper,
                  subordinate.rolbypassrls,
                  subordinate.rolinherit,
                  subordinate.rolcreatedb,
                  subordinate.rolcreaterole,
                  subordinate.rolreplication,
                  EXISTS (
                      SELECT 1
                      FROM pg_catalog.pg_roles AS candidate
                      WHERE candidate.rolname <> subordinate.rolname
                        AND pg_catalog.pg_has_role(subordinate.oid, candidate.oid, 'MEMBER')
                  )
           FROM pg_catalog.pg_roles AS subordinate
           WHERE subordinate.rolname IN ('console_leave_definer', 'console_ontology_writer')
           ORDER BY subordinate.rolname"#,
        )
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(
            |(
                role_name,
                can_login,
                is_superuser,
                bypasses_rls,
                inherits_privileges,
                can_create_db,
                can_create_role,
                can_replicate,
                has_unexpected_membership,
            )| SubordinateRoleContract {
                role_name,
                attributes: RoleAttributes {
                    can_login,
                    is_superuser,
                    bypasses_rls,
                    inherits_privileges,
                    can_create_db,
                    can_create_role,
                    can_replicate,
                },
                has_unexpected_membership,
            },
        )
        .collect::<Vec<_>>();

    let has_unexpected_application_membership_edge: bool = sqlx::query_scalar(
        r#"SELECT EXISTS (
               SELECT 1
               FROM pg_catalog.pg_auth_members AS membership
               JOIN pg_catalog.pg_roles AS granted ON granted.oid = membership.roleid
               JOIN pg_catalog.pg_roles AS member ON member.oid = membership.member
               WHERE (
                   granted.rolname IN (
                       'console_app', 'console_rt', 'console_leave_definer', 'console_leave_cmd',
                       'console_ontology_writer', 'console_ontology_cmd', 'console_platform_force_cmd'
                   )
                   OR member.rolname IN (
                       'console_app', 'console_rt', 'console_leave_definer', 'console_leave_cmd',
                       'console_ontology_writer', 'console_ontology_cmd', 'console_platform_force_cmd'
                   )
               )
               AND NOT (
                   member.rolname = 'console_app'
                   AND granted.rolname IN ('console_leave_definer', 'console_ontology_writer')
                   AND NOT membership.admin_option
                   AND membership.inherit_option
                   AND membership.set_option
               )
           )"#,
    )
    .fetch_one(&mut *conn)
    .await?;

    // Database ownership implicitly makes console_app a member of
    // pg_database_owner. Apart from that safe database-local capability and
    // the two direct SET edges required to transfer SECURITY DEFINER function
    // ownership, the migration login must not inherit any effective role.
    let has_unexpected_effective_migration_membership: bool = sqlx::query_scalar(
        r#"SELECT EXISTS (
               SELECT 1
               FROM pg_catalog.pg_roles AS candidate
               WHERE candidate.rolname <> session_user
                 AND candidate.rolname NOT IN (
                     'pg_database_owner', 'console_leave_definer', 'console_ontology_writer'
                 )
                 AND pg_catalog.pg_has_role(session_user, candidate.oid, 'MEMBER')
           )"#,
    )
    .fetch_one(&mut *conn)
    .await?;

    ensure_expected_migration_database_identity(
        &session_user,
        &current_user,
        &database_owner,
        RoleAttributes {
            can_login,
            is_superuser,
            bypasses_rls,
            inherits_privileges,
            can_create_db,
            can_create_role,
            can_replicate,
        },
        &memberships,
        &subordinate_roles,
        has_unexpected_application_membership_edge || has_unexpected_effective_migration_membership,
    )
    .map_err(|error| sqlx::Error::Protocol(error.to_string()))
}

fn ensure_expected_migration_database_budgets(
    lock_timeout: &str,
    statement_timeout: &str,
    lock_timeout_matches: bool,
    statement_timeout_matches: bool,
) -> Result<(), sqlx::Error> {
    if lock_timeout_matches && statement_timeout_matches {
        return Ok(());
    }

    Err(sqlx::Error::Protocol(format!(
        "migration connection budget readback failed: lock_timeout={lock_timeout:?}, \
         statement_timeout={statement_timeout:?}; required lock_timeout={MIGRATION_LOCK_TIMEOUT} \
         and statement_timeout={MIGRATION_STATEMENT_TIMEOUT}"
    )))
}

async fn enforce_migration_database_budgets(conn: &mut PgConnection) -> Result<(), sqlx::Error> {
    sqlx::query("SET SESSION lock_timeout = '5s'")
        .execute(&mut *conn)
        .await?;
    sqlx::query("SET SESSION statement_timeout = '60s'")
        .execute(&mut *conn)
        .await?;

    let (lock_timeout, statement_timeout, lock_timeout_matches, statement_timeout_matches): (
        String,
        String,
        bool,
        bool,
    ) = sqlx::query_as(
        r#"SELECT current_setting('lock_timeout'),
                  current_setting('statement_timeout'),
                  current_setting('lock_timeout')::interval = interval '5 seconds',
                  current_setting('statement_timeout')::interval = interval '60 seconds'"#,
    )
    .fetch_one(&mut *conn)
    .await?;

    ensure_expected_migration_database_budgets(
        &lock_timeout,
        &statement_timeout,
        lock_timeout_matches,
        statement_timeout_matches,
    )
}

async fn prepare_migration_database_connection(conn: &mut PgConnection) -> Result<(), sqlx::Error> {
    enforce_migration_database_budgets(conn).await?;
    validate_migration_database_connection(conn).await
}

async fn reset_migration_database_connection(conn: &mut PgConnection) -> Result<bool, sqlx::Error> {
    reset_database_connection_state(conn).await?;
    enforce_migration_database_budgets(conn).await?;
    Ok(true)
}

fn migration_database_pool_options() -> PgPoolOptions {
    PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(10))
        .after_connect(|conn, _meta| {
            Box::pin(async move { prepare_migration_database_connection(conn).await })
        })
        .after_release(|conn, _meta| {
            Box::pin(async move { reset_migration_database_connection(conn).await })
        })
}

fn ensure_expected_migration_database_identity(
    session_user: &str,
    current_user: &str,
    database_owner: &str,
    attributes: RoleAttributes,
    memberships: &[RoleMembership],
    subordinate_roles: &[SubordinateRoleContract],
    has_unexpected_application_membership_edge: bool,
) -> Result<(), AppError> {
    if session_user != "console_app" || current_user != "console_app" {
        return Err(AppError::Config(format!(
            "DATABASE_URL migration connection must authenticate directly as PostgreSQL role \
             \"console_app\"; session_user={session_user:?}, current_user={current_user:?}"
        )));
    }
    if database_owner != "console_app" {
        return Err(AppError::Config(
            "DATABASE_URL migration database must be owned by PostgreSQL role \"console_app\""
                .to_owned(),
        ));
    }
    if attributes != RoleAttributes::HARDENED_MIGRATION_LOGIN {
        return Err(AppError::Config(
            "DATABASE_URL migration role \"console_app\" must be LOGIN, INHERIT, NOSUPERUSER, \
             BYPASSRLS, NOCREATEDB, NOCREATEROLE, and NOREPLICATION"
                .to_owned(),
        ));
    }

    const EXPECTED_DEFINERS: [&str; 2] = ["console_leave_definer", "console_ontology_writer"];
    if memberships.len() != EXPECTED_DEFINERS.len()
        || EXPECTED_DEFINERS.iter().any(|expected| {
            !memberships.iter().any(|membership| {
                membership.role_name == *expected
                    && !membership.admin_option
                    && membership.inherit_option
                    && membership.set_option
            })
        })
    {
        return Err(AppError::Config(
            "DATABASE_URL migration role \"console_app\" must have exactly the console_leave_definer and \
             console_ontology_writer memberships with ADMIN false, INHERIT true, and SET true"
                .to_owned(),
        ));
    }
    if has_unexpected_application_membership_edge {
        return Err(AppError::Config(
            "migration application roles have a forbidden membership edge; only console_app membership in the two definer roles is permitted"
                .to_owned(),
        ));
    }
    if subordinate_roles.len() != EXPECTED_DEFINERS.len()
        || EXPECTED_DEFINERS.iter().any(|expected| {
            !subordinate_roles.iter().any(|subordinate| {
                subordinate.role_name == *expected
                    && subordinate.attributes == RoleAttributes::HARDENED_DEFINER
                    && !subordinate.has_unexpected_membership
            })
        })
    {
        return Err(AppError::Config(
            "migration definer roles must both be NOLOGIN, NOINHERIT, NOSUPERUSER, NOBYPASSRLS, \
             NOCREATEDB, NOCREATEROLE, NOREPLICATION, and have no role memberships"
                .to_owned(),
        ));
    }
    Ok(())
}

fn validate_database_url_identity(
    env_name: &str,
    raw_url: &str,
    expected_role: &str,
) -> Result<String, AppError> {
    let parsed = Url::parse(raw_url)
        .map_err(|_| AppError::Config(format!("{env_name} must be a valid PostgreSQL URL")))?;
    if !matches!(parsed.scheme(), "postgres" | "postgresql") {
        return Err(AppError::Config(format!(
            "{env_name} must use the postgres or postgresql URL scheme"
        )));
    }

    let username = decode_database_url_component(env_name, "username", parsed.username())?;
    let mut password = parsed
        .password()
        .map(|value| decode_database_url_component(env_name, "password", value))
        .transpose()?;

    for (key, value) in parsed.query_pairs() {
        if key == "user" {
            return Err(AppError::Config(format!(
                "{env_name} must not set PostgreSQL role through DSN options; name the login in the URL authority"
            )));
        } else if key == "password" {
            password = Some(value.into_owned());
        } else if (key == "options" && postgres_options_set_role(&value))
            || key
                .strip_prefix("options[")
                .and_then(|key| key.strip_suffix(']'))
                .is_some_and(|key| key.eq_ignore_ascii_case("role"))
        {
            return Err(AppError::Config(format!(
                "{env_name} must not set PostgreSQL role through DSN options"
            )));
        }
    }

    if username != expected_role {
        return Err(AppError::Config(format!(
            "{env_name} must name PostgreSQL login role {expected_role:?} directly"
        )));
    }
    password
        .filter(|password| !password.is_empty())
        .ok_or_else(|| {
            AppError::Config(format!(
                "{env_name} must contain a nonempty password credential"
            ))
        })
}

fn decode_database_url_component(
    env_name: &str,
    component_name: &str,
    value: &str,
) -> Result<String, AppError> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let Some(high) = bytes.get(index + 1).and_then(|value| hex_digit(*value)) else {
                return Err(AppError::Config(format!(
                    "{env_name} contains an invalid percent-encoded {component_name}"
                )));
            };
            let Some(low) = bytes.get(index + 2).and_then(|value| hex_digit(*value)) else {
                return Err(AppError::Config(format!(
                    "{env_name} contains an invalid percent-encoded {component_name}"
                )));
            };
            decoded.push((high << 4) | low);
            index += 3;
            continue;
        }
        decoded.push(bytes[index]);
        index += 1;
    }

    String::from_utf8(decoded).map_err(|_| {
        AppError::Config(format!(
            "{env_name} contains a non-UTF-8 percent-encoded {component_name}"
        ))
    })
}

fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn postgres_options_set_role(options: &str) -> bool {
    options.split_ascii_whitespace().any(|token| {
        let token = token.trim_start_matches(['-', '\\']);
        let assignment = token
            .strip_prefix('c')
            .filter(|assignment| assignment.contains('='))
            .unwrap_or(token);
        assignment
            .split_once('=')
            .is_some_and(|(name, _)| name.eq_ignore_ascii_case("role"))
    })
}

fn ensure_distinct_database_urls<const N: usize>(
    urls: [(&str, Option<&str>); N],
) -> Result<(), AppError> {
    for (index, (name, url)) in urls.iter().enumerate() {
        let Some(url) = url else {
            continue;
        };
        for (earlier_name, earlier_url) in &urls[..index] {
            if earlier_url.is_some_and(|earlier_url| earlier_url == *url) {
                return Err(AppError::Config(format!(
                    "{name} must be distinct from {earlier_name}"
                )));
            }
        }
    }
    Ok(())
}

fn ensure_distinct_database_credentials<const N: usize>(
    credentials: [(&str, Option<&str>); N],
) -> Result<(), AppError> {
    for (index, (left_name, left_password)) in credentials.iter().enumerate() {
        let Some(left_password) = left_password else {
            continue;
        };
        for (right_name, right_password) in &credentials[index + 1..] {
            if right_password.is_some_and(|right_password| right_password == *left_password) {
                return Err(AppError::Config(format!(
                    "{left_name} and {right_name} must use distinct password credentials"
                )));
            }
        }
    }
    Ok(())
}

/// Build the inbound-attachment object store the webmail read API uses for
/// presigned GETs, from the same storage config the evidence pipeline uses.
/// `None` when storage is unconfigured (the attachment-download endpoint 503s).
fn mail_attachment_store(state: &AppState) -> Option<console_comms_rest::SharedAttachmentStore> {
    state.sales_media_storage.as_ref().map(|(store, bucket)| {
        Arc::new(mail_sync::S3MailAttachmentStore::new(
            store.clone(),
            bucket.clone(),
        )) as console_comms_rest::SharedAttachmentStore
    })
}

// --- ontology §18 projected-dispatch registry (App-tier only; the ontology
// REST tier stays free of a domain-adapter edge, exactly like
// `TenantConfigSeeder` — see `console_ontology_rest::ProjectedDispatchRegistry`
// module docs) ---------------------------------------------------------------

/// Map a registry use-case error onto [`ActionError`] without touching the
/// ontology adapter's error type — the pattern every App-tier projected
/// handler follows.
fn registry_error_to_action_error(err: PgRegistryError) -> ActionError {
    match err {
        PgRegistryError::Domain(kernel) => ActionError::domain(kernel),
        PgRegistryError::Db(db) => ActionError::domain(KernelError::internal(db.to_string())),
        PgRegistryError::Workbook(message) => ActionError::domain(KernelError::internal(message)),
    }
}

/// `registry.update_equipment` projected dispatch: routes into the registry
/// crate's real `update_equipment` use-case (its own RLS + audit + versioning).
/// The ontology engine writes nothing of its own for this action (§9.3: no
/// second source of truth).
fn update_equipment_projected_handler(store: PgRegistryStore) -> ProjectedHandler {
    Arc::new(move |input: ProjectedDispatch| {
        let store = store.clone();
        Box::pin(async move {
            let equipment_uuid = input.target_id.ok_or_else(|| {
                ActionError::domain(KernelError::validation(
                    "update_equipment requires a target equipment id",
                ))
            })?;
            let status = input
                .params
                .get("status")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    ActionError::domain(KernelError::validation("status param is required"))
                })
                .and_then(|s| EquipmentStatus::parse(s).map_err(ActionError::domain))?;

            store
                .update_equipment(UpdateEquipmentCommand {
                    actor: input.principal.user_id,
                    // The projected dispatch is a second door onto the SAME
                    // use-case, so it carries the same branch scope; without it
                    // the ontology action would be a way around the check.
                    branch_scope: input.principal.branch_scope.clone(),
                    equipment_id: EquipmentId::from_uuid(equipment_uuid),
                    fields: UpdateEquipmentFields {
                        status: Some(status),
                        ..UpdateEquipmentFields::default()
                    },
                    trace: TraceContext::generate(),
                    occurred_at: input.occurred_at,
                })
                .await
                .map_err(registry_error_to_action_error)?;

            Ok(serde_json::json!({ "target": input.target, "target_id": equipment_uuid }))
        })
    })
}

/// The `console-app` half of the org-change → Employment port seam: it binds the
/// canonical adapter's `reassign_org_unit_via_transfers_in_tx` (the owner's
/// `hr.transfer` write) to the [`EmploymentTransferPort`] trait the orgchange
/// adapter declares. The orgchange adapter cannot take a Cargo edge on the
/// canonical adapter (adapter→adapter is illegal), so this crate — the
/// composition root, which the layer gate allows to see everything — is where
/// the two meet.
struct PgEmploymentTransferPort;

impl EmploymentTransferPort for PgEmploymentTransferPort {
    fn reassign_org_unit<'tx, 's>(
        &self,
        tx: &'tx mut sqlx::Transaction<'_, sqlx::Postgres>,
        org: OrgId,
        actor: UserId,
        op_command_id: uuid::Uuid,
        from_org_unit: &'s str,
        to_org_unit: &'s str,
        company: &'s str,
        valid_from: time::OffsetDateTime,
    ) -> TransferPortFuture<'tx> {
        let from = from_org_unit.to_owned();
        let to = to_org_unit.to_owned();
        let company = company.to_owned();
        Box::pin(async move {
            reassign_org_unit_via_transfers_in_tx(
                tx,
                org,
                actor,
                op_command_id,
                &from,
                &to,
                &company,
                valid_from,
            )
            .await
            // Re-derive the canonical KernelError kind without pulling the
            // `CanonicalPortError` trait into lib code (it is only a
            // dev-dependency of this crate), then redact Internal diagnostics at
            // the seam so the org-change REST surface never returns DB internals.
            // A closed window stays Conflict (409), a domain refusal stays
            // Validation (422), and a gate/lookup failure stays Internal (500).
            .map_err(|error| {
                let kernel = match &error {
                    EmploymentError::Frozen(kernel) => kernel.clone(),
                    EmploymentError::Database(_)
                    | EmploymentError::UnreadableReceipt(_, _)
                    | EmploymentError::UnreadableEffectiveState { .. } => {
                        KernelError::internal(error.to_string())
                    }
                    EmploymentError::DigestConflict(_)
                    | EmploymentError::UnresolvedReactivation(_) => {
                        KernelError::conflict(error.to_string())
                    }
                    EmploymentError::Blocked(_)
                    | EmploymentError::AmbiguousSourceBinding { .. }
                    | EmploymentError::UnknownOrgUnit(_)
                    | EmploymentError::UnknownJobPosition(_)
                    | EmploymentError::UnboundEmployeeForTransfer { .. }
                    | EmploymentError::OrgUnitRefNotUuid => {
                        KernelError::validation(error.to_string())
                    }
                };
                if kernel.kind == ErrorKind::Internal {
                    tracing::error!(
                        error = %kernel.message,
                        "employment transfer failed at the org-change seam"
                    );
                    KernelError::internal("employment transfer failed")
                } else {
                    kernel
                }
            })
        })
    }
}

/// The full projected-dispatch registry supplied to the ontology REST tier.
/// Unresolved targets fail closed (`NotWiredYet`) — see
/// `console_ontology_rest::ProjectedDispatchRegistry::dispatch`.
///
/// # Six lines for thirteen targets, and for the fourteenth
///
/// `register_port` is called once per canonical OBJECT, not once per action:
/// each call installs the port that owns every `dispatch_target` the contract
/// assigns to that object, so `hr.appoint`, `hr.promote` and `hr.transfer` are
/// one line between them. `ObjectKey` is LOCKED at six by
/// `six_projected_stable_object_keys_verbatim`, so this list is closed — a new
/// dispatch target lands in `dispatch_targets!` and in the owning port's query
/// enum, and needs no edit here at all. That is the whole point: the enumerable
/// thing is the one a contract test forbids from growing.
///
/// `registry.update_equipment` is not a roster member and keeps its hand-written
/// handler; it is the last of that shape.
fn projected_dispatch_registry(pool: PgPool) -> ProjectedDispatchRegistry {
    // Every port bridges a SYNCHRONOUS `execute` onto async `sqlx` with this
    // handle. `build_router` is only ever called from inside a runtime, and a
    // panic here is the correct failure: a router built without one would fail
    // every canonical action at request time instead of at startup.
    let runtime = tokio::runtime::Handle::current();
    ProjectedDispatchRegistry::new()
        .register(
            "registry.update_equipment",
            update_equipment_projected_handler(PgRegistryStore::new(pool.clone())),
        )
        .register_port(PgCompanyPort::new(pool.clone(), runtime.clone()))
        .register_port(PgOrgUnitPort::new(pool.clone(), runtime.clone()))
        .register_port(PgJobPositionPort::new(pool.clone(), runtime.clone()))
        .register_port(PgPersonPort::new(pool.clone(), runtime.clone()))
        .register_port(PgEmploymentPort::new(pool.clone(), runtime.clone()))
        .register_port(PgPayRunPort::new(pool, runtime))
}

/// The composition root is measured for COVERAGE, not for its shape: whatever
/// `dispatch_targets!` holds, the registry this crate hands the ontology tier
/// must resolve all of it. Names no target and no object, so a fourteenth
/// target is asserted the moment the contract declares one, and a dropped
/// `register_port` line is what makes it fail.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod projected_dispatch_coverage {
    use console_ontology_canonical_domain::DispatchTarget;
    #[cfg(not(feature = "test-postgres"))]
    #[tokio::test]
    async fn the_wired_registry_resolves_every_canonical_dispatch_target() {
        // Lazy: no connection is opened, and none is needed — resolution is a
        // property of the wiring, not of the database behind it.
        let pool = sqlx::postgres::PgPool::connect_lazy("postgres://console@127.0.0.1/console")
            .expect("a lazy pool never connects");
        let registry = super::projected_dispatch_registry(pool);

        let unresolved: Vec<&str> = DispatchTarget::ALL
            .iter()
            .filter(|target| !registry.resolves(**target))
            .map(|target| target.as_str())
            .collect();
        assert!(
            unresolved.is_empty(),
            "the deployed registry resolves no port for {unresolved:?}"
        );
    }
}

fn realtime_hub_from_database(database: &DatabaseDependency) -> Option<Arc<PgRealtimeHub>> {
    match database {
        DatabaseDependency::Postgres(pool) => Some(Arc::new(PgRealtimeHub::new(
            pool.clone(),
            Default::default(),
        ))),
        DatabaseDependency::NotConfigured => None,
    }
}

#[derive(Debug, Serialize)]
struct HealthBody<'a> {
    status: &'a str,
    service: String,
    role: AppRole,
}

#[derive(Debug, Serialize)]
struct ReadyBody<'a> {
    status: &'a str,
    write_ready: bool,
    service: String,
    role: AppRole,
    database: &'a str,
    leave_command_database: &'a str,
    ontology_command_database: &'a str,
    platform_force_command_database: &'a str,
}

#[derive(Debug, Deserialize, Clone)]
struct AuditQuery {
    limit: Option<i64>,
    offset: Option<i64>,
    target_type: Option<String>,
    target_id: Option<String>,
    actor: Option<uuid::Uuid>,
    trace_id: Option<String>,
}

#[derive(Debug, Clone)]
struct NormalizedAuditQuery {
    limit: i64,
    offset: i64,
    target_type: Option<String>,
    target_id: Option<String>,
    actor: Option<uuid::Uuid>,
    trace_id: Option<String>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct AuditRecord {
    id: uuid::Uuid,
    actor: Option<uuid::Uuid>,
    action: String,
    target_type: String,
    target_id: String,
    branch_id: Option<uuid::Uuid>,
    before_snap: Option<serde_json::Value>,
    after_snap: Option<serde_json::Value>,
    ip: Option<String>,
    user_agent: Option<String>,
    auth_method: Option<String>,
    device: Option<String>,
    classification_badges: Option<Vec<String>>,
    anomaly: Option<bool>,
    reason: Option<String>,
    trace_id: String,
    span_id: String,
    occurred_at: time::OffsetDateTime,
}

#[derive(Debug, Serialize)]
struct AuditPage {
    items: Vec<AuditRecord>,
    limit: i64,
    offset: i64,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: ErrorPayload,
}

#[derive(Debug, Serialize)]
struct ErrorPayload {
    code: &'static str,
    message: String,
}

// ---------------------------------------------------------------------------
// Prometheus metrics
// ---------------------------------------------------------------------------

/// Histogram name backing the OpenSLO availability + latency objectives
/// (`backend/app/slos/*.openslo.yaml`). The `metrics-exporter-prometheus`
/// renderer appends the standard `_bucket` / `_sum` / `_count` suffixes, and the
/// `s` unit suffix is already in the name, so the SLO queries
/// (`http_server_request_duration_seconds_bucket`,
/// `http_server_request_duration_seconds_count`) resolve against it directly.
const HTTP_DURATION_METRIC: &str = "http_server_request_duration_seconds";
const HTTP_ROUTE_UNMATCHED: &str = "/_unmatched";

/// Latency histogram boundaries in SECONDS. Chosen to bracket the 500ms p99 SLO
/// objective with resolution on either side.
const HTTP_LATENCY_BUCKETS: &[f64] = &[
    0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 10.0,
];

static METRICS_HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();
// Serializes concurrent installers so at most one thread ever calls
// `PrometheusBuilder::install_recorder()`. Without this, two threads racing
// the `METRICS_HANDLE.get()` fast-path miss can both reach `install_recorder`;
// the `metrics` crate's own global recorder slot rejects the loser, and the
// loser's fallback read of `METRICS_HANDLE` can still be empty (the winner
// hasn't written it yet) -- a genuine TOCTOU window that surfaced as a flaky
// "metrics recorder installs once" panic under parallel #[tokio::test]s.
static METRICS_INSTALL_LOCK: Mutex<()> = Mutex::new(());

/// Install the process-global Prometheus recorder once and return a render
/// handle. Idempotent: the first successful install wins and later calls (and a
/// lost install race) return that same handle. Call at startup before serving.
pub fn install_metrics_recorder() -> Result<PrometheusHandle, AppError> {
    if let Some(handle) = METRICS_HANDLE.get() {
        return Ok(handle.clone());
    }
    let _guard = METRICS_INSTALL_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // Re-check now that we hold the lock: another thread may have finished
    // installing while we were waiting for it.
    if let Some(handle) = METRICS_HANDLE.get() {
        return Ok(handle.clone());
    }
    match PrometheusBuilder::new()
        .set_buckets_for_metric(
            Matcher::Full(HTTP_DURATION_METRIC.to_owned()),
            HTTP_LATENCY_BUCKETS,
        )
        .and_then(PrometheusBuilder::install_recorder)
    {
        Ok(handle) => Ok(METRICS_HANDLE.get_or_init(|| handle).clone()),
        // Lost the install race against a caller outside this lock (e.g. a
        // non-console-app global installer in the same process) -- adopt the
        // winner's handle; only a genuine absence is an error.
        Err(err) => METRICS_HANDLE
            .get()
            .cloned()
            .ok_or_else(|| AppError::Telemetry(err.to_string())),
    }
}

/// Cardinality-safe route label for request metrics/traces.
///
/// An adjacent Oyatie implementation, `oya-http-wide-event-middleware-infrastructure`,
/// was evaluated as reusable source material, but that crate depends on Oyatie-only
/// tenancy and hyperscaler metrics kernels. This app already owns the portable OTLP exporter
/// and Prometheus recorder in this module, so this lane intentionally copies the
/// discipline rather than wiring the oyatie crate directly: use axum's matched
/// route template (`MatchedPath`) as the route label, never the raw URI path or
/// query string. The sentinel keeps unmatched/error paths bounded too.
fn cardinality_safe_http_route<B>(request: &Request<B>) -> &str {
    request
        .extensions()
        .get::<MatchedPath>()
        .map(MatchedPath::as_str)
        .unwrap_or(HTTP_ROUTE_UNMATCHED)
}

/// Middleware: time each request and record its duration (seconds) into the
/// `http_server_request_duration_seconds` histogram, labelled with the service
/// name, method, response status code, and MATCHED ROUTE TEMPLATE. Labels stay
/// bounded because `http_route` comes from axum's `MatchedPath` extension (e.g.
/// `/api/work-orders/{id}`) or the static `/_unmatched` sentinel — never from the
/// raw URL path or query string. A no-op until the recorder is installed.
async fn track_http_metrics(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response<Body> {
    let start = std::time::Instant::now();
    let method = request.method().as_str().to_owned();
    let http_route = cardinality_safe_http_route(&request).to_owned();
    let response = next.run(request).await;
    let status = response.status().as_u16();
    let elapsed = start.elapsed();
    let service_name = state.config.service_name.clone();
    metrics::histogram!(
        HTTP_DURATION_METRIC,
        "service_name" => service_name.clone(),
        "http_request_method" => method.clone(),
        "http_route" => http_route.clone(),
        "http_response_status_code" => status.to_string(),
    )
    .record(elapsed.as_secs_f64());
    tracing::info!(
        service_name = %service_name,
        http_request_method = %method,
        http_route = %http_route,
        http_response_status_code = status,
        latency_ms = elapsed.as_millis(),
        "http request wide event"
    );
    response
}

/// `GET /metrics` â Prometheus exposition. Internal-only: the ingress routes
/// `/api` to this server and everything else to the SPA, so `/metrics` is
/// reachable only in-cluster (e.g. by a ServiceMonitor scrape), never via the
/// public host.
async fn render_metrics() -> Response<Body> {
    match METRICS_HANDLE.get() {
        Some(handle) => (
            [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
            handle.render(),
        )
            .into_response(),
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            "metrics recorder not installed",
        )
            .into_response(),
    }
}

/// Wrap a fully-merged router with request metrics and expose `/metrics`. Applied
/// last so every route (base + all domain routers) is measured; `/metrics` is
/// added afterwards so it does not measure itself. Use `Router::layer` rather
/// than `route_layer` so the default 404 fallback is measured too; axum still
/// inserts `MatchedPath` before calling matched route endpoints, while unmatched
/// fallback requests use the bounded sentinel.
fn with_metrics(router: Router, state: &AppState) -> Router {
    router
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            track_http_metrics,
        ))
        .route("/metrics", get(render_metrics))
}

/// Build the `TraceLayer` applied to the fully-merged router so EVERY route
/// (base, domain, platform, realtime, auth) emits a request span. Tracing a
/// long-lived WS/SSE connection only logs its start/end, so this is safe to
/// apply even to the realtime route.
// The return type spells out the closure-parameterized `TraceLayer`; clippy's
// `type_complexity` lint fires on this builder-style signature, which is the
// idiomatic shape for a configured tower layer.
#[allow(clippy::type_complexity)]
fn http_trace_layer() -> TraceLayer<
    tower_http::classify::SharedClassifier<tower_http::classify::ServerErrorsAsFailures>,
    impl Fn(&Request<Body>) -> tracing::Span + Clone,
    impl Fn(&Request<Body>, &tracing::Span) + Clone,
    impl Fn(&Response<Body>, Duration, &tracing::Span) + Clone,
> {
    TraceLayer::new_for_http()
        .make_span_with(|request: &Request<Body>| {
            tracing::info_span!(
                "http.request",
                method = %request.method(),
                // Matched route template ONLY — never the raw path or query
                // string. A raw path can carry object IDs and a query can carry
                // PII (a search term, a name, a phone). `MatchedPath` gives the
                // stable template label the OTel/LGTM backend can safely index.
                http_route = %cardinality_safe_http_route(request),
                version = ?request.version(),
                trace_id = tracing::field::Empty,
                span_id = tracing::field::Empty,
            )
        })
        .on_request(|request: &Request<Body>, span: &tracing::Span| {
            let trace_id = record_otel_ids(span);
            tracing::info!(
                trace_id = %trace_id,
                method = %request.method(),
                // Matched route template only; see make_span_with.
                http_route = %cardinality_safe_http_route(request),
                "http request started"
            );
        })
        .on_response(
            |response: &Response<Body>, latency: Duration, span: &tracing::Span| {
                let trace_id = record_otel_ids(span);
                tracing::info!(
                    trace_id = %trace_id,
                    status = response.status().as_u16(),
                    latency_ms = latency.as_millis(),
                    "http request completed"
                );
            },
        )
}

/// Build the engine-backed governed-config catalog seeder injected into the
/// platform router. Runs once per newly onboarded tenant: it opens a
/// configured `PgOntologyStore` and drives the standard config object types
/// (SLO settings, console views) through the engine, scoped to the new org so the
/// registry writes pass FORCE-RLS. Lives here (App tier) because the platform tier
/// must not depend on the ontology adapter (layer boundary).
fn tenant_config_seeder(store: PgOntologyStore) -> console_platform_rest::TenantConfigSeeder {
    Arc::new(move |org, actor, at| {
        let store = store.clone();
        Box::pin(async move {
            console_platform_request_context::scope_org(
                org,
                console_ontology_adapter_postgres::seed::seed_governed_config_object_types(
                    &store, actor, at,
                ),
            )
            .await
            .map(|_| ())
            .map_err(|err| err.to_string())
        })
    })
}

pub fn build_router(state: AppState) -> Router {
    // The base router carries NO cross-cutting layers here. Per axum's `merge`
    // semantics, any layer applied to a router *before* it is merged with the
    // domain routers wraps only the base routes, not the merged-in ones â which
    // is exactly the bug this composition avoids. The trace layer, timeout, and
    // body limit are applied to the FULLY-merged router below so every route
    // (base + domains) is covered, with the realtime route deliberately merged
    // outside the timeout so a long-lived WS connection is never severed.
    let router = Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/openapi/openapi.yaml", get(openapi_yaml))
        // Public, unauthenticated native app-link association documents. Native
        // passkeys are inert until the platform serves these at the EXACT
        // dotted paths over the RP origin, so they live on the base router with
        // no auth and no tenant middleware (they carry no tenant data).
        .route(WELL_KNOWN_AASA_PATH, get(apple_app_site_association))
        .route(WELL_KNOWN_ASSETLINKS_PATH, get(android_assetlinks))
        .with_state(state.clone());

    let router = match &state.database {
        DatabaseDependency::Postgres(pool) => {
            let kpi_repository = PgKpiRepository::new(pool.clone());
            let realtime_hub = state
                .realtime_hub
                .clone()
                .unwrap_or_else(|| Arc::new(PgRealtimeHub::new(pool.clone(), Default::default())));
            let notification_store = PgNotificationStore::new(pool.clone())
                .with_notifier(Arc::new(PostgresNotificationNotifier::new(pool.clone())));
            let messenger_store = PgMessengerStore::new(pool.clone())
                .with_notifier(Arc::new(PostgresMessageNotifier::new(pool.clone())))
                // `@`-mentions create notification-center rows via the #198 sink
                // (fan-out enabled) so a mentioned member sees them in their inbox.
                .with_notification_sink(Arc::new(notification_store.clone()));
            let todo_store = PgTodoStore::new(pool.clone());
            let registry_store = PgRegistryStore::new(pool.clone());
            let financial_store = PgFinancialStore::new(pool.clone());
            let evaluation_store = PgEvaluationStore::new(pool.clone());
            let inspection_store = PgInspectionStore::new(pool.clone());
            let compliance_store = PgComplianceStore::new(pool.clone());
            let integrity_store = PgIntegrityStore::new(pool.clone());
            let dispatch_store = PgDispatchStore::new(pool.clone());
            let support_store = PgSupportStore::new(pool.clone());
            let sales_store = PgSalesStore::new(pool.clone());
            let org_store = PgOrgStore::new(pool.clone());
            // Ontology / governance / policy-studio engine stores (each rest
            // router self-arms `app.current_org`, like every domain router).
            let ontology_registry_store = match &state.ontology_command_database {
                DatabaseDependency::Postgres(command_pool) => {
                    PgOntologyStore::new(pool.clone()).with_command_pool(command_pool.clone())
                }
                DatabaseDependency::NotConfigured => PgOntologyStore::new(pool.clone()),
            };
            let platform_tenant_config_seeder = match &state.ontology_command_database {
                DatabaseDependency::Postgres(_) => {
                    Some(tenant_config_seeder(ontology_registry_store.clone()))
                }
                DatabaseDependency::NotConfigured => None,
            };
            let ontology_instance_store = PgInstanceStore::new(pool.clone());
            let governance_store = PgGovernanceStore::new(pool.clone());
            let cedar_policy_store = PgCedarPolicyStore::new(pool.clone());
            let work_order_store = PgWorkOrderStore::new(pool.clone())
                .with_created_listener(Arc::new(messenger_store.clone()));
            let benefit_store = PgBenefitCatalogStore::new(pool.clone());
            let logistics_store = PgLogisticsStore::new(pool.clone());
            let leave_store = {
                let store = console_leave_adapter_postgres::PgLeaveStore::new(
                    pool.clone(),
                    Arc::new(PgInboxStore::new(pool.clone())),
                );
                match &state.leave_command_database {
                    DatabaseDependency::Postgres(command_pool) => {
                        store.with_leave_command_pool(command_pool.clone())
                    }
                    DatabaseDependency::NotConfigured => store,
                }
            };
            // Authenticated domain routers (tenant-scoped data). Each domain
            // `router()` self-applies the per-request org middleware (so the
            // behavior is testable per crate), arming `app.current_org` for every
            // route. `/api/audit` is an app-level route, so it gets the same
            // middleware applied directly here. L20 audit-chain PR-2: the
            // read-only attestation endpoint joins the SAME router (no new
            // `.merge()`), the established pattern for app-level audit REST.
            let audit_router = console_platform_request_context::with_request_context(
                Router::new()
                    .route(AUDIT_ROUTE_PATH, get(audit_log))
                    .route(AUDIT_ATTESTATION_ROUTE_PATH, get(audit_attestation))
                    .with_state(state.clone()),
                state.jwt_verifier.clone(),
                pool.clone(),
            );
            let domain_router = audit_router
                .merge(console_telemetry::router(
                    console_telemetry::ConsoleTelemetryState::new(
                        pool.clone(),
                        state.jwt_verifier.clone(),
                    ),
                ))
                .merge(console_dispatch_rest::router(DispatchRestState::new(
                    dispatch_store,
                    state.jwt_verifier.clone(),
                    state.config.dispatch_timers,
                    state.dispatch_job_queue.clone(),
                    state.push_notifier.clone(),
                )))
                .merge(console_logistics_rest::router(LogisticsRestState::new(
                    logistics_store,
                    state.jwt_verifier.clone(),
                )))
                .merge(console_attendance_rest::router(AttendanceRestState::new(
                    PgAttendanceStore::new(pool.clone()),
                    state.jwt_verifier.clone(),
                )))
                .merge(console_inventory_rest::router(InventoryRestState::new(
                    PgInventoryStore::new(pool.clone()),
                    state.jwt_verifier.clone(),
                )))
                .merge(console_equipment_rest::router(EquipmentRestState::new(
                    PgEquipment3rStore::new(pool.clone()),
                    state.jwt_verifier.clone(),
                )))
                .merge(console_financial_rest::router(
                    FinancialRestState::new(financial_store, state.jwt_verifier.clone())
                        .with_passkey_step_up(state.policy_step_up.clone())
                        .with_purchase_attachment_storage(state.sales_media_storage.clone()),
                ))
                .merge(console_inspection_rest::router(InspectionRestState::new(
                    inspection_store,
                    state.jwt_verifier.clone(),
                )))
                .merge(console_support_rest::router({
                    let mut support_state = SupportRestState::new(
                        support_store,
                        state.jwt_verifier.clone(),
                        state.push_notifier.clone(),
                    );
                    if let Some(storefront_org) = state.config.storefront_org {
                        support_state = support_state.with_storefront_org(storefront_org);
                    }
                    support_state
                }))
                .merge(console_identity_rest::router(
                    IdentityRestState::new(org_store, state.jwt_verifier.clone())
                        .with_passkey_step_up(state.policy_step_up.clone()),
                ))
                .merge(console_compliance_rest::router(ComplianceRestState::new(
                    compliance_store,
                    state.jwt_verifier.clone(),
                )))
                .merge(console_integrity::router(IntegrityRestState::new(
                    integrity_store,
                    state.jwt_verifier.clone(),
                )))
                .merge(console_registry_rest::router(
                    RegistryRestState::new(registry_store, state.jwt_verifier.clone())
                        .with_passkey_step_up(state.policy_step_up.clone()),
                ))
                .merge(hr::router({
                    let hr_state = hr::HrState::new(pool.clone(), state.jwt_verifier.clone());
                    hr_state.with_leave_command_store(leave_store.clone())
                }))
                .merge(console_recruiting_rest::router(RecruitingRestState::new(
                    PgRecruitingStore::new(pool.clone()),
                    state.jwt_verifier.clone(),
                )))
                .merge(console_evaluation_rest::router(EvaluationRestState::new(
                    evaluation_store,
                    state.jwt_verifier.clone(),
                )))
                // The hire handshake stays app-level: it shares one transaction
                // with the HR-owned employee-creation core (see recruiting_hire).
                .merge(recruiting_hire::router(
                    recruiting_hire::RecruitingHireState::new(
                        pool.clone(),
                        state.jwt_verifier.clone(),
                        hr::HrState::new(pool.clone(), state.jwt_verifier.clone())
                            .with_leave_command_store(leave_store.clone()),
                    ),
                ))
                .merge(workflow_studio::router(
                    workflow_studio::WorkflowStudioState::new(
                        pool.clone(),
                        state.jwt_verifier.clone(),
                    )
                    .with_passkey_step_up(state.policy_step_up.clone()),
                ))
                .merge(workflow_object_context::router(
                    workflow_object_context::WorkflowObjectContextState::new(
                        pool.clone(),
                        state.jwt_verifier.clone(),
                    ),
                ))
                .merge(collaboration::router(
                    collaboration::CollaborationState::new(
                        pool.clone(),
                        state.jwt_verifier.clone(),
                    )
                    .with_passkey_step_up(state.policy_step_up.clone()),
                ))
                .merge(action_inbox::router(action_inbox::ActionInboxState::new(
                    pool.clone(),
                    state.jwt_verifier.clone(),
                )))
                .merge(workbench::router(workbench::WorkbenchState::new(
                    Arc::new(workbench_native::NativeWorkbenchReaders::new(pool.clone())),
                    pool.clone(),
                    state.jwt_verifier.clone(),
                )))
                .merge(objects::router(objects::ObjectState::new(
                    pool.clone(),
                    state.jwt_verifier.clone(),
                )))
                .merge(lifecycle::router(lifecycle::LifecycleState::new(
                    pool.clone(),
                    state.jwt_verifier.clone(),
                )))
                .merge(office::router({
                    // Reuse the already-configured SeaweedFS handle (evidence /
                    // sales media) as the office blob store — no new object-store
                    // client. `None` blobs ⇒ office endpoints 503, like mail.
                    let office_blobs = match (&state.config.office, &state.sales_media_storage) {
                        (Some(cfg), Some((store, bucket))) => {
                            office::OfficeState::seaweed_blobs(cfg, store.clone(), bucket.clone())
                        }
                        _ => None,
                    };
                    office::OfficeState::new(
                        pool.clone(),
                        state.jwt_verifier.clone(),
                        state.config.office.clone(),
                        office_blobs,
                    )
                }))
                .merge(console_sales_rest::router({
                    let mut sales_state =
                        SalesRestState::new(sales_store, state.jwt_verifier.clone());
                    if let Some(storefront_org) = state.config.storefront_org {
                        sales_state = sales_state.with_storefront_org(storefront_org);
                    }
                    if let Some((object_store, bucket)) = state.sales_media_storage.clone() {
                        sales_state = sales_state.with_media_storage(object_store, bucket);
                    }
                    sales_state
                }))
                .merge(console_reporting_rest::router(KpiRestState::new(
                    kpi_repository,
                    state.jwt_verifier.clone(),
                )))
                .merge(console_workorder_rest::router(
                    WorkOrderRestState::new(work_order_store.clone(), state.jwt_verifier.clone())
                        .with_workflow_runtime(Some(
                            console_workflow_runtime_adapter_postgres::PgWorkflowRuntimeStore::new(
                                pool.clone(),
                            ),
                        )),
                ))
                .merge(console_workorder_rest::mobile_router(
                    MobileRestState::new(
                        pool.clone(),
                        work_order_store,
                        state.jwt_verifier.clone(),
                        state.evidence_storage.clone(),
                    )
                    .with_passkey_step_up(state.policy_step_up.clone())
                    .with_workflow_runtime(Some(
                        console_workflow_runtime_adapter_postgres::PgWorkflowRuntimeStore::new(
                            pool.clone(),
                        ),
                    ))
                    .with_job_queue(state.dispatch_job_queue.clone()),
                ))
                .merge(console_facilities_rest::router(FacilitiesRestState::new(
                    pool.clone(),
                    state.jwt_verifier.clone(),
                )))
                .merge(console_production_rest::router(
                    ProductionRestState::new(pool.clone(), state.jwt_verifier.clone())
                        .with_service_principal_hmac_key(
                            state.config.production_service_principal_hmac_key,
                        ),
                ))
                .merge(console_messenger_rest::router(MessengerRestState::new(
                    messenger_store,
                    state.jwt_verifier.clone(),
                )))
                // Evidence-objects console surface (custody / fixity-verify /
                // legal-hold). The fixity check HEADs the WORM (replica) bucket;
                // when object storage is unconfigured the store is `None` and
                // `verify` 503s rather than green-lighting an unverifiable object.
                .merge(console_docs_rest::router(DocsRestState::new(
                    PgDocsStore::new(pool.clone()),
                    governance_store.clone(),
                    state
                        .worm_evidence_storage
                        .as_ref()
                        .map(|(store, _)| Arc::new(store.clone()) as Arc<dyn S3ObjectStore>),
                    state
                        .worm_evidence_storage
                        .as_ref()
                        .map(|(_, bucket)| bucket.clone())
                        .unwrap_or_default(),
                    state.jwt_verifier.clone(),
                )))
                .merge(console_notifications_rest::router(
                    NotificationRestState::new(
                        notification_store.clone(),
                        state.jwt_verifier.clone(),
                    ),
                ))
                .merge(console_inbox_rest::router(
                    InboxRestState::new(
                        PgInboxStore::new(pool.clone()),
                        state.jwt_verifier.clone(),
                    )
                    .with_passkey_step_up(state.policy_step_up.clone()),
                ))
                // Leave-request queue + §61 statutory push. The push delivers a
                // receipt-gated notice through the SAME inbox vault (a fresh
                // `PgInboxStore` over the shared pool as the `InboxDocSink`).
                .merge(console_leave_rest::router(
                    console_leave_rest::LeaveRestState::new(
                        leave_store,
                        state.jwt_verifier.clone(),
                    ),
                ))
                .merge(console_benefit_rest::router(BenefitRestState::new(
                    benefit_store,
                    state.jwt_verifier.clone(),
                )))
                .merge(console_consulting_rest::router(ConsultingRestState::new(
                    pool.clone(),
                    state.jwt_verifier.clone(),
                )))
                .merge(console_todos_rest::router(TodoRestState::new(
                    todo_store,
                    state.jwt_verifier.clone(),
                )))
                // Webmail (`/api/v1/mail/*`). The router ALWAYS mounts so the
                // OpenAPI paths exist and the app boots without the master key;
                // when `state.mail_cipher` is `None`, read paths degrade cleanly
                // while credential-using endpoints return 503.
                // The inbound-attachment object store (presigned GET) is wired
                // from the same storage config the evidence pipeline uses; `None`
                // when storage is unconfigured (download then 503s).
                // mox integration (slice 1): when `CONSOLE_MAIL_MOX_BASE_URL` is set,
                // the outbound transport rides our own mox server's webapi instead
                // of the lettre SMTP client; `CONSOLE_MAIL_MOX_WEBHOOK_SECRET` arms the
                // inbound delivery webhook (absent → webhook 503s, never hardcoded).
                .merge(console_comms_rest::router(
                    CommsRestState::new(
                        PgMailStore::new(pool.clone()),
                        state.mail_cipher.clone(),
                        state.jwt_verifier.clone(),
                    )
                    .with_attachments(mail_attachment_store(&state))
                    .with_mox_transport(state.config.mail_mox_base_url.clone())
                    .with_mox_webhook_secret(state.config.mail_mox_webhook_secret.clone()),
                ))
                // Ontology / governance / Policy-Studio engine surfaces.
                // `ontology` self-applies four-eyes/governance gate chains; its
                // action-execute path is the single mutation surface (§16).
                .merge(console_ontology_rest::router(
                    OntologyRestState::new(
                        ontology_registry_store.clone(),
                        ontology_instance_store,
                        governance_store.clone(),
                        state.jwt_verifier.clone(),
                    )
                    .with_projected_dispatch(projected_dispatch_registry(pool.clone())),
                ))
                .merge(console_governance_rest::router(GovernanceRestState::new(
                    governance_store,
                    state.jwt_verifier.clone(),
                )))
                // Org-change lifecycle engine (조직 개편 결재): draft → preflight
                // → ordered SoD approval → effective-dated apply (§15/§16).
                .merge(console_orgchange_rest::router(OrgChangeRestState::new(
                    PgOrgChangeStore::new(pool.clone())
                        .with_employment_transfer(std::sync::Arc::new(PgEmploymentTransferPort)),
                    state.jwt_verifier.clone(),
                )))
                .merge(console_platform_authz_rest::router(
                    CedarPolicyRestState::new(cedar_policy_store, state.jwt_verifier.clone()),
                ))
                // Notice board (사내 게시판): publish snapshots recipients into
                // `notice_receipts` and fans out one notification per recipient
                // through the SAME notification-center sink the messenger
                // @-mention path uses (#198 pattern).
                .merge(console_notices_rest::router(NoticeRestState::new(
                    PgNoticeStore::new(pool.clone())
                        .with_notification_sink(Arc::new(notification_store.clone())),
                    state.jwt_verifier.clone(),
                )))
                // Accounting GL vouchers (전표): create/submit/approve/post/reverse.
                .merge(console_finance_gl_rest::router(FinanceGlRestState::new(
                    PgVoucherStore::new(pool.clone()),
                    state.jwt_verifier.clone(),
                )))
                // Payroll draft-run visibility (admin org-wide + self payslips).
                .merge(console_payroll_rest::router(PayrollRestState::new(
                    PgPayrollStore::new(pool.clone()),
                    state.jwt_verifier.clone(),
                )))
                // Deterministic statistical projection (read-only, stateless).
                .merge(console_analytics_quant_rest::router(
                    AnalyticsQuantState::new(pool.clone(), state.jwt_verifier.clone()),
                ));
            // READ-ONLY WALL for PLATFORM "view as": wrap the WHOLE tenant
            // domain router so any request carrying a `view_as` token may use
            // ONLY GET/HEAD — every other method is rejected 403 `view_as_read_only`
            // BEFORE any handler or per-handler authz runs. This is a blanket
            // method gate keyed purely on the token's `view_as` claim, so no
            // mutation handler on any tenant route is reachable while
            // impersonating, regardless of the acting role. It is orthogonal to
            // (and does not replace) the tenant org middleware each domain router
            // already applies; an ordinary tenant/platform token is untouched.
            let domain_router = console_platform_rest::with_view_as_read_only_gate(
                domain_router,
                state.jwt_verifier.clone(),
            );
            // The realtime WS upgrade and the pre-auth login/refresh endpoints are
            // intentionally NOT under the org middleware: a login request has no
            // tenant yet, and the WS handler runs its own auth over the socket
            // lifetime (a task-local would not survive the upgrade anyway).
            // PLATFORM tier (`/api/platform/*`). Mounted at the APP level (merged,
            // not nested) behind the PLATFORM extractor â deliberately NOT under
            // the tenant org middleware: the PLATFORM extractor rejects a tenant
            // token here (403), and the per-router tenant org middleware rejects a
            // platform token on the tenant `/api/v1/*` routes (403). There is NO
            // blanket `/api/*` platform-token rejection, so `/api/platform/*` is
            // reached untouched by tenant middleware. Living under `/api` lets the
            // ingress `/api`→backend rule route it while the SPA keeps the bare
            // browser routes `/platform/*`. This is the only path that creates org
            // rows.
            let platform_router = console_platform_rest::router(
                PlatformRestState::new(
                    pool.clone(),
                    state.jwt_verifier.clone(),
                    PlatformProvisioner::new(state.config.coldstart_otp_ttl),
                )
                .with_view_as_issuer(state.view_as_issuer.clone())
                .with_force_remove_command_pool(match &state.platform_force_command_database {
                    DatabaseDependency::Postgres(pool) => Some(pool.clone()),
                    DatabaseDependency::NotConfigured => None,
                })
                .with_tenant_config_seeder(platform_tenant_config_seeder),
            );
            // Everything EXCEPT the realtime WS upgrade: base health/openapi
            // routes, the tenant domain routers, the platform tier, and the
            // pre-auth login/refresh endpoints. These are all short-lived
            // request/response cycles, so they carry the 30s request timeout.
            let timed = {
                let timed = router.merge(domain_router).merge(platform_router);
                let timed = match state.auth_rest.clone() {
                    Some(auth_rest) => {
                        // The auth-rest router carries authenticated tenant
                        // mutations (issue-OTP, credential-reset, enroll-handoff,
                        // passkey register/finish). They live OUTSIDE the tenant
                        // domain router, so the view-as read-only wall must also
                        // wrap them — otherwise an impersonation token could reach
                        // a write here. The pre-auth POSTs (login/refresh/redeem)
                        // carry no view_as token, so the gate is a no-op for them.
                        let auth_router = console_platform_rest::with_view_as_read_only_gate(
                            console_platform_auth_rest::router(auth_rest),
                            state.jwt_verifier.clone(),
                        );
                        timed.merge(auth_router)
                    }
                    None => timed,
                };
                // Defense-in-depth: shed any request that hangs on a slow
                // upstream or DB so a stuck handler cannot pin a worker. Applied
                // BEFORE the realtime router is merged so the long-lived WS
                // connection below is never severed by this 30s budget â this is
                // the #1 regression to prevent (live wallboard/dispatch SSE/WS).
                timed.layer(TimeoutLayer::with_status_code(
                    StatusCode::REQUEST_TIMEOUT,
                    state.config.request_timeout,
                ))
            };
            // The realtime WS upgrade is merged AFTER the timeout layer so it
            // escapes the 30s budget. It is intentionally NOT under the org
            // middleware: the WS handler runs its own auth over the socket
            // lifetime (a task-local would not survive the upgrade anyway).
            timed.merge(console_platform_realtime::router(RealtimeRestState::new(
                realtime_hub,
                state.jwt_verifier.clone(),
            )))
        }
        DatabaseDependency::NotConfigured => router,
    };
    let router = router.nest(
        "/_ui",
        Router::new()
            .route("/", get(ui_shell))
            .route("/organization", get(ui_organization))
            .route("/hr", get(ui_hr))
            .route("/payroll", get(ui_payroll))
            .merge(console_payroll_ui::pkg_router())
            .with_state(state.clone()),
    );
    // Cross-cutting layers on the FULLY-merged router (base + every domain +
    // platform + realtime + auth), so they actually cover the merged routes:
    //   * DefaultBodyLimit (2 MiB) â bounds every request body. Applied here
    //     (innermost of these), so a per-route `DefaultBodyLimit::max(N)` set
    //     deeper in a domain router (e.g. the 16 MiB equipment import) still
    //     wins. The realtime WS upgrade carries no body, so this is a no-op for
    //     it. Overridable per-route, unlike an outermost RequestBodyLimitLayer.
    //   * TraceLayer â emits a request span for EVERY route, realtime included
    //     (tracing a long-lived WS only logs its start/end, so it is safe).
    let router = router
        .layer(axum::extract::DefaultBodyLimit::max(MAX_REQUEST_BODY_BYTES))
        .layer(http_trace_layer());
    // This wraps the fully-composed HTTP surface exactly once.  Every router
    // below consumes the resolved extension; no domain/rate-limit surface is
    // allowed to interpret raw X-Forwarded-For itself.
    let router = console_platform_request_context::with_trusted_client_ip(
        router,
        state.config.trusted_proxy_count,
        state.config.trusted_proxy_cidrs.clone(),
    );
    with_metrics(router, &state)
}

async fn ui_shell(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    ui_screens(state, headers, console_payroll_ui::UiScreen::Home).await
}

async fn ui_organization(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    ui_screens(state, headers, console_payroll_ui::UiScreen::Organization).await
}

async fn ui_hr(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    ui_screens(state, headers, console_payroll_ui::UiScreen::Hr).await
}

async fn ui_payroll(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    ui_screens(state, headers, console_payroll_ui::UiScreen::Payroll).await
}

async fn ui_screens(
    state: AppState,
    headers: HeaderMap,
    focus: console_payroll_ui::UiScreen,
) -> impl IntoResponse {
    console_payroll_ui::html_shell_with_screens(&compose_ui_screens(&state, &headers).await, focus)
}

async fn compose_ui_screens(
    state: &AppState,
    headers: &HeaderMap,
) -> console_payroll_ui::ShippingScreens {
    let (org_entities, people, runs, floors) = match (&state.database, &state.jwt_verifier) {
        (DatabaseDependency::Postgres(pool), Some(verifier)) => {
            let floors = ui_listing_floors(verifier, pool, headers).await;
            let org = OrgChangeRestState::new(
                PgOrgChangeStore::new(pool.clone()),
                Some(verifier.clone()),
            )
            .visible_org_entities(headers)
            .await;
            let people =
                IdentityRestState::new(PgOrgStore::new(pool.clone()), Some(verifier.clone()))
                    .visible_directory_people(headers)
                    .await;
            let runs =
                PayrollRestState::new(PgPayrollStore::new(pool.clone()), Some(verifier.clone()))
                    .visible_run_summaries(headers)
                    .await;
            (org, people, runs, floors)
        }
        _ => (
            Vec::new(),
            Vec::new(),
            console_payroll_rest::PayrollRunListing::Omitted,
            UiListingFloors::denied(),
        ),
    };
    console_payroll_ui::ShippingScreens {
        org_entities: console_payroll_ui::ScreenSection::from_authorized_listing(
            org_entities.iter().map(ui_org_entity).collect(),
            floors.org,
        ),
        people: console_payroll_ui::ScreenSection::from_authorized_listing(
            people.iter().map(ui_person).collect(),
            floors.hr,
        ),
        runs: match runs {
            console_payroll_rest::PayrollRunListing::Omitted => {
                console_payroll_ui::ScreenSection::Omitted
            }
            console_payroll_rest::PayrollRunListing::Loaded(page) => {
                console_payroll_ui::ScreenSection::from_authorized_listing(
                    page.items.iter().map(ui_run_summary).collect(),
                    true,
                )
            }
            console_payroll_rest::PayrollRunListing::FailedAfterAuthorization => {
                console_payroll_ui::ScreenSection::Failure
            }
        },
    }
}

struct UiListingFloors {
    org: bool,
    hr: bool,
}

impl UiListingFloors {
    const fn denied() -> Self {
        Self {
            org: false,
            hr: false,
        }
    }
}

/// Same listing floors as the existing GETs those screens already use.
/// Organization/HR helpers still collapse errors to `[]`; these floors only
/// distinguish authorized empty from omission for those two listings.
/// Payroll carries its own authority and failure outcome from its read owner.
async fn ui_listing_floors(
    verifier: &JwtVerifier,
    pool: &PgPool,
    headers: &HeaderMap,
) -> UiListingFloors {
    let principal =
        match console_platform_request_context::resolve_principal(verifier, pool, headers).await {
            Ok(principal) => principal,
            Err(_) => return UiListingFloors::denied(),
        };
    UiListingFloors {
        org: principal
            .roles
            .iter()
            .any(|role| matches!(role, Role::Admin | Role::Executive | Role::SuperAdmin)),
        hr: ui_directory_listing_ok(&principal),
    }
}

fn ui_directory_listing_ok(principal: &Principal) -> bool {
    let mut effective = BranchScope::Branches(BTreeSet::new());
    let role_allows = principal.roles.iter().any(|role| {
        permission_for(*role, Feature::EmployeeDirectoryRead) == PermissionLevel::Allow
    });
    if role_allows {
        effective = ui_branch_scope_union(&effective, &principal.branch_scope);
    }
    for grant in principal.effective_feature_grants.iter().filter(|grant| {
        grant.feature == Feature::EmployeeDirectoryRead
            && grant.permission == PermissionLevel::Allow
    }) {
        let grant_scope = principal.branch_scope.intersect(&grant.branch_scope);
        effective = ui_branch_scope_union(&effective, &grant_scope);
    }
    match &effective {
        BranchScope::All => true,
        BranchScope::Branches(branches) => !branches.is_empty(),
    }
}

fn ui_branch_scope_union(a: &BranchScope, b: &BranchScope) -> BranchScope {
    match (a, b) {
        (BranchScope::All, _) | (_, BranchScope::All) => BranchScope::All,
        (BranchScope::Branches(left), BranchScope::Branches(right)) => {
            BranchScope::Branches(left.union(right).copied().collect())
        }
    }
}

fn ui_org_entity(
    entity: &console_orgchange_rest::VisibleOrgEntity,
) -> console_payroll_ui::OrgEntityView {
    console_payroll_ui::OrgEntityView {
        org_id: entity.org_id.clone(),
        slug: entity.slug.clone(),
        name: entity.name.clone(),
        status: entity.status.clone(),
    }
}

fn ui_person(
    person: &console_identity_rest::VisibleDirectoryPerson,
) -> console_payroll_ui::PersonView {
    console_payroll_ui::PersonView {
        id: person.id.clone(),
        display_name: person.display_name.clone(),
        employee_id: person.employee_id.clone(),
        employee_name: person.employee_name.clone(),
        employee_number: person.employee_number.clone(),
        employee_company: person.employee_company.clone(),
        employee_org_unit: person.employee_org_unit.clone(),
        employee_position: person.employee_position.clone(),
        employee_identity_review_required: person.employee_identity_review_required.clone(),
        employee_identity_resolution_confidence: person
            .employee_identity_resolution_confidence
            .clone(),
        employee_link_status: person.employee_link_status.clone(),
        team: person.team.clone(),
        roles: person.roles.clone(),
        branch_ids: person.branch_ids.clone(),
        is_active: person.is_active.clone(),
        has_passkey: person.has_passkey.clone(),
        account_status: person.account_status.clone(),
        created_at: person.created_at.clone(),
    }
}

fn ui_run_summary(
    run: &console_payroll_adapter_postgres::PayrollRunSummary,
) -> console_payroll_ui::RunSummary {
    use time::format_description::well_known::Rfc3339;
    console_payroll_ui::RunSummary {
        id: run.id.to_string(),
        period_start: run.period_start.to_string(),
        period_end: run.period_end.to_string(),
        source_label: run.source_label.clone(),
        status: run.status.clone(),
        calculation_enabled: run.calculation_enabled,
        created_at: run
            .created_at
            .format(&Rfc3339)
            .unwrap_or_else(|_| String::new()),
        updated_at: run
            .updated_at
            .format(&Rfc3339)
            .unwrap_or_else(|_| String::new()),
    }
}

async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
    Json(HealthBody {
        status: "ok",
        service: state.config.service_name,
        role: state.config.role,
    })
}

async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    let database = readiness_dependency_status(&state.database, "runtime").await;
    let command_databases_required = state.config.role == AppRole::Api
        && matches!(state.database, DatabaseDependency::Postgres(_));
    let leave_command_database =
        readiness_dependency_status(&state.leave_command_database, "leave_command").await;
    let ontology_command_database =
        readiness_dependency_status(&state.ontology_command_database, "ontology_command").await;
    let platform_force_command_database = readiness_dependency_status(
        &state.platform_force_command_database,
        "platform_force_command",
    )
    .await;

    let ready = database.healthy()
        && (!command_databases_required
            || (leave_command_database.configured
                && leave_command_database.ready
                && ontology_command_database.configured
                && ontology_command_database.ready
                && platform_force_command_database.configured
                && platform_force_command_database.ready));
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        Json(ReadyBody {
            status: if ready { "ready" } else { "not_ready" },
            // True requires separate fenced writer authority and operation-bound two-site proof.
            write_ready: false,
            service: state.config.service_name,
            role: state.config.role,
            database: database.label,
            leave_command_database: leave_command_database.label,
            ontology_command_database: ontology_command_database.label,
            platform_force_command_database: platform_force_command_database.label,
        }),
    )
}

#[derive(Debug, Clone, Copy)]
struct ReadinessDependencyStatus {
    configured: bool,
    ready: bool,
    label: &'static str,
}

impl ReadinessDependencyStatus {
    fn healthy(self) -> bool {
        !self.configured || self.ready
    }
}

async fn readiness_dependency_status(
    dependency: &DatabaseDependency,
    dependency_name: &'static str,
) -> ReadinessDependencyStatus {
    match dependency {
        DatabaseDependency::NotConfigured => ReadinessDependencyStatus {
            configured: false,
            ready: false,
            label: "not_configured",
        },
        DatabaseDependency::Postgres(pool) => {
            let ready = sqlx::query("SELECT 1")
                // rls-arming: ok readiness health probe is SELECT 1 (non-tenant, non-RLS)
                .execute(pool)
                .instrument(tracing::info_span!(
                    "db.readiness",
                    db_system = "postgresql",
                    db_operation = "SELECT",
                    db_statement = "SELECT 1",
                    dependency = dependency_name,
                ))
                .await
                .is_ok();
            ReadinessDependencyStatus {
                configured: true,
                ready,
                label: if ready { "ready" } else { "unreachable" },
            }
        }
    }
}

async fn openapi_yaml() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/yaml; charset=utf-8")],
        OPENAPI_YAML,
    )
}

/// Serve the Apple App Site Association document at the well-known path.
///
/// Public + unauthenticated, served as `application/json` (no extension, per
/// Apple's requirement). The `webcredentials.apps` list is sourced from
/// `CONSOLE_IOS_APP_IDS`; an empty list yields a valid but inert document. Apple
/// fetches this over the RP origin to authorize the native app's passkeys.
async fn apple_app_site_association(State(state): State<AppState>) -> Response<Body> {
    let body = apple_app_site_association_json(AppleAppSiteAssociationConfig {
        app_ids: state.config.app_links.ios_app_ids.clone(),
    });
    well_known_json_response(body)
}

/// Serve the Android Digital Asset Links document at `/.well-known/assetlinks.json`.
///
/// Public + unauthenticated, served as `application/json`. The package +
/// signing-cert fingerprints come from `CONSOLE_ANDROID_PACKAGE` /
/// `CONSOLE_ANDROID_CERT_SHA256`; when the package is unset the document is an empty
/// JSON array (valid + inert). Android fetches this to authorize the app's
/// passkeys for the RP domain.
async fn android_assetlinks(State(state): State<AppState>) -> Response<Body> {
    let links = &state.config.app_links;
    let body = match &links.android_package {
        Some(package_name) => android_assetlinks_json(AndroidAssetLinksConfig {
            package_name: package_name.clone(),
            sha256_cert_fingerprints: links.android_cert_sha256.clone(),
        }),
        // No package configured yet: serve an empty (but valid) asset-links array
        // rather than a half-populated entry with no package name.
        None => Ok("[]".to_owned()),
    };
    well_known_json_response(body)
}

/// Build an `application/json` response for a well-known association document.
///
/// Serialization of these tiny, statically-shaped documents cannot realistically
/// fail; if it ever did we surface a 500 rather than serving a malformed body.
fn well_known_json_response(
    body: Result<String, console_platform_auth::AuthError>,
) -> Response<Body> {
    match body {
        Ok(json) => ([(header::CONTENT_TYPE, "application/json")], json).into_response(),
        Err(err) => {
            tracing::error!(error = %err, "failed to serialize well-known association document");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn audit_log(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Query(query): Query<AuditQuery>,
) -> Result<Json<AuditPage>, ApiError> {
    let pool = match &state.database {
        DatabaseDependency::Postgres(pool) => pool,
        DatabaseDependency::NotConfigured => {
            return Err(ApiError::service_unavailable(
                "database is not configured for audit access",
            ));
        }
    };
    authorize_audit_read(&principal)?;
    let query = normalize_audit_query(query)?;
    let audit_event = audit_read_event(&principal)?;
    let branch_scope = principal.branch_scope.clone();
    let limit = query.limit;
    let offset = query.offset;

    let items = with_audit::<_, Vec<AuditRecord>, AppError>(pool, audit_event, |tx| {
        Box::pin(async move {
            fetch_audit_records(tx.as_mut(), branch_scope, query)
                .await
                .map_err(AppError::Database)
        })
    })
    .await
    .map_err(ApiError::from_app)?;

    Ok(Json(AuditPage {
        items,
        limit,
        offset,
    }))
}

/// L20 audit-chain PR-2: read-only attestation for the caller's tenant.
/// Recomputes and compares the org's sealed hash chain (`verify_org_chain`,
/// charter §5.3) and returns the verdict — never mutates. Unlike `/api/audit`,
/// which can safely branch-filter rows for a branch-scoped ADMIN, this endpoint
/// verifies the whole tenant chain. Require org-wide `AuditLogRead` so the
/// attestation surface cannot widen branch-scoped audit visibility. For
/// `AuditLogRead` today, built-in org-wide authority is SUPER_ADMIN; a
/// branch-scoped or branch-omitted ADMIN token must not pass this gate.
///
/// Cost note: `verify_org_chain` re-derives every seal's batch from its full
/// `audit_events` range — a FULL-CHAIN re-verify, not an incremental one, so
/// wall time scales with the org's total sealed audit history. Bounded today
/// by (a) org-wide built-in callers (SUPER_ADMIN for `AuditLogRead`), so
/// callers are trusted and infrequent, (b) the app-wide request timeout, and
/// (c) console_rt's 30s `statement_timeout` (migration 0112) capping any single DB
/// pass. A head-N-seals or cached-verdict endpoint variant for orgs with a long
/// history is a PR-3 item (charter F1 anchor), not built here.
async fn audit_attestation(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<ChainReport>, ApiError> {
    let pool = match &state.database {
        DatabaseDependency::Postgres(pool) => pool,
        DatabaseDependency::NotConfigured => {
            return Err(ApiError::service_unavailable(
                "database is not configured for audit access",
            ));
        }
    };
    authorize_audit_attestation(&principal)?;

    // Shared throwaway signer is correct here: `verify` reconstructs the
    // public key from each seal's OWN stored `key_ref`; this object is not the
    // trust root, just the implementation that performs verification. The
    // context-selected external signer/key-custody adapter will verify the same
    // way (key material keyed off the stored `key_ref`).
    let signer = state.audit_attestation_signer.clone();

    let report = verify_org_chain(
        pool,
        principal.org_id,
        &signer,
        time::OffsetDateTime::now_utc(),
        &SealConfig::default(),
    )
    .await
    .map_err(|err| {
        tracing::error!(error = %err, "audit-chain attestation verify failed");
        ApiError::internal("attestation verification failed")
    })?;

    Ok(Json(report))
}

/// `GET /api/audit` is a QUERY over the caller's whole branch scope, not a read of
/// one branch's row — `fetch_audit_records` is already handed `branch_scope` and
/// confines the rows. There is no single resource branch to authorize against, so
/// this is a capability check. (Its sibling `authorize_audit_attestation` is
/// deliberately org-wide; see its doc comment.)
fn authorize_audit_read(principal: &Principal) -> Result<(), ApiError> {
    authorize_capability(principal, Action::new(Feature::AuditLogRead))
        .map_err(ApiError::from_kernel)
}

fn authorize_audit_attestation(principal: &Principal) -> Result<(), ApiError> {
    // Whole-tenant attestation is intentionally stricter than branch-filtered
    // `/api/audit`: built-in ADMIN is operational/branch authority, not an
    // org-wide evidentiary attestation authority. `authorize_org_wide` first
    // requires all-branch scope, then applies the feature matrix; for
    // `AuditLogRead` that means built-in SUPER_ADMIN today, while still
    // preserving the custom-grant path for future policy-managed org-wide
    // AuditLogRead.
    authorize_org_wide(principal, Action::new(Feature::AuditLogRead)).map_err(ApiError::from_kernel)
}

fn normalize_audit_query(query: AuditQuery) -> Result<NormalizedAuditQuery, ApiError> {
    let limit = query.limit.unwrap_or(DEFAULT_AUDIT_LIMIT);
    if !(1..=MAX_AUDIT_LIMIT).contains(&limit) {
        return Err(ApiError::validation(format!(
            "limit must be between 1 and {MAX_AUDIT_LIMIT}"
        )));
    }
    let offset = query.offset.unwrap_or(0);
    if offset < 0 {
        return Err(ApiError::validation("offset must be non-negative"));
    }
    let target_type = query
        .target_type
        .map(|target_type| target_type.trim().to_owned())
        .filter(|target_type| !target_type.is_empty());
    let target_id = query
        .target_id
        .map(|target_id| target_id.trim().to_owned())
        .filter(|target_id| !target_id.is_empty());
    let trace_id = query
        .trace_id
        .map(|trace_id| trace_id.trim().to_owned())
        .filter(|trace_id| !trace_id.is_empty());

    Ok(NormalizedAuditQuery {
        limit,
        offset,
        target_type,
        target_id,
        actor: query.actor,
        trace_id,
    })
}

fn audit_read_event(principal: &Principal) -> Result<AuditEvent, ApiError> {
    let event = AuditEvent::new(
        Some(principal.user_id),
        AuditAction::new("audit.read").map_err(ApiError::from_kernel)?,
        "audit_log",
        "query",
        current_trace_context(),
        time::OffsetDateTime::now_utc(),
    )
    // Arm `app.current_org` for the FORCE-RLS read: `with_audit` binds the GUC
    // from `event.org_id`, so without this the `audit_events` SELECT runs with
    // an unset GUC and RLS fails closed (zero rows) as the `console_rt` role. Also
    // stamps the `audit.read` row with the caller's org instead of NULL.
    .with_org(principal.org_id);
    Ok(match audit_event_branch(&principal.branch_scope) {
        Some(branch_id) => event.with_branch(branch_id),
        None => event,
    })
}

fn audit_event_branch(scope: &BranchScope) -> Option<BranchId> {
    match scope {
        BranchScope::All => None,
        // fabricated-branch: ok stamps the ACTOR's branch on an audit row; never reaches authorize
        BranchScope::Branches(branches) => branches.iter().next().copied(),
    }
}

async fn fetch_audit_records(
    conn: &mut sqlx::PgConnection,
    branch_scope: BranchScope,
    query: NormalizedAuditQuery,
) -> Result<Vec<AuditRecord>, sqlx::Error> {
    let mut builder = QueryBuilder::<Postgres>::new(
        r#"
        SELECT
            id, actor, action, target_type, target_id, branch_id,
            before_snap, after_snap,
            ip, user_agent, auth_method, device,
            classification_badges, anomaly, reason,
            trace_id::text AS trace_id,
            span_id::text AS span_id, occurred_at
        FROM audit_events
        WHERE
        "#,
    );

    match branch_scope {
        BranchScope::All => {
            builder.push("TRUE");
        }
        BranchScope::Branches(branches) if branches.is_empty() => {
            builder.push("FALSE");
        }
        BranchScope::Branches(branches) => {
            let branch_ids: Vec<uuid::Uuid> = branches
                .into_iter()
                .map(|branch_id| *branch_id.as_uuid())
                .collect();
            builder
                .push("branch_id = ANY(")
                .push_bind(branch_ids)
                .push(")");
        }
    }

    if let Some(target_type) = query.target_type {
        builder.push(" AND target_type = ").push_bind(target_type);
    }
    if let Some(target_id) = query.target_id {
        builder.push(" AND target_id = ").push_bind(target_id);
    }
    if let Some(actor) = query.actor {
        builder.push(" AND actor = ").push_bind(actor);
    }
    // `trace_id` is CHAR(32); cast to text so the bound String compares as
    // text=text (matching the SELECT projection) rather than bpchar padding.
    if let Some(trace_id) = query.trace_id {
        builder.push(" AND trace_id::text = ").push_bind(trace_id);
    }
    builder
        .push(" ORDER BY occurred_at DESC, id DESC LIMIT ")
        .push_bind(query.limit)
        .push(" OFFSET ")
        .push_bind(query.offset);

    builder
        .build_query_as::<AuditRecord>()
        .fetch_all(conn)
        .instrument(tracing::info_span!(
            "db.query",
            db_system = "postgresql",
            db_operation = "SELECT",
            db_statement = "SELECT audit_events",
        ))
        .await
}

fn current_trace_context() -> TraceContext {
    let context = tracing::Span::current().context();
    let span_context = context.span().span_context().clone();
    if span_context.is_valid() {
        TraceContext::new(
            span_context.trace_id().to_string(),
            span_context.span_id().to_string(),
        )
        .unwrap_or_else(|_| TraceContext::generate())
    } else {
        TraceContext::generate()
    }
}

fn record_otel_ids(span: &tracing::Span) -> String {
    let context = span.context();
    let span_context = context.span().span_context().clone();
    let fallback = TraceContext::generate();
    let trace_id = if span_context.is_valid() {
        span_context.trace_id().to_string()
    } else {
        fallback.trace_id().to_owned()
    };
    let span_id = if span_context.is_valid() {
        span_context.span_id().to_string()
    } else {
        fallback.span_id().to_owned()
    };
    span.record("trace_id", tracing::field::display(&trace_id));
    span.record("span_id", tracing::field::display(&span_id));
    trace_id
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", message)
    }

    fn validation(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, "validation", message)
    }

    fn service_unavailable(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "service_unavailable",
            message,
        )
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
    }

    fn from_kernel(error: KernelError) -> Self {
        match error.kind {
            ErrorKind::Validation => Self::validation(error.message),
            ErrorKind::Forbidden => Self::forbidden(error.message),
            ErrorKind::NotFound => Self::new(StatusCode::NOT_FOUND, "not_found", error.message),
            ErrorKind::Conflict | ErrorKind::InvalidTransition => {
                Self::new(StatusCode::CONFLICT, "conflict", error.message)
            }
            ErrorKind::Internal => Self::internal(error.message),
        }
    }

    fn from_app(error: AppError) -> Self {
        tracing::error!(error = %error, "audit api failed");
        match error {
            AppError::Config(message) => Self::service_unavailable(message),
            AppError::Database(_)
            | AppError::Io(_)
            | AppError::Storage(_)
            | AppError::Realtime(_)
            | AppError::Telemetry(_)
            | AppError::Worker(_)
            | AppError::Internal(_) => Self::internal("internal server error"),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        (
            self.status,
            Json(ErrorBody {
                error: ErrorPayload {
                    code: self.code,
                    message: self.message,
                },
            }),
        )
            .into_response()
    }
}

pub fn init_tracing(config: &AppConfig) -> Result<TelemetryGuard, AppError> {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let fmt_layer = tracing_subscriber::fmt::layer().json();

    if let Some(endpoint) = &config.otlp_endpoint {
        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .with_endpoint(endpoint)
            .build()
            .map_err(|err| AppError::Telemetry(err.to_string()))?;
        let provider = SdkTracerProvider::builder()
            .with_resource(
                Resource::builder()
                    .with_service_name(config.service_name.clone())
                    .build(),
            )
            .with_batch_exporter(exporter)
            .build();
        let tracer = provider.tracer(config.service_name.clone());
        global::set_tracer_provider(provider.clone());

        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt_layer)
            .with(tracing_opentelemetry::layer().with_tracer(tracer))
            .try_init()
            .map_err(|err| AppError::Telemetry(err.to_string()))?;

        Ok(TelemetryGuard {
            provider: Some(provider),
        })
    } else {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt_layer)
            .try_init()
            .map_err(|err| AppError::Telemetry(err.to_string()))?;

        Ok(TelemetryGuard { provider: None })
    }
}

#[derive(Debug)]
pub struct TelemetryGuard {
    provider: Option<SdkTracerProvider>,
}

impl TelemetryGuard {
    pub fn shutdown(&self) {
        if let Some(provider) = &self.provider
            && let Err(err) = provider.shutdown()
        {
            tracing::warn!(error = %err, "failed to shut down telemetry provider");
        }
    }
}

pub async fn serve(config: AppConfig, state: AppState) -> Result<(), AppError> {
    if let DatabaseDependency::Postgres(pool) = &state.database {
        assert_no_dev_auth_personas(pool).await?;
    }
    match config.role {
        AppRole::Api => serve_api(config, state).await,
        AppRole::Worker => run_dispatch_worker(config, state).await,
        // Migrate mode never builds an AppState (no HTTP server, no JWT/S3
        // wiring); it is dispatched in `main` before `from_config` runs.
        AppRole::Migrate => run_migrations(&config).await,
    }
}

/// Refuse to boot (api or worker role) if a dev-auth persona row leaked into
/// this database. `dev-auth:<org>:<role>` is the synthetic `phone` key
/// `DevPrincipalProvisioner` (console-platform-provisioning) upserts for the
/// local role-switch endpoint; that endpoint only exists in a build compiled
/// with `--features dev-auth`, so any such row in a build WITHOUT that
/// feature means a dev database dump (or a devved-up environment's data)
/// reached somewhere it should not have. Compiled out entirely under
/// `dev-auth` (a dev-auth build is expected to have these rows).
///
/// `pub` (rather than private) so `backend/app/tests/*` can exercise both cfg
/// bodies directly without booting a full HTTP server.
#[cfg(not(feature = "dev-auth"))]
pub async fn assert_no_dev_auth_personas(pool: &sqlx::PgPool) -> Result<(), AppError> {
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE phone LIKE 'dev-auth:%'")
        // rls-arming: ok fail-closed boot guard (global users phone prefix census); enforced by console-gate-dev-auth-absence, not a tenant read
        .fetch_one(pool)
        .await
        .map_err(AppError::Database)?;
    if count > 0 {
        return Err(AppError::Internal(format!(
            "refusing to start: {count} dev-auth:* persona row(s) found in `users` — a dev-only \
             database dump reached a build without the dev-auth feature. Purge those phone-prefix \
             persona rows before starting this environment (phone LIKE 'dev-auth:%')."
        )));
    }
    Ok(())
}

#[cfg(feature = "dev-auth")]
pub async fn assert_no_dev_auth_personas(_pool: &sqlx::PgPool) -> Result<(), AppError> {
    Ok(())
}

/// Apply the embedded schema migrations against `DATABASE_URL`, then return.
///
/// This is the `migrate` run-mode (an Argo CD PreSync Job). It is deliberately
/// lean: it needs ONLY `DATABASE_URL` (the OWNER `console_app` connection that can
/// run DDL) â no JWT keys, S3 creds, or any other app config â so a migration
/// Job can run with a minimal environment. It opens a tiny single-connection
/// pool, runs the migrator (idempotent: sqlx skips versions already recorded in
/// `_sqlx_migrations`), logs how many were applied, then returns so the process
/// can exit 0. Any DDL/connection error propagates so the Job fails (non-zero)
/// and Argo blocks the sync.
pub async fn run_migrations(config: &AppConfig) -> Result<(), AppError> {
    let database_url = config
        .database_url
        .as_deref()
        .ok_or_else(|| AppError::Config("migrate role requires DATABASE_URL".to_owned()))?;

    let pool = migration_database_pool_options()
        .connect(database_url)
        .await
        .map_err(AppError::Database)?;

    // `after_connect` has already enforced the migration budgets and exact
    // migration identity on this physical connection. Keep the same checkout
    // through the preflight count, migration run, and final count so no
    // unvalidated connection can enter the migration execution path.
    // Detach: on every error/cancellation path the physical connection closes,
    // releasing its session lock instead of returning a locked checkout.
    let mut connection = pool.acquire().await.map_err(AppError::Database)?.detach();
    sqlx::query("SET SESSION search_path = public")
        .execute(&mut connection)
        .await
        .map_err(AppError::Database)?;
    acquire_migration_lock(&mut connection).await?;
    let applied_before = applied_migration_count(&mut connection).await?;
    let embedded = MIGRATOR.iter().count();

    tracing::info!(
        embedded,
        applied_before,
        "applying schema migrations (migrate mode)"
    );

    // Close the time-of-check gap as far as the database protocol permits:
    // revalidate the same physical connection immediately before handing it
    // to SQLx's migrator.
    prepare_migration_database_connection(&mut connection)
        .await
        .map_err(AppError::Database)?;

    schema_install::run(&mut connection).await?;

    // Apalis' vendor migrations are deliberately owned by the same one-shot
    // migration boundary rather than by an API/worker login. Reuse the exact
    // already-validated physical checkout so the runtime `console_rt` role never
    // needs schema or migration-ledger write privileges.
    migrate_and_reconcile_apalis_postgres(&mut connection)
        .await
        .map_err(|err| AppError::Internal(format!("apalis migration run failed: {err}")))?;

    // Reassert the owner identity and migration budgets after both migration
    // engines have completed, before reading the final ledger and releasing
    // the physical connection.
    prepare_migration_database_connection(&mut connection)
        .await
        .map_err(AppError::Database)?;

    let applied_after = applied_migration_count(&mut connection).await?;
    let newly_applied = applied_after.saturating_sub(applied_before);

    if newly_applied == 0 {
        tracing::info!(
            applied = applied_after,
            "schema is up to date; nothing to apply"
        );
    } else {
        tracing::info!(
            newly_applied,
            applied = applied_after,
            "schema migrations applied"
        );
    }

    sqlx::Connection::close(connection)
        .await
        .map_err(AppError::Database)?;
    pool.close().await;
    Ok(())
}

async fn acquire_migration_lock(connection: &mut PgConnection) -> Result<(), AppError> {
    // A blocking advisory-lock query remains an open virtual transaction.
    // CREATE INDEX CONCURRENTLY in the current migration owner waits for it,
    // producing a deadlock with another migrator waiting for that same lock.
    // Bound each wait so PostgreSQL can finish its index scan between attempts.
    sqlx::query("SET SESSION lock_timeout = '100ms'")
        .execute(&mut *connection)
        .await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        match sqlx::migrate::Migrate::lock(&mut *connection).await {
            Ok(()) => break,
            Err(sqlx::migrate::MigrateError::Execute(sqlx::Error::Database(error)))
                if error.code().as_deref() == Some("55P03")
                    && tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            Err(error) => {
                return Err(AppError::Internal(format!(
                    "migration lock failed: {error}"
                )));
            }
        }
    }
    enforce_migration_database_budgets(connection).await?;
    Ok(())
}

/// Count rows in sqlx's `_sqlx_migrations` ledger, returning 0 before the table
/// exists (a fresh database, before the first `MIGRATOR.run`). Used only to log
/// how many migrations a `migrate` run newly applied.
async fn applied_migration_count(connection: &mut PgConnection) -> Result<usize, AppError> {
    let exists: bool =
        sqlx::query_scalar("SELECT to_regclass('public._sqlx_migrations') IS NOT NULL")
            .fetch_one(&mut *connection)
            .await
            .map_err(AppError::Database)?;
    if !exists {
        return Ok(0);
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations WHERE success")
        .fetch_one(connection)
        .await
        .map_err(AppError::Database)?;
    usize::try_from(count)
        .map_err(|err| AppError::Internal(format!("invalid migration count: {err}")))
}

async fn serve_api(config: AppConfig, state: AppState) -> Result<(), AppError> {
    install_metrics_recorder()?;
    seed_cold_start_otp(&config, &state).await?;

    let listener = tokio::net::TcpListener::bind(config.http_addr)
        .await
        .map_err(AppError::Io)?;
    tracing::info!(
        service = %config.service_name,
        role = %config.role,
        addr = %config.http_addr,
        "starting console-app"
    );

    axum::serve(
        listener,
        build_router(state.clone()).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal(config.shutdown_timeout, state))
    .await
    .map_err(AppError::Io)
}

/// Seed the cold-start admin's bootstrap OTP at API boot, after migrations have
/// been applied to the database.
///
/// Runs only for the API role with a configured `CONSOLE_COLDSTART_OTP` and a live
/// database. The seeding itself is idempotent and race-safe in the provisioning
/// crate: it inserts a credential only when the cold-start admin has neither a
/// passkey nor an open credential. The OTP value is NEVER logged â only whether a
/// credential was seeded or skipped.
async fn seed_cold_start_otp(config: &AppConfig, state: &AppState) -> Result<(), AppError> {
    let Some(otp) = config.coldstart_otp.as_deref() else {
        tracing::info!(
            "no cold-start OTP configured; skipping cold-start seed (normal once admins exist)"
        );
        return Ok(());
    };
    let DatabaseDependency::Postgres(pool) = &state.database else {
        tracing::info!(
            "cold-start OTP configured but no database is wired; skipping cold-start seed"
        );
        return Ok(());
    };

    let seeded = BootstrapCredentialStore
        .seed_cold_start_credential(
            pool,
            otp,
            config.coldstart_otp_ttl,
            time::OffsetDateTime::now_utc(),
        )
        .await
        .map_err(|err| AppError::Internal(format!("cold-start seed failed: {err}")))?;
    if seeded {
        tracing::info!("cold-start OTP seeded for the cold-start admin");
    } else {
        tracing::info!(
            "cold-start OTP not seeded (admin already has a passkey or an open credential)"
        );
    }
    Ok(())
}

async fn run_dispatch_worker(config: AppConfig, state: AppState) -> Result<(), AppError> {
    install_metrics_recorder()?;
    let database_url = config
        .database_url
        .as_deref()
        .ok_or_else(|| AppError::Config("worker role requires DATABASE_URL".to_owned()))?;
    let pool = match &state.database {
        DatabaseDependency::Postgres(pool) => pool.clone(),
        DatabaseDependency::NotConfigured => {
            return Err(AppError::Config(
                "worker role requires a configured database".to_owned(),
            ));
        }
    };
    tracing::info!(
        service = %config.service_name,
        role = %config.role,
        queue = "console.dispatch",
        "starting console-app worker"
    );
    // M2 workflow-runtime payroll outbox drainer (design §B/§F). Runs alongside
    // the apalis dispatch worker on the same `console_rt` pool, re-arming
    // `app.current_org` per tenant each tick. Lands dark: no tenant is enrolled in
    // a shipped migration/seed, so it finds no work in production.
    let workflow_drain_handle = workflow_drain::spawn(pool.clone());
    // L20 tamper-evident audit-chain seal worker (charter §5.1). Seals batches of
    // audit_events into the append-only audit_chain_seals hash chain on the same
    // `console_rt` pool, re-arming `app.current_org` per tenant each tick.
    // Production path: ExternalSealSigner + HTTP SealSignTransport to an
    // out-of-process custody daemon (console-ae5 wiring). Without custody config
    // the worker fails closed unless CONSOLE_AUDIT_CHAIN_ALLOW_DEV_SIGNER=true.
    // The attestation REST endpoint reads whatever is sealed regardless of this
    // gate. Default OFF.
    let audit_chain_handle = if config.audit_chain_seal_enabled {
        match audit_chain_signer::build_seal_worker_signer(
            config.audit_chain_external.as_ref(),
            config.audit_chain_allow_dev_signer,
        ) {
            Ok(Some(signer)) => {
                tracing::info!(
                    key_ref = %signer.key_ref(),
                    external = config.audit_chain_external.is_some(),
                    "starting audit-chain seal worker"
                );
                Some(console_platform_audit_chain::spawn(pool.clone(), signer))
            }
            Ok(None) => None,
            Err(err) => {
                tracing::error!(error = %err, "audit-chain signer init failed; seal worker not started");
                None
            }
        }
    } else {
        tracing::info!(
            "CONSOLE_AUDIT_CHAIN_SEAL_ENABLED is not set; the audit-chain seal worker is OFF"
        );
        None
    };
    // Workflow cron-schedule poller (BE-AUTO slice 1). Same worker-role,
    // per-tenant re-armed loop shape as the drainer. Dark-safe: no shipped
    // migration/seed creates a schedule row, so it finds no work until a tenant
    // authors one through the audited studio REST surface.
    let workflow_schedule_handle = workflow_schedules::spawn(pool.clone());
    let facilities_schedule_handle = facilities_schedule::spawn(pool.clone());
    let alimtalk_policy = if config.solapi.is_some() {
        AlimtalkEscalationPolicy::enabled()
    } else {
        AlimtalkEscalationPolicy::disabled()
    };
    let dispatch_worker = DispatchWorker::new(
        PgDispatchStore::new(pool),
        state.push_notifier.clone(),
        alimtalk_policy,
    );
    // Compose the dispatch-timer worker with the evidence-transcode handler so a
    // SINGLE apalis worker on the `console.dispatch` queue services both job
    // families. EvidenceTranscode is routed to the transcode handler (which owns
    // the EvidenceService + ffmpeg processor + a concurrency cap); everything
    // else falls through to the dispatch worker.
    let evidence_worker = state.evidence_storage.clone().map(|service| {
        EvidenceTranscodeWorker::new(service, config.evidence_transcode_concurrency)
    });
    if evidence_worker.is_none() {
        tracing::warn!(
            "evidence storage is not configured; evidence transcode jobs will be rejected"
        );
    }
    let worker = CompositeJobHandler {
        dispatch: dispatch_worker,
        evidence: evidence_worker,
    };

    // The worker exposes no API surface, but orchestrators (Compose/K8s) still
    // need a liveness/readiness probe. Serve /healthz + /readyz on the same
    // address the API role uses, concurrently with the apalis worker.
    let health_listener = tokio::net::TcpListener::bind(config.http_addr)
        .await
        .map_err(AppError::Io)?;
    tracing::info!(addr = %config.http_addr, "worker health server listening");
    let health_router = with_metrics(
        Router::new()
            .route("/healthz", get(healthz))
            .route("/readyz", get(readyz))
            .with_state(state.clone()),
        &state,
    );
    let health_server = tokio::spawn(async move {
        if let Err(err) = axum::serve(health_listener, health_router).await {
            tracing::warn!(error = %err, "worker health server stopped");
        }
    });

    // Kubernetes sets this explicitly via the Downward API in worker.yaml. Do
    // not use generic HOSTNAME: Docker/local shells often set it too, and those
    // environments should keep the stable service-name-only worker identity.
    let pod_name = env::var("CONSOLE_POD_NAME").ok();
    let worker_name = dispatch_apalis_worker_name(&config.service_name, pod_name.as_deref());
    let result = run_apalis_worker_until_shutdown(
        database_url,
        "console.dispatch",
        worker_name,
        worker,
        shutdown_signal(config.shutdown_timeout, state.clone()),
    )
    .await
    .map_err(|err| AppError::Worker(err.to_string()));

    workflow_drain_handle.shutdown();
    if let Some(handle) = audit_chain_handle {
        handle.shutdown();
    }
    workflow_schedule_handle.shutdown();
    facilities_schedule_handle.shutdown();
    health_server.abort();
    result
}

fn dispatch_apalis_worker_name(service_name: &str, pod_name: Option<&str>) -> String {
    let service_name = service_name.trim();
    let service_name = if service_name.is_empty() {
        DEFAULT_SERVICE_NAME
    } else {
        service_name
    };
    match pod_name.map(str::trim).filter(|value| !value.is_empty()) {
        Some(pod_name) => format!("{service_name}-{pod_name}-dispatch-worker"),
        None => format!("{service_name}-dispatch-worker"),
    }
}

/// Routes apalis jobs on the shared `console.dispatch` queue: `EvidenceTranscode`
/// goes to the evidence handler, everything else to the dispatch-timer worker.
struct CompositeJobHandler {
    dispatch: DispatchWorker,
    evidence: Option<EvidenceTranscodeWorker>,
}

impl PlatformJobHandler for CompositeJobHandler {
    fn handle<'a>(&'a self, job: PlatformJob) -> BoxFuture<'a, Result<(), JobQueueError>> {
        Box::pin(async move {
            match job {
                PlatformJob::EvidenceTranscode(evidence_job) => match &self.evidence {
                    Some(worker) => {
                        worker
                            .handle(evidence_job.org_id, evidence_job.evidence_id)
                            .await
                    }
                    None => Err(JobQueueError::Worker(
                        "evidence storage is not configured for transcode jobs".to_owned(),
                    )),
                },
                // Delegate via the PlatformJobHandler impl so the dispatch
                // worker's error is mapped into JobQueueError.
                other => PlatformJobHandler::handle(&self.dispatch, other).await,
            }
        })
    }
}

/// Background handler that transcodes/optimizes a staged evidence original into
/// the final 1080p/recompressed deliverable. Arms `app.current_org` to the job's
/// tenant (RLS), claims the pending row, runs ffmpeg/image processing behind a
/// concurrency cap (backpressure), and transitions PROCESSING → READY/FAILED.
struct EvidenceTranscodeWorker {
    service: EvidenceService<SeaweedS3Storage>,
    processor: FfmpegMediaProcessor,
    permits: Arc<tokio::sync::Semaphore>,
}

impl EvidenceTranscodeWorker {
    fn new(service: EvidenceService<SeaweedS3Storage>, concurrency: usize) -> Self {
        let concurrency = concurrency.max(1);
        Self {
            service,
            processor: FfmpegMediaProcessor::default(),
            permits: Arc::new(tokio::sync::Semaphore::new(concurrency)),
        }
    }

    async fn handle(&self, org: OrgId, evidence_id: EvidenceId) -> Result<(), JobQueueError> {
        // Backpressure: cap the number of concurrent transcodes regardless of how
        // many jobs apalis hands us, so a burst of uploads can't exhaust CPU/disk.
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|err| JobQueueError::Worker(err.to_string()))?;
        // Arm the tenant from the job payload BEFORE any RLS-gated work; the
        // EvidenceService reads/writes the staging + status rows under this org.
        console_platform_request_context::scope_org(org, async { self.process(evidence_id).await })
            .await
            .map_err(|err| JobQueueError::Worker(err.to_string()))
    }

    async fn process(&self, evidence_id: EvidenceId) -> Result<(), StorageError> {
        let Some(job) = self.service.claim_processing_job().await? else {
            // Already processed (READY/FAILED) or claimed by another worker —
            // idempotent no-op.
            tracing::info!(
                %evidence_id,
                "evidence transcode: no pending row to claim (already processed)"
            );
            return Ok(());
        };
        let status = self
            .service
            .process_job(
                &self.processor,
                &job,
                TraceContext::generate(),
                time::OffsetDateTime::now_utc(),
            )
            .await?;
        tracing::info!(
            media_id = %job.media_id,
            work_order_id = %job.work_order_id,
            status = status.as_db_str(),
            "evidence transcode complete"
        );
        Ok(())
    }
}

async fn shutdown_signal(timeout: Duration, state: AppState) {
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::warn!(error = %err, "failed to install ctrl-c handler");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(err) => {
                tracing::warn!(error = %err, "failed to install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }

    tracing::info!(
        timeout_secs = timeout.as_secs(),
        "shutdown signal received; draining in-flight requests and realtime connections"
    );
    state.shutdown_realtime().await;
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("configuration error: {0}")]
    Config(String),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("internal error: {0}")]
    Internal(String),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("storage error: {0}")]
    Storage(#[from] StorageError),
    #[error("telemetry error: {0}")]
    Telemetry(String),
    #[error("worker error: {0}")]
    Worker(String),
    #[error("realtime error: {0}")]
    Realtime(#[from] console_platform_realtime::RealtimeError),
}

impl From<DbError> for AppError {
    fn from(value: DbError) -> Self {
        match value {
            DbError::Sqlx(err) => Self::Database(err),
            DbError::Serialize(err) => Self::Internal(err.to_string()),
            DbError::CodeIssuance(err) => Self::Internal(err),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod trusted_ingress_tests {
    use std::net::SocketAddr;

    use axum::extract::{ConnectInfo, Extension};
    use axum::routing::get;
    use console_platform_request_context::TrustedClientIp;
    use http::Request;
    use tower::ServiceExt;

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn proxy_forwarding_is_disabled_by_default_and_requires_a_trusted_peer_cidr() {
        let direct = super::AppConfig::from_pairs([
            ("CONSOLE_APP_ROLE", "api".to_owned()),
            ("CONSOLE_HTTP_ADDR", "127.0.0.1:0".to_owned()),
        ])
        .unwrap();
        assert_eq!(direct.trusted_proxy_count, 0);
        assert!(direct.trusted_proxy_cidrs.is_empty());

        let err = super::AppConfig::from_pairs([
            ("CONSOLE_APP_ROLE", "api".to_owned()),
            ("CONSOLE_HTTP_ADDR", "127.0.0.1:0".to_owned()),
            ("CONSOLE_TRUSTED_PROXY_COUNT", "1".to_owned()),
        ])
        .expect_err("a nonzero proxy count without trusted peers must fail closed");
        assert!(err.to_string().contains("CONSOLE_TRUSTED_PROXY_CIDRS"));
    }

    #[cfg(not(feature = "test-postgres"))]
    #[tokio::test]
    async fn ingress_accepts_only_a_complete_trusted_proxy_suffix() {
        async fn observed_client_ip(Extension(ip): Extension<TrustedClientIp>) -> String {
            ip.get().to_string()
        }

        let app = console_platform_request_context::with_trusted_client_ip(
            axum::Router::new().route("/__test/client-ip", get(observed_client_ip)),
            2,
            vec!["10.0.0.0/8".parse().unwrap()],
        );
        let mut request = Request::builder()
            .uri("/__test/client-ip")
            .header("x-forwarded-for", "198.51.100.250, 10.0.0.2")
            .body(axum::body::Body::empty())
            .unwrap();
        request
            .extensions_mut()
            .insert(ConnectInfo("10.0.0.3:443".parse::<SocketAddr>().unwrap()));

        let response = app.oneshot(request).await.unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"198.51.100.250");
    }

    #[cfg(not(feature = "test-postgres"))]
    #[tokio::test]
    async fn ingress_rejects_an_untrusted_configured_proxy_suffix() {
        async fn observed_client_ip(Extension(ip): Extension<TrustedClientIp>) -> String {
            ip.get().to_string()
        }

        let app = console_platform_request_context::with_trusted_client_ip(
            axum::Router::new().route("/__test/client-ip", get(observed_client_ip)),
            2,
            vec!["10.0.0.0/8".parse().unwrap()],
        );
        let mut request = Request::builder()
            .uri("/__test/client-ip")
            .header("x-forwarded-for", "198.51.100.250, 203.0.113.7")
            .body(axum::body::Body::empty())
            .unwrap();
        request
            .extensions_mut()
            .insert(ConnectInfo("10.0.0.3:443".parse::<SocketAddr>().unwrap()));

        let response = app.oneshot(request).await.unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"10.0.0.3");
    }

    #[cfg(not(feature = "test-postgres"))]
    #[tokio::test]
    async fn ingress_falls_back_to_transport_peer_for_raw_or_short_xff() {
        async fn observed_client_ip(Extension(ip): Extension<TrustedClientIp>) -> String {
            ip.get().to_string()
        }

        let app = console_platform_request_context::with_trusted_client_ip(
            axum::Router::new().route("/__test/client-ip", get(observed_client_ip)),
            2,
            vec!["10.0.0.0/8".parse().unwrap()],
        );
        let mut request = Request::builder()
            .uri("/__test/client-ip")
            .header("x-forwarded-for", "198.51.100.250")
            .body(axum::body::Body::empty())
            .unwrap();
        request
            .extensions_mut()
            .insert(ConnectInfo("10.0.0.3:443".parse::<SocketAddr>().unwrap()));

        let response = app.oneshot(request).await.unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"10.0.0.3");
    }

    #[cfg(not(feature = "test-postgres"))]
    #[tokio::test]
    async fn ingress_rejects_direct_client_xff_spoofing_even_with_proxy_hops_configured() {
        async fn observed_client_ip(Extension(ip): Extension<TrustedClientIp>) -> String {
            ip.get().to_string()
        }

        let app = console_platform_request_context::with_trusted_client_ip(
            axum::Router::new().route("/__test/client-ip", get(observed_client_ip)),
            1,
            vec!["10.0.0.0/8".parse().unwrap()],
        );
        let mut request = Request::builder()
            .uri("/__test/client-ip")
            .header("x-forwarded-for", "198.51.100.250")
            .body(axum::body::Body::empty())
            .unwrap();
        request
            .extensions_mut()
            .insert(ConnectInfo("192.0.2.10:443".parse::<SocketAddr>().unwrap()));

        let response = app.oneshot(request).await.unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"192.0.2.10");
    }
}

// Every test in this module is `#[cfg(feature = "test-postgres")]`, so without
// that feature the whole module is helpers and imports with no consumer.
#[cfg(all(test, feature = "test-postgres"))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod readiness_tests {
    use axum::extract::State;
    use axum::response::IntoResponse;
    use http::StatusCode;
    use sqlx::PgPool;
    use sqlx::postgres::PgPoolOptions;

    use super::{AppConfig, AppRole, AppState, DatabaseDependency, readyz};

    fn api_config() -> AppConfig {
        AppConfig::from_pairs([
            ("CONSOLE_APP_ROLE", AppRole::Api.to_string()),
            ("CONSOLE_HTTP_ADDR", "127.0.0.1:0".to_owned()),
        ])
        .expect("valid api test config")
    }

    async fn separate_pool(pool: &PgPool) -> PgPool {
        PgPoolOptions::new()
            .max_connections(1)
            .connect_with(pool.connect_options().as_ref().clone())
            .await
            .expect("separate readiness pool connects")
    }

    #[cfg(feature = "test-postgres")]
    #[sqlx::test(migrations = "../crates/platform/db/migrations")]
    async fn api_readiness_fails_closed_when_either_command_pool_degrades(pool: PgPool) {
        let leave = separate_pool(&pool).await;
        let ontology = separate_pool(&pool).await;
        let platform_force = separate_pool(&pool).await;
        let mut state =
            AppState::new(api_config(), DatabaseDependency::Postgres(pool)).expect("state builds");
        state.leave_command_database = DatabaseDependency::Postgres(leave.clone());
        state.ontology_command_database = DatabaseDependency::Postgres(ontology);
        state.platform_force_command_database = DatabaseDependency::Postgres(platform_force);

        let healthy = readyz(State(state.clone())).await.into_response();
        assert_eq!(healthy.status(), StatusCode::OK);

        leave.close().await;
        let degraded = readyz(State(state)).await.into_response();
        assert_eq!(degraded.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod wide_event_middleware_tests {
    use axum::body::{Body, to_bytes};
    use axum::routing::get;
    use http::Request;
    use tower::ServiceExt;

    use super::{AppConfig, AppRole, AppState, DatabaseDependency, install_metrics_recorder};

    #[cfg(not(feature = "test-postgres"))]
    #[tokio::test]
    async fn http_metrics_label_matched_route_template_not_raw_path() {
        install_metrics_recorder().expect("metrics recorder installs once");
        let config = AppConfig::from_pairs([
            ("CONSOLE_APP_ROLE", AppRole::Api.to_string()),
            ("CONSOLE_HTTP_ADDR", "127.0.0.1:0".to_owned()),
            ("CONSOLE_SERVICE_NAME", "console-wide-event-test".to_owned()),
        ])
        .expect("valid app config");
        let state = AppState::new(config, DatabaseDependency::NotConfigured)
            .expect("test state builds without database");
        let app = super::with_metrics(
            axum::Router::new()
                .route("/__wide_event/widgets/{widget_id}", get(|| async { "ok" }))
                .with_state(state.clone()),
            &state,
        );

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/__wide_event/widgets/widget-123?search=secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(response.status().is_success());

        let metrics = app
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let text = String::from_utf8(
            to_bytes(metrics.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();

        assert!(
            text.contains("service_name=\"console-wide-event-test\"")
                && text.contains("http_route=\"/__wide_event/widgets/{widget_id}\""),
            "wide-event metrics must label the matched route template, not the raw request path; got:\n{text}"
        );
        assert!(
            !text.contains("widget-123") && !text.contains("search=secret"),
            "wide-event metrics must not leak raw path params or query strings; got:\n{text}"
        );
    }

    #[cfg(not(feature = "test-postgres"))]
    #[tokio::test]
    async fn http_metrics_label_unmatched_route_sentinel_not_raw_path() {
        install_metrics_recorder().expect("metrics recorder installs once");
        let config = AppConfig::from_pairs([
            ("CONSOLE_APP_ROLE", AppRole::Api.to_string()),
            ("CONSOLE_HTTP_ADDR", "127.0.0.1:0".to_owned()),
            (
                "CONSOLE_SERVICE_NAME",
                "console-wide-event-unmatched-test".to_owned(),
            ),
        ])
        .expect("valid app config");
        let state = AppState::new(config, DatabaseDependency::NotConfigured)
            .expect("test state builds without database");
        let app = super::with_metrics(
            axum::Router::new()
                .route("/__wide_event/known", get(|| async { "ok" }))
                .with_state(state.clone()),
            &state,
        );

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/__wide_event/missing/account-999?token=secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), http::StatusCode::NOT_FOUND);

        let metrics = app
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let text = String::from_utf8(
            to_bytes(metrics.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();

        assert!(
            text.contains("service_name=\"console-wide-event-unmatched-test\"")
                && text.contains("http_route=\"/_unmatched\"")
                && text.contains("http_response_status_code=\"404\""),
            "unmatched wide-event metrics must use the bounded sentinel; got:\n{text}"
        );
        assert!(
            !text.contains("/__wide_event/missing")
                && !text.contains("account-999")
                && !text.contains("token=secret"),
            "unmatched wide-event metrics must not leak raw paths or query strings; got:\n{text}"
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod audit_attestation_auth_tests {
    use std::collections::BTreeSet;

    use console_kernel_core::{BranchId, BranchScope, OrgId, UserId};
    use console_platform_authz::{Principal, Role};

    use super::authorize_audit_attestation;

    fn principal(role: Role, branch_scope: BranchScope) -> Principal {
        Principal::new(
            UserId::new(),
            OrgId::knl(),
            BTreeSet::from([role]),
            branch_scope,
        )
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn audit_attestation_builtin_gate_allows_only_super_admin_for_audit_read() {
        assert!(
            authorize_audit_attestation(&principal(Role::Admin, BranchScope::All)).is_err(),
            "built-in ADMIN is not org-wide attestation authority even with all-branch scope"
        );
        assert!(
            authorize_audit_attestation(&principal(
                Role::Admin,
                BranchScope::single(BranchId::new())
            ))
            .is_err(),
            "branch-scoped ADMIN must not access a whole-tenant attestation"
        );
        assert!(
            authorize_audit_attestation(&principal(Role::SuperAdmin, BranchScope::All)).is_ok()
        );
        assert!(
            authorize_audit_attestation(&principal(Role::Executive, BranchScope::All)).is_err(),
            "EXECUTIVE has org-wide scope semantics, but not AuditLogRead matrix permission"
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod router_layer_tests {
    //! Guards the cross-cutting tower-layer composition in `build_router`.
    //!
    //! The bug being prevented: applying `TraceLayer`/`TimeoutLayer`/body-limit
    //! to the *base* router before merging the domain routers leaves the merged
    //! routes with none of them (axum `merge` does not propagate pre-merge
    //! layers). The fix applies trace + body-limit to the fully-merged router
    //! and applies the timeout to base+domains+platform+auth *before* merging
    //! the long-lived realtime route, so the realtime route escapes the timeout.
    //!
    //! These tests assert the *merge-order semantics* the fix relies on, using
    //! the same `TimeoutLayer` and `.merge()` ordering as `build_router`. A
    //! route merged INSIDE the timeout 408s when its handler is slow; a route
    //! merged AFTER (outside) the timeout is never severed â which is exactly
    //! how the realtime WS/SSE route is composed. (Note: tower-http 0.7's
    //! `TimeoutLayer` times out the *response future*, so it cannot abort an
    //! already-streaming SSE body or an upgraded WS in the first place; merging
    //! the realtime route outside the timeout is belt-and-suspenders correctness
    //! that stays robust even if that behavior ever changes.)

    use std::time::Duration;

    use axum::Router;
    use axum::http::StatusCode;
    use axum::routing::get;
    use tower::ServiceExt;
    use tower_http::timeout::TimeoutLayer;

    use super::REQUEST_TIMEOUT;

    async fn slow() -> &'static str {
        tokio::time::sleep(Duration::from_secs(2)).await;
        "done"
    }

    /// Mirrors `build_router`'s composition: a "timed" bundle wrapped by a short
    /// `TimeoutLayer`, then the realtime-equivalent route merged AFTER it.
    fn compose(timeout: Duration) -> Router {
        let timed =
            Router::new()
                .route("/timed/slow", get(slow))
                .layer(TimeoutLayer::with_status_code(
                    StatusCode::REQUEST_TIMEOUT,
                    timeout,
                ));
        // The realtime route is merged AFTER the timeout layer, exactly as in
        // `build_router`, so it does not inherit the timeout.
        timed.merge(Router::new().route("/realtime/slow", get(slow)))
    }

    #[cfg(not(feature = "test-postgres"))]
    #[tokio::test]
    async fn domain_route_inside_timeout_is_shed_when_slow() {
        let app = compose(Duration::from_millis(200));
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/timed/slow")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::REQUEST_TIMEOUT,
            "a slow handler on a route INSIDE the timeout layer must be shed with 408"
        );
    }

    #[cfg(not(feature = "test-postgres"))]
    #[tokio::test]
    async fn realtime_route_merged_outside_timeout_is_never_shed() {
        // Same short timeout, but the realtime-equivalent route is merged
        // outside it: a handler that runs well past the timeout still completes
        // 200 and is never severed. This is the #1 regression guard â a 30s
        // timeout must never cut a live realtime/SSE connection.
        let app = compose(Duration::from_millis(200));
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/realtime/slow")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "the realtime route is merged OUTSIDE the timeout and must not be timed out"
        );
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn default_request_timeout_is_thirty_seconds() {
        assert_eq!(REQUEST_TIMEOUT, Duration::from_secs(30));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod worker_identity_tests {
    use super::{DEFAULT_SERVICE_NAME, dispatch_apalis_worker_name};

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn dispatch_worker_name_includes_pod_hostname_when_present() {
        let name = dispatch_apalis_worker_name("console-app-worker", Some("console-worker-abc123"));

        assert_eq!(
            name,
            "console-app-worker-console-worker-abc123-dispatch-worker"
        );
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn dispatch_worker_name_falls_back_to_service_name_outside_kubernetes() {
        let name = dispatch_apalis_worker_name("console-app-worker", None);

        assert_eq!(name, "console-app-worker-dispatch-worker");
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn dispatch_worker_name_ignores_empty_hostname() {
        let name = dispatch_apalis_worker_name("console-app-worker", Some("   "));

        assert_eq!(name, "console-app-worker-dispatch-worker");
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn dispatch_worker_name_uses_default_service_for_empty_service_name() {
        let name = dispatch_apalis_worker_name(" ", Some("pod-1"));

        assert_eq!(
            name,
            format!("{DEFAULT_SERVICE_NAME}-pod-1-dispatch-worker")
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod migration_database_budget_tests {
    use std::time::Duration;

    use sqlx::{PgPool, postgres::PgPoolOptions};

    use super::{
        enforce_migration_database_budgets, ensure_expected_migration_database_budgets,
        reset_migration_database_connection,
    };

    // Consumed only by the `test-postgres` test below; this module also holds
    // non-featured tests, so it cannot be gated wholesale.
    #[allow(dead_code)]
    fn isolated_owner_budget_pool_options() -> PgPoolOptions {
        PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(10))
            .after_connect(|conn, _meta| {
                Box::pin(async move { enforce_migration_database_budgets(conn).await })
            })
            .after_release(|conn, _meta| {
                Box::pin(async move { reset_migration_database_connection(conn).await })
            })
    }

    #[allow(dead_code)]
    async fn cluster_identity_snapshot(pool: &PgPool) -> String {
        sqlx::query_scalar(
            r#"SELECT jsonb_build_object(
                    'database_owner', pg_catalog.pg_get_userbyid(database.datdba),
                    'roles', COALESCE((
                        SELECT jsonb_agg(to_jsonb(role_state) ORDER BY role_state.rolname)
                        FROM (
                            SELECT role.rolname,
                                   role.rolpassword,
                                   role.rolcanlogin,
                                   role.rolsuper,
                                   role.rolinherit,
                                   role.rolcreaterole,
                                   role.rolcreatedb,
                                   role.rolreplication,
                                   role.rolbypassrls
                            FROM pg_catalog.pg_authid AS role
                            WHERE role.rolname IN (
                                'console_app', 'console_leave_definer', 'console_ontology_writer'
                            )
                        ) AS role_state
                    ), '[]'::jsonb),
                    'memberships', COALESCE((
                        SELECT jsonb_agg(to_jsonb(membership_state)
                                         ORDER BY membership_state.member,
                                                  membership_state.granted_role)
                        FROM (
                            SELECT member.rolname AS member,
                                   granted.rolname AS granted_role,
                                   membership.admin_option,
                                   membership.inherit_option,
                                   membership.set_option
                            FROM pg_catalog.pg_auth_members AS membership
                            JOIN pg_catalog.pg_roles AS member
                              ON member.oid = membership.member
                            JOIN pg_catalog.pg_roles AS granted
                              ON granted.oid = membership.roleid
                            WHERE member.rolname IN (
                                'console_app', 'console_leave_definer', 'console_ontology_writer'
                            ) OR granted.rolname IN (
                                'console_app', 'console_leave_definer', 'console_ontology_writer'
                            )
                        ) AS membership_state
                    ), '[]'::jsonb)
                )::text
                FROM pg_catalog.pg_database AS database
                WHERE database.datname = current_database()"#,
        )
        .fetch_one(pool)
        .await
        .expect("cluster identity snapshot reads")
    }

    #[allow(dead_code)]
    async fn assert_migration_session(pool: &PgPool, expected_user: &str) {
        let (session_user, current_user, lock_timeout, statement_timeout): (
            String,
            String,
            String,
            String,
        ) = sqlx::query_as(
            r#"SELECT session_user::text,
                      current_user::text,
                      current_setting('lock_timeout'),
                      current_setting('statement_timeout')"#,
        )
        .fetch_one(pool)
        .await
        .expect("migration session settings read back");

        assert_eq!(session_user, expected_user);
        assert_eq!(current_user, expected_user);
        assert_eq!(lock_timeout, "5s");
        assert_eq!(statement_timeout, "1min");
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn migration_database_budgets_accept_exact_readback() {
        assert!(ensure_expected_migration_database_budgets("5s", "1min", true, true).is_ok());
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn migration_database_budgets_reject_any_readback_mismatch() {
        for (lock_timeout_matches, statement_timeout_matches) in
            [(false, true), (true, false), (false, false)]
        {
            let error = ensure_expected_migration_database_budgets(
                "7s",
                "90s",
                lock_timeout_matches,
                statement_timeout_matches,
            )
            .unwrap_err();
            let message = error.to_string();

            assert!(message.contains("lock_timeout=\"7s\""));
            assert!(message.contains("statement_timeout=\"90s\""));
            assert!(message.contains("required lock_timeout=5s and statement_timeout=60s"));
        }
    }

    #[cfg(feature = "test-postgres")]
    #[sqlx::test(migrations = false)]
    async fn migration_pool_applies_and_restores_session_budgets(pool: PgPool) {
        // Snapshot before the test body performs any session mutation. This
        // regression deliberately runs with no migrations so it cannot alter
        // cluster-global role defaults or memberships.
        let identity_before = cluster_identity_snapshot(&pool).await;
        let expected_user: String = sqlx::query_scalar("SELECT session_user::text")
            .fetch_one(&pool)
            .await
            .expect("isolated test database owner reads");
        let connect_options = pool.connect_options().as_ref().clone();
        let migration_pool = isolated_owner_budget_pool_options()
            .connect_with(connect_options)
            .await
            .expect("migration budget pool connects as isolated test database owner");

        assert_migration_session(&migration_pool, &expected_user).await;

        {
            let mut connection = migration_pool
                .acquire()
                .await
                .expect("migration connection acquires for poisoning");
            sqlx::raw_sql(
                r#"
                SET SESSION lock_timeout = '13s';
                SET SESSION statement_timeout = '17s';
                SET SESSION AUTHORIZATION pg_database_owner;
                "#,
            )
            .execute(&mut *connection)
            .await
            .expect(
                "isolated database owner can safely poison only its session authorization and budgets",
            );
        }

        assert_migration_session(&migration_pool, &expected_user).await;
        migration_pool.close().await;
        assert_eq!(cluster_identity_snapshot(&pool).await, identity_before);
    }
}

// As with `readiness_tests`: the single test here is feature-gated, so the
// module has no non-featured content to compile.
#[cfg(all(test, feature = "test-postgres"))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod serving_database_timeout_tests {
    use std::time::Duration;

    use sqlx::{PgPool, postgres::PgPoolOptions};

    use super::{reset_serving_database_connection, validate_database_connection_identity};

    async fn assert_serving_timeouts(pool: &PgPool) {
        let (statement, idle_in_transaction, transaction): (String, String, String) =
            sqlx::query_as(
                r#"SELECT current_setting('statement_timeout'),
                          current_setting('idle_in_transaction_session_timeout'),
                          current_setting('transaction_timeout')"#,
            )
            .fetch_one(pool)
            .await
            .expect("serving timeouts read back");

        assert_eq!(statement, "30s");
        assert_eq!(idle_in_transaction, "30s");
        assert_eq!(transaction, "45s");
    }

    #[cfg(feature = "test-postgres")]
    #[sqlx::test(migrations = false)]
    async fn serving_pool_rejects_overrides_and_restores_role_defaults_after_release(pool: PgPool) {
        let backend_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&pool)
            .await
            .expect("test backend pid reads");
        let role = format!("console_serving_timeout_test_{backend_pid}");
        let password = "serving-timeout-test-password";
        let quoted_database: String = sqlx::query_scalar("SELECT quote_ident(current_database())")
            .fetch_one(&pool)
            .await
            .expect("test database name quotes");

        // `role` is a fixed prefix plus the numeric backend PID, `password` is
        // a test literal, and PostgreSQL produced `quoted_database` with
        // quote_ident; no external input reaches this audited dynamic DDL.
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            r#"CREATE ROLE {role}
                    LOGIN NOINHERIT NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE NOREPLICATION
                    PASSWORD '{password}';
               ALTER ROLE {role} SET statement_timeout = '30s';
               ALTER ROLE {role} SET idle_in_transaction_session_timeout = '30s';
               ALTER ROLE {role} SET transaction_timeout = '45s';
               GRANT CONNECT ON DATABASE {quoted_database} TO {role};"#,
        )))
        .execute(&pool)
        .await
        .expect("isolated hardened serving role provisions");

        let connect_options = pool
            .connect_options()
            .as_ref()
            .clone()
            .username(&role)
            .password(password);
        let after_connect_role = role.clone();
        let after_release_role = role.clone();
        let serving_pool = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(10))
            .after_connect(move |conn, _meta| {
                let expected_role = after_connect_role.clone();
                Box::pin(async move {
                    validate_database_connection_identity(
                        conn,
                        "TEST_SERVING_DATABASE_URL",
                        &expected_role,
                    )
                    .await
                })
            })
            .after_release(move |conn, _meta| {
                let expected_role = after_release_role.clone();
                Box::pin(async move {
                    reset_serving_database_connection(
                        conn,
                        "TEST_SERVING_DATABASE_URL",
                        &expected_role,
                    )
                    .await
                })
            })
            .connect_with(connect_options.clone())
            .await
            .expect("exact role defaults pass serving startup validation");

        assert_serving_timeouts(&serving_pool).await;
        {
            let mut connection = serving_pool
                .acquire()
                .await
                .expect("serving connection acquires for poisoning");
            sqlx::raw_sql(
                r#"SET SESSION statement_timeout = '1s';
                   SET SESSION idle_in_transaction_session_timeout = '2s';
                   SET SESSION transaction_timeout = '3s';"#,
            )
            .execute(&mut *connection)
            .await
            .expect("serving session timeout poison applies");
        }
        assert_serving_timeouts(&serving_pool).await;
        serving_pool.close().await;

        let _startup_override = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(3))
            .after_connect({
                let role = role.clone();
                move |conn, _meta| {
                    let role = role.clone();
                    Box::pin(async move {
                        validate_database_connection_identity(
                            conn,
                            "TEST_SERVING_DATABASE_URL",
                            &role,
                        )
                        .await
                    })
                }
            })
            .connect_with(
                connect_options
                    .clone()
                    .options([("statement_timeout", "29s")]),
            )
            .await
            .expect_err("startup options must not override serving role defaults");

        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "ALTER ROLE {role} SET transaction_timeout = '44s';"
        )))
        .execute(&pool)
        .await
        .expect("isolated role default drifts");
        let _wrong_default = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(3))
            .after_connect({
                let role = role.clone();
                move |conn, _meta| {
                    let role = role.clone();
                    Box::pin(async move {
                        validate_database_connection_identity(
                            conn,
                            "TEST_SERVING_DATABASE_URL",
                            &role,
                        )
                        .await
                    })
                }
            })
            .connect_with(connect_options)
            .await
            .expect_err("wrong serving role default must fail startup");

        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "REVOKE CONNECT ON DATABASE {quoted_database} FROM {role}; DROP ROLE {role};"
        )))
        .execute(&pool)
        .await
        .expect("isolated serving role drops");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod command_database_config_tests {
    use super::{
        AppConfig, AppRole, RoleAttributes, RoleMembership, SubordinateRoleContract,
        ensure_expected_migration_database_identity, ensure_expected_serving_database_identity,
        ensure_expected_serving_database_timeouts, postgres_options_set_role,
        serving_database_identity_query, validate_database_url_identity,
    };

    const RUNTIME_URL: &str = "postgresql://console_rt:runtime-secret@db/console";
    const LEAVE_COMMAND_URL: &str = "postgresql://console_leave_cmd:leave-secret@db/console";
    const ONTOLOGY_COMMAND_URL: &str =
        "postgresql://console_ontology_cmd:ontology-secret@db/console";
    const PLATFORM_FORCE_COMMAND_URL: &str =
        "postgresql://console_platform_force_cmd:platform-force-secret@db/console";

    fn migration_memberships() -> Vec<RoleMembership> {
        ["console_leave_definer", "console_ontology_writer"]
            .into_iter()
            .map(|role_name| RoleMembership {
                role_name: role_name.to_owned(),
                admin_option: false,
                inherit_option: true,
                set_option: true,
            })
            .collect()
    }

    fn migration_subordinate_roles() -> Vec<SubordinateRoleContract> {
        ["console_leave_definer", "console_ontology_writer"]
            .into_iter()
            .map(|role_name| SubordinateRoleContract {
                role_name: role_name.to_owned(),
                attributes: RoleAttributes::HARDENED_DEFINER,
                has_unexpected_membership: false,
            })
            .collect()
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn api_with_database_requires_distinct_leave_command_database_url() {
        let error =
            AppConfig::from_pairs([("CONSOLE_APP_ROLE", "api"), ("DATABASE_URL", RUNTIME_URL)])
                .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("LEAVE_COMMAND_DATABASE_URL is required for api role"),
            "unexpected error: {error}"
        );
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn blank_leave_command_database_url_fails_closed_for_database_backed_api() {
        assert!(
            AppConfig::from_pairs([
                ("CONSOLE_APP_ROLE", "api"),
                ("DATABASE_URL", RUNTIME_URL),
                ("LEAVE_COMMAND_DATABASE_URL", "   "),
            ])
            .is_err()
        );
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn api_accepts_distinct_leave_command_database_url() {
        let config = AppConfig::from_pairs([
            ("CONSOLE_APP_ROLE", "api"),
            ("DATABASE_URL", RUNTIME_URL),
            ("LEAVE_COMMAND_DATABASE_URL", LEAVE_COMMAND_URL),
            ("ONTOLOGY_COMMAND_DATABASE_URL", ONTOLOGY_COMMAND_URL),
            (
                "PLATFORM_FORCE_COMMAND_DATABASE_URL",
                PLATFORM_FORCE_COMMAND_URL,
            ),
        ])
        .unwrap();

        assert_eq!(config.role, AppRole::Api);
        assert_eq!(
            config.leave_command_database_url.as_deref(),
            Some(LEAVE_COMMAND_URL)
        );
        assert_eq!(
            config.ontology_command_database_url.as_deref(),
            Some(ONTOLOGY_COMMAND_URL)
        );
        assert_eq!(
            config.platform_force_command_database_url.as_deref(),
            Some(PLATFORM_FORCE_COMMAND_URL)
        );
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn api_with_database_requires_distinct_ontology_command_database_url() {
        let error = AppConfig::from_pairs([
            ("CONSOLE_APP_ROLE", "api"),
            ("DATABASE_URL", RUNTIME_URL),
            ("LEAVE_COMMAND_DATABASE_URL", LEAVE_COMMAND_URL),
        ])
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("ONTOLOGY_COMMAND_DATABASE_URL is required for api role"),
            "unexpected error: {error}"
        );
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn api_with_database_requires_platform_force_command_database_url() {
        let error = AppConfig::from_pairs([
            ("CONSOLE_APP_ROLE", "api"),
            ("DATABASE_URL", RUNTIME_URL),
            ("LEAVE_COMMAND_DATABASE_URL", LEAVE_COMMAND_URL),
            ("ONTOLOGY_COMMAND_DATABASE_URL", ONTOLOGY_COMMAND_URL),
        ])
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("PLATFORM_FORCE_COMMAND_DATABASE_URL is required for api role")
        );
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn api_rejects_command_urls_equal_to_runtime_or_each_other() {
        let leave_equals_runtime = AppConfig::from_pairs([
            ("CONSOLE_APP_ROLE", "api"),
            ("DATABASE_URL", RUNTIME_URL),
            ("LEAVE_COMMAND_DATABASE_URL", RUNTIME_URL),
            ("ONTOLOGY_COMMAND_DATABASE_URL", ONTOLOGY_COMMAND_URL),
            (
                "PLATFORM_FORCE_COMMAND_DATABASE_URL",
                PLATFORM_FORCE_COMMAND_URL,
            ),
        ])
        .unwrap_err();
        assert!(
            leave_equals_runtime
                .to_string()
                .contains("LEAVE_COMMAND_DATABASE_URL must be distinct from DATABASE_URL")
        );

        let ontology_equals_runtime = AppConfig::from_pairs([
            ("CONSOLE_APP_ROLE", "api"),
            ("DATABASE_URL", RUNTIME_URL),
            ("LEAVE_COMMAND_DATABASE_URL", LEAVE_COMMAND_URL),
            ("ONTOLOGY_COMMAND_DATABASE_URL", RUNTIME_URL),
            (
                "PLATFORM_FORCE_COMMAND_DATABASE_URL",
                PLATFORM_FORCE_COMMAND_URL,
            ),
        ])
        .unwrap_err();
        assert!(
            ontology_equals_runtime
                .to_string()
                .contains("ONTOLOGY_COMMAND_DATABASE_URL must be distinct from DATABASE_URL")
        );

        let shared_command_url = AppConfig::from_pairs([
            ("CONSOLE_APP_ROLE", "api"),
            ("DATABASE_URL", RUNTIME_URL),
            ("LEAVE_COMMAND_DATABASE_URL", LEAVE_COMMAND_URL),
            ("ONTOLOGY_COMMAND_DATABASE_URL", LEAVE_COMMAND_URL),
            (
                "PLATFORM_FORCE_COMMAND_DATABASE_URL",
                PLATFORM_FORCE_COMMAND_URL,
            ),
        ])
        .unwrap_err();
        assert!(shared_command_url.to_string().contains(
            "ONTOLOGY_COMMAND_DATABASE_URL must be distinct from LEAVE_COMMAND_DATABASE_URL"
        ));

        for (conflicting_url, expected) in [
            (RUNTIME_URL, "DATABASE_URL"),
            (LEAVE_COMMAND_URL, "LEAVE_COMMAND_DATABASE_URL"),
            (ONTOLOGY_COMMAND_URL, "ONTOLOGY_COMMAND_DATABASE_URL"),
        ] {
            let error = AppConfig::from_pairs([
                ("CONSOLE_APP_ROLE", "api"),
                ("DATABASE_URL", RUNTIME_URL),
                ("LEAVE_COMMAND_DATABASE_URL", LEAVE_COMMAND_URL),
                ("ONTOLOGY_COMMAND_DATABASE_URL", ONTOLOGY_COMMAND_URL),
                ("PLATFORM_FORCE_COMMAND_DATABASE_URL", conflicting_url),
            ])
            .unwrap_err();
            assert!(
                error.to_string().contains(expected),
                "platform-force URL collision with {expected} must be rejected: {error}"
            );
        }
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn connected_role_guards_require_exact_direct_identity_without_membership() {
        assert!(
            ensure_expected_serving_database_identity(
                "LEAVE_COMMAND_DATABASE_URL",
                "console_leave_cmd",
                "console_leave_cmd",
                "console_leave_cmd",
                RoleAttributes::HARDENED_LOGIN,
                false,
            )
            .is_ok()
        );
        assert!(
            ensure_expected_serving_database_identity(
                "DATABASE_URL",
                "console_rt",
                "console_leave_cmd",
                "console_rt",
                RoleAttributes::HARDENED_LOGIN,
                false,
            )
            .is_err()
        );
        assert!(
            ensure_expected_serving_database_identity(
                "DATABASE_URL",
                "local_dev",
                "console_rt",
                "console_rt",
                RoleAttributes::HARDENED_LOGIN,
                false,
            )
            .is_err()
        );
        assert!(
            ensure_expected_serving_database_identity(
                "DATABASE_URL",
                "console_rt",
                "console_rt",
                "console_rt",
                RoleAttributes::HARDENED_LOGIN,
                true,
            )
            .is_err()
        );
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn serving_role_guards_reject_each_escalating_attribute() {
        let hostile_attributes = [
            RoleAttributes {
                can_login: false,
                ..RoleAttributes::HARDENED_LOGIN
            },
            RoleAttributes {
                is_superuser: true,
                ..RoleAttributes::HARDENED_LOGIN
            },
            RoleAttributes {
                bypasses_rls: true,
                ..RoleAttributes::HARDENED_LOGIN
            },
            RoleAttributes {
                inherits_privileges: true,
                ..RoleAttributes::HARDENED_LOGIN
            },
            RoleAttributes {
                can_create_db: true,
                ..RoleAttributes::HARDENED_LOGIN
            },
            RoleAttributes {
                can_create_role: true,
                ..RoleAttributes::HARDENED_LOGIN
            },
            RoleAttributes {
                can_replicate: true,
                ..RoleAttributes::HARDENED_LOGIN
            },
        ];

        for attributes in hostile_attributes {
            assert!(
                ensure_expected_serving_database_identity(
                    "DATABASE_URL",
                    "console_rt",
                    "console_rt",
                    "console_rt",
                    attributes,
                    false,
                )
                .is_err()
            );
        }
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn database_urls_reject_login_role_aliases_and_role_options() {
        assert!(
            validate_database_url_identity(
                "DATABASE_URL",
                "postgresql://local_dev:secret@db/console",
                "console_rt",
            )
            .is_err()
        );

        for options in [
            "-c role=console_rt",
            "-crole=console_rt",
            "--ROLE=console_rt",
            "role=console_rt",
        ] {
            assert!(postgres_options_set_role(options), "options: {options}");
        }
        assert!(!postgres_options_set_role("-c search_path=public"));

        for url in [
            "postgresql://console_rt:secret@db/console?options=-c%20role%3Dconsole_rt",
            "postgresql://console_rt:secret@db/console?options=-crole%3Dconsole_rt",
            "postgresql://console_rt:secret@db/console?options%5Brole%5D=console_rt",
            "postgresql://local_dev:secret@db/console?user=console_rt",
        ] {
            let error =
                validate_database_url_identity("DATABASE_URL", url, "console_rt").unwrap_err();
            assert!(
                error.to_string().contains("must not set PostgreSQL role"),
                "unexpected error: {error}"
            );
        }
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn database_urls_require_nonempty_decoded_passwords() {
        for url in [
            "postgresql://console_rt@db/console",
            "postgresql://console_rt:@db/console",
            "postgresql://console_rt:secret@db/console?password=",
        ] {
            let error =
                validate_database_url_identity("DATABASE_URL", url, "console_rt").unwrap_err();
            let message = error.to_string();
            assert!(
                message.contains("nonempty password"),
                "unexpected error: {message}"
            );
            assert!(!message.contains("secret"));
        }
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn serving_identity_query_checks_all_memberships_and_role_attributes() {
        let query = serving_database_identity_query();
        assert!(query.contains("pg_has_role"));
        assert!(query.contains("'MEMBER'"));
        assert!(query.contains("pg_auth_members"));
        assert!(query.contains("membership.roleid = authenticated.oid"));
        for attribute in [
            "rolcanlogin",
            "rolsuper",
            "rolbypassrls",
            "rolinherit",
            "rolcreatedb",
            "rolcreaterole",
            "rolreplication",
        ] {
            assert!(query.contains(attribute));
        }
        for timeout in [
            "statement_timeout",
            "idle_in_transaction_session_timeout",
            "transaction_timeout",
        ] {
            assert!(query.contains(timeout));
        }
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn serving_timeout_guards_accept_only_exact_effective_defaults() {
        assert!(
            ensure_expected_serving_database_timeouts(
                "DATABASE_URL",
                "30s",
                "30s",
                "45s",
                true,
                true,
                true,
            )
            .is_ok()
        );

        for matches in [
            (false, true, true),
            (true, false, true),
            (true, true, false),
            (false, false, false),
        ] {
            let error = ensure_expected_serving_database_timeouts(
                "DATABASE_URL",
                "29s",
                "31s",
                "44s",
                matches.0,
                matches.1,
                matches.2,
            )
            .unwrap_err();
            let message = error.to_string();

            assert!(message.contains("statement_timeout=\"29s\""));
            assert!(message.contains("idle_in_transaction_session_timeout=\"31s\""));
            assert!(message.contains("transaction_timeout=\"44s\""));
            assert!(message.contains("required statement_timeout=30s"));
            assert!(message.contains("idle_in_transaction_session_timeout=30s"));
            assert!(message.contains("transaction_timeout=45s"));
        }
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn migration_identity_accepts_only_the_exact_owner_topology() {
        assert!(
            ensure_expected_migration_database_identity(
                "console_app",
                "console_app",
                "console_app",
                RoleAttributes::HARDENED_MIGRATION_LOGIN,
                &migration_memberships(),
                &migration_subordinate_roles(),
                false,
            )
            .is_ok()
        );

        for (session_user, current_user, owner) in [
            ("bootstrap", "console_app", "console_app"),
            ("console_app", "console_leave_definer", "console_app"),
            ("console_app", "console_app", "bootstrap"),
        ] {
            assert!(
                ensure_expected_migration_database_identity(
                    session_user,
                    current_user,
                    owner,
                    RoleAttributes::HARDENED_MIGRATION_LOGIN,
                    &migration_memberships(),
                    &migration_subordinate_roles(),
                    false,
                )
                .is_err()
            );
        }

        let hostile_attributes = [
            RoleAttributes {
                can_login: false,
                ..RoleAttributes::HARDENED_MIGRATION_LOGIN
            },
            RoleAttributes {
                is_superuser: true,
                ..RoleAttributes::HARDENED_MIGRATION_LOGIN
            },
            RoleAttributes {
                bypasses_rls: false,
                ..RoleAttributes::HARDENED_MIGRATION_LOGIN
            },
            RoleAttributes {
                inherits_privileges: false,
                ..RoleAttributes::HARDENED_MIGRATION_LOGIN
            },
            RoleAttributes {
                can_create_db: true,
                ..RoleAttributes::HARDENED_MIGRATION_LOGIN
            },
            RoleAttributes {
                can_create_role: true,
                ..RoleAttributes::HARDENED_MIGRATION_LOGIN
            },
            RoleAttributes {
                can_replicate: true,
                ..RoleAttributes::HARDENED_MIGRATION_LOGIN
            },
        ];
        for attributes in hostile_attributes {
            assert!(
                ensure_expected_migration_database_identity(
                    "console_app",
                    "console_app",
                    "console_app",
                    attributes,
                    &migration_memberships(),
                    &migration_subordinate_roles(),
                    false,
                )
                .is_err()
            );
        }
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn migration_identity_rejects_membership_and_definer_drift() {
        let mut missing_membership = migration_memberships();
        missing_membership.pop();
        let mut admin_membership = migration_memberships();
        admin_membership[0].admin_option = true;
        let mut non_inheritable_membership = migration_memberships();
        non_inheritable_membership[0].inherit_option = false;
        let mut non_settable_membership = migration_memberships();
        non_settable_membership[1].set_option = false;
        let mut extra_membership = migration_memberships();
        extra_membership.push(RoleMembership {
            role_name: "pg_read_all_data".to_owned(),
            admin_option: false,
            inherit_option: true,
            set_option: true,
        });
        for memberships in [
            missing_membership,
            admin_membership,
            non_inheritable_membership,
            non_settable_membership,
            extra_membership,
        ] {
            assert!(
                ensure_expected_migration_database_identity(
                    "console_app",
                    "console_app",
                    "console_app",
                    RoleAttributes::HARDENED_MIGRATION_LOGIN,
                    &memberships,
                    &migration_subordinate_roles(),
                    false,
                )
                .is_err()
            );
        }

        let mut login_definer = migration_subordinate_roles();
        login_definer[0].attributes.can_login = true;
        let mut inheriting_definer = migration_subordinate_roles();
        inheriting_definer[0].attributes.inherits_privileges = true;
        let mut privileged_definer = migration_subordinate_roles();
        privileged_definer[1].attributes.bypasses_rls = true;
        let mut member_definer = migration_subordinate_roles();
        member_definer[1].has_unexpected_membership = true;
        for subordinate_roles in [
            login_definer,
            inheriting_definer,
            privileged_definer,
            member_definer,
        ] {
            assert!(
                ensure_expected_migration_database_identity(
                    "console_app",
                    "console_app",
                    "console_app",
                    RoleAttributes::HARDENED_MIGRATION_LOGIN,
                    &migration_memberships(),
                    &subordinate_roles,
                    false,
                )
                .is_err()
            );
        }

        assert!(
            ensure_expected_migration_database_identity(
                "console_app",
                "console_app",
                "console_app",
                RoleAttributes::HARDENED_MIGRATION_LOGIN,
                &migration_memberships(),
                &migration_subordinate_roles(),
                true,
            )
            .is_err()
        );
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn migration_identity_rejects_incoming_console_app_membership_edge() {
        let error = ensure_expected_migration_database_identity(
            "console_app",
            "console_app",
            "console_app",
            RoleAttributes::HARDENED_MIGRATION_LOGIN,
            &migration_memberships(),
            &migration_subordinate_roles(),
            true,
        )
        .unwrap_err();

        assert!(
            error.to_string().contains("forbidden membership edge"),
            "unexpected error: {error}"
        );
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn serving_identity_rejects_outgoing_or_incoming_membership_edges() {
        let error = ensure_expected_serving_database_identity(
            "DATABASE_URL",
            "console_rt",
            "console_rt",
            "console_rt",
            RoleAttributes::HARDENED_LOGIN,
            true,
        )
        .unwrap_err();
        assert!(error.to_string().contains("membership edge"));
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn api_rejects_pairwise_equal_decoded_database_passwords() {
        let cases = [
            (
                "postgresql://console_rt:shared%2Dsecret@db/console",
                "postgresql://console_leave_cmd:shared-secret@db/leave",
                ONTOLOGY_COMMAND_URL,
                PLATFORM_FORCE_COMMAND_URL,
                "DATABASE_URL and LEAVE_COMMAND_DATABASE_URL",
            ),
            (
                RUNTIME_URL,
                "postgresql://console_leave_cmd:shared%2Dsecret@db/leave",
                "postgresql://console_ontology_cmd:shared-secret@db/ontology",
                PLATFORM_FORCE_COMMAND_URL,
                "LEAVE_COMMAND_DATABASE_URL and ONTOLOGY_COMMAND_DATABASE_URL",
            ),
            (
                "postgresql://console_rt:query-secret@db/runtime",
                LEAVE_COMMAND_URL,
                "postgresql://console_ontology_cmd:different@db/ontology?password=query-secret",
                PLATFORM_FORCE_COMMAND_URL,
                "DATABASE_URL and ONTOLOGY_COMMAND_DATABASE_URL",
            ),
            (
                "postgresql://console_rt:force-secret@db/runtime",
                LEAVE_COMMAND_URL,
                ONTOLOGY_COMMAND_URL,
                "postgresql://console_platform_force_cmd:force-secret@db/platform-force",
                "DATABASE_URL and PLATFORM_FORCE_COMMAND_DATABASE_URL",
            ),
            (
                RUNTIME_URL,
                "postgresql://console_leave_cmd:force-secret@db/leave",
                ONTOLOGY_COMMAND_URL,
                "postgresql://console_platform_force_cmd:force-secret@db/platform-force",
                "LEAVE_COMMAND_DATABASE_URL and PLATFORM_FORCE_COMMAND_DATABASE_URL",
            ),
            (
                RUNTIME_URL,
                LEAVE_COMMAND_URL,
                "postgresql://console_ontology_cmd:force-secret@db/ontology",
                "postgresql://console_platform_force_cmd:force-secret@db/platform-force",
                "ONTOLOGY_COMMAND_DATABASE_URL and PLATFORM_FORCE_COMMAND_DATABASE_URL",
            ),
        ];

        for (runtime, leave, ontology, platform_force, expected_pair) in cases {
            let error = AppConfig::from_pairs([
                ("CONSOLE_APP_ROLE", "api"),
                ("DATABASE_URL", runtime),
                ("LEAVE_COMMAND_DATABASE_URL", leave),
                ("ONTOLOGY_COMMAND_DATABASE_URL", ontology),
                ("PLATFORM_FORCE_COMMAND_DATABASE_URL", platform_force),
            ])
            .unwrap_err();
            let message = error.to_string();
            assert!(
                message.contains(expected_pair),
                "unexpected error: {message}"
            );
            assert!(!message.contains("shared-secret"));
            assert!(!message.contains("query-secret"));
            assert!(!message.contains("force-secret"));
        }
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn database_free_api_does_not_require_leave_command_database_url() {
        let config = AppConfig::from_pairs([("CONSOLE_APP_ROLE", "api")]).unwrap();

        assert!(config.database_url.is_none());
        assert!(config.leave_command_database_url.is_none());
        assert!(config.ontology_command_database_url.is_none());
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn worker_and_migrate_do_not_require_leave_command_database_url() {
        for (role, database_url) in [
            ("worker", RUNTIME_URL),
            (
                "migrate",
                "postgresql://console_app:migration-secret@db/console",
            ),
        ] {
            let config =
                AppConfig::from_pairs([("CONSOLE_APP_ROLE", role), ("DATABASE_URL", database_url)])
                    .unwrap();

            assert!(config.leave_command_database_url.is_none());
            assert!(config.ontology_command_database_url.is_none());
        }
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn worker_requires_exact_runtime_login_but_migrate_keeps_owner_url() {
        assert!(
            AppConfig::from_pairs([
                ("CONSOLE_APP_ROLE", "worker"),
                ("DATABASE_URL", "postgresql://local_dev:secret@db/console",),
            ])
            .is_err()
        );
        assert!(
            AppConfig::from_pairs([
                ("CONSOLE_APP_ROLE", "migrate"),
                ("DATABASE_URL", "postgresql://console_app:secret@db/console",),
            ])
            .is_ok()
        );
        assert!(
            AppConfig::from_pairs([
                ("CONSOLE_APP_ROLE", "migrate"),
                ("DATABASE_URL", "postgresql://console_rt:secret@db/console",),
            ])
            .is_err()
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod email_config_tests {
    //! Guards `email_config_from_vars` robustness. The regression being
    //! prevented: a partially-set `CONSOLE_EMAIL_*` group (ConfigMap supplies
    //! host/port/from, but the Secret with the SMTP creds is not yet
    //! provisioned) must not silently degrade to the OTP-logging stub in
    //! production. Stub OTP logging is allowed only behind an explicit
    //! development/e2e/test policy.

    use std::collections::HashMap;

    use console_platform_email::StubEmailMode;

    use super::{
        AppConfig, AppState, DatabaseDependency, EMAIL_STUB_MODE_ENV, email_config_from_vars,
        email_stub_mode_from_vars,
    };

    /// The full, well-formed `CONSOLE_EMAIL_*` set that yields a live SMTP config.
    fn full_email_vars() -> HashMap<String, String> {
        [
            ("CONSOLE_EMAIL_SMTP_HOST", "smtp.example.com"),
            ("CONSOLE_EMAIL_SMTP_PORT", "587"),
            ("CONSOLE_EMAIL_SMTP_USERNAME", "ocid1.user.oc1..example"),
            ("CONSOLE_EMAIL_SMTP_PASSWORD", "secret"),
            ("CONSOLE_EMAIL_FROM", "noreply@example.com"),
            ("CONSOLE_EMAIL_FROM_NAME", "Console"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
    }

    fn app_pairs() -> Vec<(&'static str, String)> {
        vec![
            ("CONSOLE_APP_ROLE", "api".to_owned()),
            ("CONSOLE_HTTP_ADDR", "127.0.0.1:0".to_owned()),
        ]
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn unset_group_yields_no_config() {
        let vars = HashMap::new();
        assert!(email_config_from_vars(&vars, None).unwrap().is_none());
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn fully_set_group_yields_live_config() {
        let config = email_config_from_vars(&full_email_vars(), None)
            .unwrap()
            .expect("fully-configured email group should yield Some(config)");
        assert_eq!(config.host, "smtp.example.com");
        assert_eq!(config.port, 587);
        assert_eq!(config.username, "ocid1.user.oc1..example");
        assert_eq!(config.from_address, "noreply@example.com");
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn partial_config_missing_username_without_stub_mode_errors() {
        // ConfigMap set host/port/from, but the Secret (username) is absent.
        // In production this must fail closed instead of reaching the OTP-logging stub.
        let mut vars = full_email_vars();
        vars.remove("CONSOLE_EMAIL_SMTP_USERNAME");
        assert!(
            email_config_from_vars(&vars, None).is_err(),
            "missing SMTP username must fail closed unless explicit stub mode is enabled"
        );
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn partial_config_missing_username_with_stub_mode_falls_back_to_stub() {
        let mut vars = full_email_vars();
        vars.remove("CONSOLE_EMAIL_SMTP_USERNAME");
        assert!(
            email_config_from_vars(&vars, Some(StubEmailMode::E2e))
                .unwrap()
                .is_none(),
            "explicit e2e stub mode may fall back to the logging stub"
        );
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn partial_config_missing_password_without_stub_mode_errors() {
        let mut vars = full_email_vars();
        vars.remove("CONSOLE_EMAIL_SMTP_PASSWORD");
        assert!(
            email_config_from_vars(&vars, None).is_err(),
            "missing SMTP password must fail closed unless explicit stub mode is enabled"
        );
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn empty_credentials_without_stub_mode_error() {
        // Empty (not absent) creds — e.g. a Secret mounted with blank values —
        // are treated the same as missing and must fail closed outside stub mode.
        let mut vars = full_email_vars();
        vars.insert("CONSOLE_EMAIL_SMTP_USERNAME".to_owned(), "   ".to_owned());
        vars.insert("CONSOLE_EMAIL_SMTP_PASSWORD".to_owned(), String::new());
        assert!(email_config_from_vars(&vars, None).is_err());
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn email_stub_mode_env_accepts_only_explicit_non_production_modes() {
        let vars: HashMap<String, String> = [(EMAIL_STUB_MODE_ENV, "e2e")]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        assert_eq!(
            email_stub_mode_from_vars(&vars).unwrap(),
            Some(StubEmailMode::E2e)
        );

        let vars: HashMap<String, String> = [(EMAIL_STUB_MODE_ENV, "production")]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        assert!(email_stub_mode_from_vars(&vars).is_err());
    }

    #[cfg(not(feature = "test-postgres"))]
    #[tokio::test]
    async fn app_state_without_smtp_or_stub_mode_fails_closed_without_logging_otp() {
        let config = AppConfig::from_pairs(app_pairs()).unwrap();
        let state = AppState::new(config, DatabaseDependency::NotConfigured).unwrap();

        let result = state
            .email_sender()
            .send_otp(
                "ops@example.com",
                "123456",
                std::time::Duration::from_secs(300),
            )
            .await;

        assert!(
            result.is_err_and(|err| err.to_string().contains("disabled")),
            "without explicit stub mode, missing SMTP must fail closed instead of logging the OTP"
        );
    }

    #[cfg(not(feature = "test-postgres"))]
    #[tokio::test]
    async fn app_state_with_explicit_stub_mode_allows_nonprod_otp_logging_stub() {
        let mut pairs = app_pairs();
        pairs.push((EMAIL_STUB_MODE_ENV, "e2e".to_owned()));
        let config = AppConfig::from_pairs(pairs).unwrap();
        let state = AppState::new(config, DatabaseDependency::NotConfigured).unwrap();

        let result = state
            .email_sender()
            .send_otp(
                "ops@example.com",
                "123456",
                std::time::Duration::from_secs(300),
            )
            .await;

        assert!(result.is_ok());
    }

    /// Exhaustively prove the OTP-log leak footgun is closed: ANY permutation of
    /// the `CONSOLE_EMAIL_*` group that is set WITHOUT both SMTP credentials must
    /// error outside explicit stub mode. This is the prod scenario — a ConfigMap
    /// supplies some non-secret fields while the credential Secret is not yet
    /// provisioned — across every subset of the non-secret fields.
    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn every_partial_config_without_credentials_errors_without_stub_mode() {
        const NON_SECRET_KEYS: [(&str, &str); 4] = [
            ("CONSOLE_EMAIL_SMTP_HOST", "smtp.example.com"),
            ("CONSOLE_EMAIL_SMTP_PORT", "587"),
            ("CONSOLE_EMAIL_FROM", "noreply@example.com"),
            ("CONSOLE_EMAIL_FROM_NAME", "Console"),
        ];
        // All 16 subsets of the 4 non-secret fields, crossed with the 3 broken
        // credential states (no username, no password, neither). The empty subset
        // with no creds is the all-unset case and remains Ok(None); every other
        // configured subset must fail closed.
        for mask in 0u8..(1 << NON_SECRET_KEYS.len()) {
            for creds in [
                &[("CONSOLE_EMAIL_SMTP_USERNAME", "ocid1.user.oc1..example")][..],
                &[("CONSOLE_EMAIL_SMTP_PASSWORD", "secret")][..],
                &[][..],
            ] {
                let mut vars: HashMap<String, String> = HashMap::new();
                for (bit, (key, value)) in NON_SECRET_KEYS.iter().enumerate() {
                    if mask & (1 << bit) != 0 {
                        vars.insert((*key).to_owned(), (*value).to_owned());
                    }
                }
                for (key, value) in creds {
                    vars.insert((*key).to_owned(), (*value).to_owned());
                }
                if vars.is_empty() {
                    assert!(email_config_from_vars(&vars, None).unwrap().is_none());
                    continue;
                }
                assert!(
                    email_config_from_vars(&vars, None).is_err(),
                    "partial config (mask={mask:#06b}, creds={creds:?}) must fail \
                     closed unless explicit stub mode is enabled"
                );
            }
        }
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn creds_present_but_missing_host_still_errors() {
        // With BOTH secrets in hand, a missing non-secret field is a genuine
        // operator misconfiguration and still hard-errors (not the missing-Secret
        // crashloop this guard targets).
        let mut vars = full_email_vars();
        vars.remove("CONSOLE_EMAIL_SMTP_HOST");
        assert!(email_config_from_vars(&vars, None).is_err());
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn creds_present_but_missing_port_still_errors() {
        let mut vars = full_email_vars();
        vars.remove("CONSOLE_EMAIL_SMTP_PORT");
        assert!(email_config_from_vars(&vars, None).is_err());
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn creds_present_but_missing_from_still_errors() {
        let mut vars = full_email_vars();
        vars.remove("CONSOLE_EMAIL_FROM");
        assert!(email_config_from_vars(&vars, None).is_err());
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn creds_present_but_invalid_port_still_errors() {
        // A non-numeric port is an operator typo, not a missing Secret — error.
        let mut vars = full_email_vars();
        vars.insert(
            "CONSOLE_EMAIL_SMTP_PORT".to_owned(),
            "not-a-port".to_owned(),
        );
        assert!(email_config_from_vars(&vars, None).is_err());
    }

    #[cfg(not(feature = "test-postgres"))]
    #[test]
    fn credentials_only_without_relay_fields_errors() {
        // Only the secrets are set (no host/port/from). The operator must supply
        // the relay fields once creds exist; silently sending nowhere or using the
        // stub would hide a broken production delivery path.
        let vars: HashMap<String, String> = [
            ("CONSOLE_EMAIL_SMTP_USERNAME", "ocid1.user.oc1..example"),
            ("CONSOLE_EMAIL_SMTP_PASSWORD", "secret"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
        assert!(email_config_from_vars(&vars, None).is_err());
    }
}
