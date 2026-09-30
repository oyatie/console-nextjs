//! Korean payroll domain kernel.
//!
//! This crate intentionally contains pure, source-versioned data and guardrail
//! math only. It must not call external services, read environment variables, or
//! silently estimate tax-table values. Production payroll release remains gated
//! by licensed 노무사/세무사 validation artifacts.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_kernel_core::KernelError;
use time::Date;
use time::macros::date;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OfficialSource {
    pub authority: &'static str,
    pub title: &'static str,
    pub url: &'static str,
    pub retrieved_on: Date,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectivePeriod {
    pub from: Date,
    pub to_exclusive: Option<Date>,
}

impl EffectivePeriod {
    #[must_use]
    pub const fn new(from: Date, to_exclusive: Option<Date>) -> Self {
        Self { from, to_exclusive }
    }

    #[must_use]
    pub fn contains(self, day: Date) -> bool {
        day >= self.from && self.to_exclusive.is_none_or(|to| day < to)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContributionCode {
    NationalPension,
    HealthInsurance,
    LongTermCare,
    EmploymentUnemployment,
    IndustrialAccident,
}

/// A named legal instrument — the document that actually sets a number.
///
/// Deliberately NOT an agency explainer page. The version anchor is
/// `(법령ID, MST, 시행일자)` for a 법령 and `(행정규칙일련번호, 발령번호)` for a
/// 고시; `promulgation_ko` + `enforced_on` carry the citable half of that here.
/// A `flSeq` file handle is never an anchor: three different `flSeq` values
/// serve byte-identical 별표 2 content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Instrument {
    /// 법령명 or 행정규칙명.
    pub name_ko: &'static str,
    /// The article carrying the number, quoted tightly enough to diff.
    pub article_ko: &'static str,
    /// 공포번호 (법령) or 발령번호 (고시).
    pub promulgation_ko: &'static str,
    /// 시행일자 of the text quoted in `article_ko`.
    pub enforced_on: Date,
    pub url: &'static str,
    pub retrieved_on: Date,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContributionBasis {
    /// 기준소득월액 (국민연금법 제3조제1항제5호), clamped by the 고시 band.
    MonthlyStandardIncome,
    /// 보수월액 (국민건강보험법 제70조).
    MonthlyRemuneration,
    /// 건강보험료액 — 장기요양's statutory basis under 노인장기요양보험법
    /// 제9조제1항. NOT 보수월액: modelling it as a direct rate on 보수월액 is
    /// only arithmetically close, and is wrong by a multiple for anyone with a
    /// 경감/면제.
    HealthInsurancePremium,
    /// 사업종류별 요율 — employer-side, per-entity, never an employee line.
    IndustryTariff,
}

/// 10원 미만 절사. 국고금 관리법 제47조제1항 verbatim (법령ID 009409, MST
/// 218677, efYd 20200609 — the slice in force at every citing row's
/// `effective_from`; MST 276079 / 시행 2026-01-02 is the LATER slice an earlier
/// round read, and its 제47조제1항 is word-for-word the same):
/// 「국고금의 수입 또는 지출에서 10원 미만의 끝수가
/// 있을 때에는 그 끝수는 계산하지 아니하고, 전액이 10원 미만일 때에도 그 전액을
/// 계산하지 아니한다」. The statute says 끝수, not 단수 — the two bridges that
/// reach it use the other word, and quoting either loosely is how a citation
/// stops being checkable.
///
/// Three bridges reach it: 국민건강보험법 제107조, 노인장기요양보험법 제64조
/// (준용), and 국민연금법 제117조. Non-negative input only.
const fn trunc10(won: i128) -> i128 {
    won - won % 10
}

/// The competing candidate when 절사 vs 반올림 is not settled by any text read.
const fn round10(won: i128) -> i128 {
    (won + 5) / 10 * 10
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundingUnit {
    /// The exact won after integer division — no 단수 rule applied.
    ExactWon,
    Trunc10,
    Round10,
}

impl RoundingUnit {
    const fn apply(self, won: i128) -> i128 {
        match self {
            Self::ExactWon => won,
            Self::Trunc10 => trunc10(won),
            Self::Round10 => round10(won),
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ExactWon => "EXACT_WON",
            Self::Trunc10 => "TRUNC_10_WON",
            Self::Round10 => "ROUND_10_WON",
        }
    }
}

/// How one component's won figure is rounded.
///
/// Three states, and the difference between them is the whole proposition:
///
/// - `Resolved` — an instrument prescribes the unit, and it is cited.
/// - `Assumed` — **nothing** prescribes it. The unit is a disclosed assumption
///   and no instrument is named, because naming one that does not settle the
///   question produces a citation that is auditable and false.
/// - `Unresolved` — two instruments compete. Both candidates are computed and
///   the amount is emitted **only when they produce the identical won**;
///   otherwise the component is blocked and names its `question_id`. Picking a
///   default "because it is usually right" is the failure this exists to
///   prevent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rounding {
    Resolved {
        unit: RoundingUnit,
        instrument: Instrument,
    },
    Assumed {
        unit: RoundingUnit,
        note_ko: &'static str,
    },
    Unresolved {
        candidates: [RoundingUnit; 2],
        question_id: &'static str,
    },
}

impl Rounding {
    /// `Err(question_id)` when the unresolved candidates disagree.
    fn apply(self, raw: i128) -> Result<i128, &'static str> {
        match self {
            Self::Resolved { unit, .. } | Self::Assumed { unit, .. } => Ok(unit.apply(raw)),
            Self::Unresolved {
                candidates,
                question_id,
            } => {
                let first = candidates[0].apply(raw);
                if candidates.iter().all(|unit| unit.apply(raw) == first) {
                    Ok(first)
                } else {
                    Err(question_id)
                }
            }
        }
    }
}

/// Who bears the premium, and how the bearer's share is derived from the total.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareRule {
    /// 각각 보험료액의 100분의 50씩 (국민건강보험법 제76조제1항; 준용 for
    /// 장기요양 via 노인장기요양보험법 제11조).
    Half { rounding: Rounding },
    /// The rate already IS the employee share — there is no halving step.
    WholeOfRate,
    /// 사업주 전액 부담 (징수법 제13조제5항). No employee line exists at all.
    EmployerOnly,
}

/// 「월별 건강보험료액의 상한과 하한에 관한 고시」 — the fifth 고시 the source
/// register never listed. Applied to the TOTAL premium, after 단수 절사.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonthlyPremiumClamp {
    pub floor_won: i64,
    pub cap_won: i64,
    pub instrument: Instrument,
}

/// One effective-dated statutory contribution rate, instrument-level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatutoryRate {
    pub code: ContributionCode,
    pub period: EffectivePeriod,
    pub basis: ContributionBasis,
    /// Integer rate/ratio on the basis — NEVER a decimal literal. 장기요양 is
    /// exactly 9,448/71,900 of the 건강보험료액 before 2026-11-27.
    pub rate_num: i64,
    pub rate_den: i64,
    pub total_rounding: Rounding,
    pub employee_share: ShareRule,
    pub clamp: Option<MonthlyPremiumClamp>,
    pub instrument: Instrument,
    /// The instrument that sets the bearer split, when it is a different one.
    pub share_instrument: Option<Instrument>,
    /// What is actually established about the version quoted, stated so a
    /// reviewer is never told more than was verified.
    pub provenance: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonthlyBaseLimit {
    pub period: EffectivePeriod,
    pub minimum_won: i64,
    pub maximum_won: i64,
    pub instrument: Instrument,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MinimumWageRate {
    pub period: EffectivePeriod,
    pub hourly_won: i64,
    /// 10,320 × 8. OURS, not the Minister's — the 고시 carries no daily figure.
    pub daily_8h_won: i64,
    pub monthly_209h_won: i64,
    pub instrument: Instrument,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NtsWithholdingTaxRow {
    pub table_version: &'static str,
    pub monthly_income_tax_won: i64,
    pub local_income_tax_won: i64,
    pub source: OfficialSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayrollDraftInput {
    pub pay_date: Date,
    pub monthly_remuneration_won: i64,
    pub pension_standard_monthly_income_won: Option<i64>,
    pub nts_tax_row: Option<NtsWithholdingTaxRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeductionLine {
    pub code: DeductionCode,
    pub label_ko: &'static str,
    pub amount_won: i64,
    pub source_url: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeductionCode {
    NationalPension,
    HealthInsurance,
    LongTermCare,
    EmploymentInsurance,
    IncomeTax,
    LocalIncomeTax,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayrollDraft {
    pub pay_date: Date,
    pub gross_wage_won: i64,
    pub taxable_income_tax_table_version: &'static str,
    pub lines: Vec<DeductionLine>,
    pub total_employee_deductions_won: i64,
    pub net_pay_won: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeverancePayInput {
    pub hire_date: Date,
    pub exit_date: Date,
    pub average_wage_period_start: Date,
    pub average_wage_period_end: Date,
    pub average_wage_calendar_days: i64,
    pub average_wage_total_won: i64,
    /// 1-day 통상임금 (ordinary wage) in won.
    ///
    /// MANDATORY (not `Option`) by design: 근로기준법 시행령 제2조② requires
    /// severance to use the HIGHER of the average wage and the ordinary wage, so
    /// every caller must supply this. A plain field makes omission a *compile*
    /// error rather than a silent average-wage-only fall back — which is exactly
    /// the under-calculation bug that hurts the absence→exit population whose
    /// depressed 3-month window yields an artificially low average wage. The
    /// caller is responsible for deriving this daily figure from the monthly
    /// 통상임금 (e.g. via the 기준시간 209h rule); the kernel keeps that policy out.
    /// A non-positive value is rejected in `build_severance_pay_draft`.
    pub ordinary_daily_wage_won: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeverancePayDraft {
    pub hire_date: Date,
    pub exit_date: Date,
    pub service_days: i64,
    pub average_wage_period_start: Date,
    pub average_wage_period_end: Date,
    pub average_wage_calendar_days: i64,
    pub average_wage_total_won: i64,
    pub average_daily_wage_milliwon: i64,
    pub ordinary_daily_wage_won: i64,
    /// The daily wage that actually governed severance: max(average, ordinary).
    /// Auditable proof of which statutory basis won.
    pub statutory_daily_wage_milliwon: i64,
    pub statutory_30_day_wage_won: i64,
    pub severance_pay_won: i64,
    pub source: OfficialSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfessionalReviewerKind {
    LaborAttorney,
    TaxAccountant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoldenPayrollCase {
    pub case_id: String,
    pub rate_table_version: String,
    pub professionally_validated: bool,
    /// The case's own declared kernel inputs, embedded WHOLE rather than
    /// copied field-by-field: [`build_line_calculation`] consumes exactly this
    /// type, so when the kernel's input vocabulary moves, every golden-case
    /// construction site fails to COMPILE and must be re-signed. A copied
    /// field set would rot silently while still reading green.
    pub inputs: LineCalculationInput,
    pub expected_total_employee_deductions_won: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfessionalValidation {
    pub reviewer_kind: ProfessionalReviewerKind,
    pub reviewed_on: Date,
    pub artifact_sha256: String,
    pub reviewer_reference: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayrollReleaseGateInput {
    pub rate_table_version: String,
    pub official_source_urls: Vec<String>,
    pub golden_cases: Vec<GoldenPayrollCase>,
    pub professional_validation: Option<ProfessionalValidation>,
}

/// The persisted lifecycle states already admitted by
/// `payroll_draft_runs.status` (migration 0074).
///
/// This is deliberately separate from tax calculation: a lifecycle decision
/// records whether a run may advance toward review, approval, and issuance;
/// it never manufactures a payable amount or an external bank transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayrollRunStatus {
    Staged,
    BlockedLegalGate,
    ReadyForReview,
    Approved,
    Issued,
    Void,
}

impl PayrollRunStatus {
    /// Parse only the storage values defined by migration 0074.
    pub fn parse(value: &str) -> Result<Self, KernelError> {
        match value {
            "STAGED" => Ok(Self::Staged),
            "BLOCKED_LEGAL_GATE" => Ok(Self::BlockedLegalGate),
            "READY_FOR_REVIEW" => Ok(Self::ReadyForReview),
            "APPROVED" => Ok(Self::Approved),
            "ISSUED" => Ok(Self::Issued),
            "VOID" => Ok(Self::Void),
            _ => Err(KernelError::validation("unknown payroll run status")),
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Staged => "STAGED",
            Self::BlockedLegalGate => "BLOCKED_LEGAL_GATE",
            Self::ReadyForReview => "READY_FOR_REVIEW",
            Self::Approved => "APPROVED",
            Self::Issued => "ISSUED",
            Self::Void => "VOID",
        }
    }
}

/// A close-to-payslip command.
///
/// Approval is a pure transition after close prerequisites have been proven.
/// Calculation and issuance remain fail-closed until their regulated evidence
/// contracts are available to the persistence and transport layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayrollRunCommand {
    Calculate,
    Approve { approver_is_creator: bool },
    MarkIssued,
}

/// Facts which must all hold before a payroll run can advance.
///
/// `attendance_month_closes` is intentionally an org-level close. A branch
/// close cannot attest a whole payroll population, and a payroll lock must be
/// active for the *same* organization and exact calendar month.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PayrollClosePrerequisites {
    pub period_start: Date,
    pub period_end: Date,
    pub org_month_close_present: bool,
    pub active_exact_payroll_lock_present: bool,
    pub unresolved_attendance_exception_count: u64,
}

/// The deterministic outcome of one lifecycle command. `idempotent` means a
/// retry observed the terminal state that the command itself requests and made
/// no new transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PayrollRunTransition {
    pub status: PayrollRunStatus,
    pub idempotent: bool,
}

/// Fail closed unless an existing run has an exact org/month attendance close,
/// its active payroll period lock, and no unresolved attendance exceptions.
///
/// It permits only the approval transition: `Calculate` and `MarkIssued` return
/// `Conflict` unconditionally.
///
/// # NOTHING IN PRODUCTION CALLS THIS
///
/// Read the previous sentence as a statement about this function only, never
/// about the system. This guard has **no non-test caller**: every call site is
/// inside this file's `#[cfg(test)] mod tests`. The doc comment here previously
/// said it was "intentionally reusable by every persistence/transport adapter"
/// and that calculation and issuance "remain blocked" — and both readings were
/// false at the system level:
///
/// - **Calculation is not blocked.** It ships and works, through
///   `console_payroll_adapter_postgres::lifecycle::calculate_run_in_tx`, a
///   parallel path added later that never consults this function. That path
///   gates on a raw string compare, `run.status != "ATTENDANCE_CLOSED"`, as do
///   the submit, decide, schedule-disbursement and attest steps.
/// - **Issuance is not gated on step-up.** No step-up mechanism exists anywhere
///   in the payroll crates; the only occurrence of the phrase in the whole
///   payroll tree is a test name below. The mechanism does exist elsewhere
///   (`verify_step_up` in the inbox, financial and identity REST crates) and is
///   simply not wired here.
///
/// So this is a pure FSM kept for its shape, not an enforced boundary. Wiring
/// production through it — or deleting it — is a decision this comment does not
/// make. What it must not do is let a reader mistake it for the guard that
/// blocks payment, because the release gate is consulted in exactly one place
/// (`load_payslip_issuance_in_tx`, after `status == "PAID"`), which withholds
/// the 임금명세서 and not the money.
pub fn transition_payroll_run(
    current: PayrollRunStatus,
    command: PayrollRunCommand,
    prerequisites: PayrollClosePrerequisites,
) -> Result<PayrollRunTransition, KernelError> {
    // A confirmed prior transition is a retry-safe no-op. In particular, a
    // later attendance correction or lock amendment must not turn a network
    // retry into a second mutation or a contradictory failure.
    if let (PayrollRunStatus::Approved, PayrollRunCommand::Approve { .. }) = (current, command) {
        return Ok(PayrollRunTransition {
            status: current,
            idempotent: true,
        });
    }
    validate_close_prerequisites(prerequisites)?;

    let (status, idempotent) = match command {
        PayrollRunCommand::Calculate => {
            return Err(KernelError::conflict(
                "payroll calculation is blocked until immutable validated release-gate evidence is persisted",
            ));
        }
        PayrollRunCommand::Approve {
            approver_is_creator,
        } => match current {
            PayrollRunStatus::ReadyForReview => {
                if approver_is_creator {
                    return Err(KernelError::forbidden(
                        "payroll run creator cannot approve the same run",
                    ));
                }
                (PayrollRunStatus::Approved, false)
            }
            PayrollRunStatus::Approved => unreachable!("handled as retry above"),
            PayrollRunStatus::Staged
            | PayrollRunStatus::BlockedLegalGate
            | PayrollRunStatus::Issued
            | PayrollRunStatus::Void => {
                return Err(KernelError::invalid_transition(
                    "payroll approval requires a run ready for review",
                ));
            }
        },
        PayrollRunCommand::MarkIssued => {
            return Err(KernelError::conflict(
                "payroll issuance is blocked until step-up authorization, audit evidence, and an immutable issuance artifact are persisted",
            ));
        }
    };
    Ok(PayrollRunTransition { status, idempotent })
}

fn validate_close_prerequisites(
    prerequisites: PayrollClosePrerequisites,
) -> Result<(), KernelError> {
    if !is_exact_calendar_month(prerequisites.period_start, prerequisites.period_end) {
        return Err(KernelError::validation(
            "payroll run period must be one exact calendar month",
        ));
    }
    if !prerequisites.org_month_close_present {
        return Err(KernelError::conflict(
            "payroll run requires an org-level attendance close for its exact month",
        ));
    }
    if !prerequisites.active_exact_payroll_lock_present {
        return Err(KernelError::conflict(
            "payroll run requires an active payroll period lock for its exact month",
        ));
    }
    if prerequisites.unresolved_attendance_exception_count != 0 {
        return Err(KernelError::conflict(
            "payroll run is blocked by unresolved attendance exceptions",
        ));
    }
    Ok(())
}

fn is_exact_calendar_month(period_start: Date, period_end: Date) -> bool {
    if period_start.day() != 1 {
        return false;
    }
    let Some(day_after_end) = period_end.next_day() else {
        return false;
    };
    if day_after_end.day() != 1 {
        return false;
    }
    match period_start.month() {
        time::Month::January => {
            day_after_end.year() == period_start.year()
                && day_after_end.month() == time::Month::February
        }
        time::Month::February => {
            day_after_end.year() == period_start.year()
                && day_after_end.month() == time::Month::March
        }
        time::Month::March => {
            day_after_end.year() == period_start.year()
                && day_after_end.month() == time::Month::April
        }
        time::Month::April => {
            day_after_end.year() == period_start.year() && day_after_end.month() == time::Month::May
        }
        time::Month::May => {
            day_after_end.year() == period_start.year()
                && day_after_end.month() == time::Month::June
        }
        time::Month::June => {
            day_after_end.year() == period_start.year()
                && day_after_end.month() == time::Month::July
        }
        time::Month::July => {
            day_after_end.year() == period_start.year()
                && day_after_end.month() == time::Month::August
        }
        time::Month::August => {
            day_after_end.year() == period_start.year()
                && day_after_end.month() == time::Month::September
        }
        time::Month::September => {
            day_after_end.year() == period_start.year()
                && day_after_end.month() == time::Month::October
        }
        time::Month::October => {
            day_after_end.year() == period_start.year()
                && day_after_end.month() == time::Month::November
        }
        time::Month::November => {
            day_after_end.year() == period_start.year()
                && day_after_end.month() == time::Month::December
        }
        time::Month::December => {
            day_after_end.year() == period_start.year() + 1
                && day_after_end.month() == time::Month::January
        }
    }
}

#[must_use]
pub const fn payroll_sources_verified_on() -> Date {
    date!(2026 - 06 - 27)
}

#[must_use]
pub const fn moel_retirement_pay_source() -> OfficialSource {
    OfficialSource {
        authority: "Ministry of Employment and Labor",
        title: "Retirement pay average wage formula",
        url: "https://www.moel.go.kr/faq/faqView.do?seqRepeat=89",
        retrieved_on: date!(2026 - 07 - 03),
    }
}

#[must_use]
pub const fn nhis_qualification_loss_form_source() -> OfficialSource {
    OfficialSource {
        authority: "National Health Insurance Service",
        title: "4-insurance workplace subscriber qualification loss report",
        url: "https://www.nhis.or.kr/static/html/wbdb/f/wbdbf0201.html",
        retrieved_on: date!(2026 - 07 - 03),
    }
}

#[must_use]
pub const fn nps_source() -> OfficialSource {
    OfficialSource {
        authority: "국민연금공단",
        title: "사업장가입자 보험료율 및 기준소득월액 상·하한액",
        url: "https://www.nps.or.kr/pnsinfo/ntpsklg/getOHAF0038M0.do",
        retrieved_on: payroll_sources_verified_on(),
    }
}

#[must_use]
pub const fn nhis_source() -> OfficialSource {
    OfficialSource {
        authority: "국민건강보험공단",
        title: "2026년도 보험료율 인상 안내",
        url: "https://edi.nhis.or.kr/portal/images/popup/20251204_pop01longdesc.html",
        retrieved_on: payroll_sources_verified_on(),
    }
}

#[must_use]
pub const fn nts_source() -> OfficialSource {
    OfficialSource {
        authority: "국세청",
        title: "근로소득 원천징수방법(간이세액표)",
        url: "https://www.nts.go.kr/nts/cm/cntnts/cntntsView.do?cntntsId=7862&mi=6583",
        retrieved_on: payroll_sources_verified_on(),
    }
}

#[must_use]
pub const fn minimum_wage_source() -> OfficialSource {
    OfficialSource {
        authority: "최저임금위원회",
        title: "연도별 최저임금 결정현황",
        url: "https://www.minimumwage.go.kr/minWage/policy/decisionMain.do",
        retrieved_on: payroll_sources_verified_on(),
    }
}

/// The date this crate's statutory instruments were last retrieved from
/// law.go.kr. Every `Instrument` below carries it.
///
/// # `조문시행일자` is not a second witness
///
/// The citations below used to quote a `조문시행일자` beside the law-level
/// `efYd`/시행일자, as if the two corroborated each other. They are **one**
/// observation, not two: a `target=eflaw` response for a pinned `efYd` reports
/// that same date back on every 조문단위 it contains, so both numbers come from
/// one fetch of one document. The pairing is gone from every citation in this
/// crate. Measured directly — 지방세법 MST 282559 제103조의13 is byte-identical
/// across efYd 20260101, 20260424, 20260701 and 20270101, while `조문시행일자`
/// dutifully reports each of those four dates in turn. It records which slice
/// was asked for; it corroborates nothing about the slice. The load-bearing
/// evidence is the pinned `efYd` plus the article text.
///
/// The digest backing that byte-identity claim, and the normalization needed to
/// reproduce it, are recorded once — on `local_income_tax_instrument`. It is
/// deliberately not repeated here: two hand-maintained copies of a number is the
/// defect this round is closing, not one to introduce.
#[must_use]
pub const fn statutory_instruments_retrieved_on() -> Date {
    date!(2026 - 08 - 01)
}

/// The one unresolved rounding question that reaches an amount. 국민건강보험법
/// 제76조제1항 says `100분의 50씩`; 제107조 truncates 끝수 on `보험료등`. Which
/// unit governs the HALF is settled by no text read. It needs NHIS practice or
/// counsel; until then the agreement gate emits only when both candidates agree.
pub const QUESTION_HALF_SHARE_ROUNDING: &str = "Q-HALF-SHARE-ROUNDING-UNIT";

const HALF_SHARE_ROUNDING: Rounding = Rounding::Unresolved {
    candidates: [RoundingUnit::Trunc10, RoundingUnit::Round10],
    question_id: QUESTION_HALF_SHARE_ROUNDING,
};

/// The negative search result the 고용보험·산재 rounding rests on, re-run
/// 2026-08-01 against both in-force texts: 단수 / 끝수 / 국고금 occur **zero**
/// times in 징수법 (법령ID 009589, MST 247481, efYd 20240101, 101개 조문단위), and
/// its 시행령 (법령ID 009842, MST 280527, efYd 20251223, 118개 조문단위) has exactly
/// one 국고금 hit — **제41조의5**, not 제41조, and its 제2항제1호, verbatim:
///
/// > 1. 「국가를 당사자로 하는 계약에 관한 법률」 제2조에 따른 계약. 다만,
/// >    「국고금 관리법 시행령」 제31조에 따른 관서운영경비로 그 대가를
/// >    지급받는 계약은 제외한다.
///
/// That is 제41조의5(보험료등의 완납증명이 필요한 경우 등) carving 관서운영경비
/// contracts out of the 완납증명 duty. It cites 「국고금 관리법 시행령」 제31조,
/// not 국고금관리법 제47조, and prescribes nothing about 단수. (The neighbouring
/// 제2호 is the 「지방회계법 시행령」 one — a different 호 that contains no 국고금
/// at all, which an earlier revision of this comment conflated with the hit.)
///
/// This is what `Rounding::Assumed` records. 국민건강보험법 제107조 truncates
/// 「보험료등」 under **국민건강보험법**; 징수법 provides no 준용 bridge to it, so
/// citing it for a 징수법 premium would be a false citation, not a conservative
/// one.
const NO_FRACTION_RULE_IN_COLLECTION_ACT: &str = "단수 규정 미발견 — 징수법 본문(MST 247481, efYd 20240101)과 같은 법 시행령(MST 280527, efYd 20251223) 어디에도 단수·끝수 조문이 없고 국고금관리법을 준용하는 조항도 없다. 시행령의 유일한 국고금 언급은 제41조의5제2항제1호가 「국고금 관리법 시행령」 제31조의 관서운영경비 계약을 완납증명 대상에서 제외한 것으로, 단수와 무관하다. 국민건강보험법 제107조는 같은 법 「보험료등」에만 미치므로 징수법 보험료에 준용할 근거가 없다. 절사 없는 정확분은 확인된 규칙이 아니라 공개된 가정이다(오차 <10원).";

/// 국민건강보험법 제107조 → 국고금관리법 제47조제1항.
///
/// Anchored to the slice in force at the rows it governs (효력 2026-01-01):
/// 법령ID 001971, MST 265877, **efYd 20250423** — one pinned fetch, one anchor
/// (see `statutory_instruments_retrieved_on` on why the response's own
/// `조문시행일자` is not a second one). The earlier
/// anchor — 법률 제21065호 / 2026-01-02 — was the NEXT slice, enforced a day
/// AFTER every row citing it began; it escaped
/// `no_rate_row_is_in_force_before_the_instrument_that_sets_it` only because
/// that test read `rate.instrument` and never the rounding instrument.
#[must_use]
pub const fn national_treasury_fraction_instrument() -> Instrument {
    Instrument {
        name_ko: "국민건강보험법",
        article_ko: "제107조 (「국고금관리법」 제47조에 따른 끝수는 계산하지 아니한다) → 국고금관리법 제47조제1항 (10원 미만의 단수는 계산하지 아니한다)",
        promulgation_ko: "법률 제20505호",
        enforced_on: date!(2025 - 04 - 23),
        url: "https://www.law.go.kr/법령/국민건강보험법",
        retrieved_on: statutory_instruments_retrieved_on(),
    }
}

/// 국민연금법 제117조 → 국고금 관리법 제47조제1항 — the bridge an earlier round
/// recorded as **absent**, and which the 10원 절사 for 연금보험료 rests on.
///
/// Read via `target=eflaw` at the date the rows citing it begin: 법령ID 001781,
/// MST 280269, **efYd 20260101** (공포번호 법률 제21203호, 공포일자 2025-12-16).
/// The evidence is that the pinned 20260101 fetch returns 제117조's text; the
/// `조문시행일자` it prints alongside is the same response echoing the same date,
/// not a second witness. "No rule was located" was a statement about the search,
/// not about the statute book; it is now a located rule and the 절사 is no longer
/// a disclosed assumption.
///
/// The earlier anchor 2026-06-17 was the trap: **MST 280269 carries two 시행일자
/// slices, 20260101 and 20260617**, and any fetch that does not pin `efYd`
/// returns the later one. 제117조 is present verbatim in the 20260101 slice, so
/// the rule is in force on the row's own `effective_from`.
#[must_use]
pub const fn national_pension_fraction_instrument() -> Instrument {
    Instrument {
        name_ko: "국민연금법",
        article_ko: "제117조(단수의 처리) 「이 법에 따른 급여ㆍ연금보험료ㆍ반환금 등을 계산할 때 그 금액에 10원 미만의 단수(端數)가 있으면 「국고금관리법」을 준용하여 계산한다」 → 국고금 관리법 제47조제1항 「… 10원 미만의 끝수가 있을 때에는 그 끝수는 계산하지 아니하고 …」",
        promulgation_ko: "법률 제21203호",
        enforced_on: date!(2026 - 01 - 01),
        url: "https://www.law.go.kr/법령/국민연금법",
        retrieved_on: statutory_instruments_retrieved_on(),
    }
}

#[must_use]
pub const fn national_pension_rate_instrument() -> Instrument {
    Instrument {
        name_ko: "국민연금법 부칙 <법률 제20903호, 2025. 4. 2.>",
        article_ko: "제4조제1항제1호 (기준소득월액의 1만분의 475) — 본칙 제88조제3항의 1천분의 65를 2026년에 대해 대체",
        promulgation_ko: "법률 제20903호",
        enforced_on: date!(2026 - 01 - 01),
        url: "https://www.law.go.kr/법령/국민연금법",
        retrieved_on: statutory_instruments_retrieved_on(),
    }
}

#[must_use]
pub const fn employment_insurance_rate_instrument() -> Instrument {
    Instrument {
        name_ko: "고용보험 및 산업재해보상보험의 보험료징수 등에 관한 법률 시행령",
        article_ko: "제12조제1항제2호 (실업급여 1천분의 18) — 법 제13조제2항에 따라 근로자가 2분의 1 부담",
        promulgation_ko: "대통령령 제35935호",
        enforced_on: date!(2025 - 12 - 23),
        url: "https://www.law.go.kr/법령/고용보험및산업재해보상보험의보험료징수등에관한법률시행령",
        retrieved_on: statutory_instruments_retrieved_on(),
    }
}

/// Same slice, same correction as `national_treasury_fraction_instrument`:
/// 법령ID 001971, MST 265877, efYd 20250423 (one pinned fetch, one anchor).
#[must_use]
pub const fn fifty_fifty_share_instrument() -> Instrument {
    Instrument {
        name_ko: "국민건강보험법",
        article_ko: "제76조제1항 (직장가입자와 사업주가 각각 보험료액의 100분의 50씩 부담한다)",
        promulgation_ko: "법률 제20505호",
        enforced_on: date!(2025 - 04 - 23),
        url: "https://www.law.go.kr/법령/국민건강보험법",
        retrieved_on: statutory_instruments_retrieved_on(),
    }
}

/// 장기요양 does not reach 국민건강보험법 directly — it gets there by 준용, and
/// **the bridge is part of the citation**.
///
/// The two functions below exist because naming only the destination
/// (국민건강보험법 제107조 / 제76조제1항) makes the citation checkable only by a
/// reader who already knows the chain — 국민건강보험법 says nothing about
/// 장기요양보험료 on its own face. 노인장기요양보험법 supplies the chain in two
/// different articles, both read at the slice in force when the LTC rows begin:
/// 법령ID 010436, MST 281921, **efYd 20251230**, 법률 제21257호.
///
/// Both are anchored at that earliest in-force slice, which is the F1 rule, and
/// each was checked against the later slice the 2026-11-27 row runs under
/// (MST 286217, efYd 20261127, 법률 제21690호):
///
/// * 제11조 — rendered text **byte-identical** in both. (`조문시행일자` moves
///   20251230 → 20261127 while the article does not; see
///   `statutory_instruments_retrieved_on`.)
/// * 제64조 — **amended** by 법률 제21690호, 시행 2026-11-27: 제91조의2 and
///   「보험료 등 부과의 제척기간」 are inserted into the list. 제107조 and
///   「단수처리 … 준용한다」 survive verbatim, which is the only part cited here,
///   so the fragment quoted below is present in both slices.
#[must_use]
pub const fn long_term_care_fraction_instrument() -> Instrument {
    Instrument {
        name_ko: "노인장기요양보험법",
        article_ko: "제64조(시효 등에 관한 준용) 「「국민건강보험법」 … 제107조, 제111조 및 제112조는 … 단수처리 등에 관하여 준용한다. 이 경우 \"보험료\"를 \"장기요양보험료\"로 … 본다」 → 국민건강보험법 제107조 → 국고금관리법 제47조제1항 (10원 미만의 단수는 계산하지 아니한다)",
        promulgation_ko: "법률 제21257호",
        enforced_on: date!(2025 - 12 - 30),
        url: "https://www.law.go.kr/법령/노인장기요양보험법",
        retrieved_on: statutory_instruments_retrieved_on(),
    }
}

/// The 50/50 half of the same bridge — 제11조, a different 준용 article from the
/// 단수 one above, and the reason 「제107조 준용」 cannot be made to carry both.
#[must_use]
pub const fn long_term_care_half_share_instrument() -> Instrument {
    Instrument {
        name_ko: "노인장기요양보험법",
        article_ko: "제11조(장기요양보험가입 자격 등에 관한 준용) 「「국민건강보험법」 제5조, 제6조, 제8조부터 제11조까지, 제69조제1항부터 제3항까지, 제76조부터 제86조까지, 제109조제1항부터 제9항까지 및 제110조는 장기요양보험가입자ㆍ피부양자의 자격취득ㆍ상실, 장기요양보험료 및 그 밖의 이 법에 따른 징수금(이하 \"장기요양보험료등\"이라 한다)의 납부ㆍ징수 및 결손처분 등에 관하여 이를 준용한다」 → 국민건강보험법 제76조제1항 (직장가입자와 사업주가 각각 보험료액의 100분의 50씩 부담한다)",
        promulgation_ko: "법률 제21257호",
        enforced_on: date!(2025 - 12 - 30),
        url: "https://www.law.go.kr/법령/노인장기요양보험법",
        retrieved_on: statutory_instruments_retrieved_on(),
    }
}

#[must_use]
pub const fn health_premium_clamp() -> MonthlyPremiumClamp {
    MonthlyPremiumClamp {
        floor_won: 20_160,
        cap_won: 9_183_480,
        instrument: Instrument {
            name_ko: "월별 건강보험료액의 상한과 하한에 관한 고시",
            article_ko: "직장가입자의 보수월액보험료 — 상한 9,183,480원 / 하한 20,160원",
            promulgation_ko: "보건복지부고시 제2025-222호 (발령 2025. 12. 24.)",
            enforced_on: date!(2026 - 01 - 01),
            url: "https://www.law.go.kr/LSW/admRulInfoR.do?admRulSeq=2100000270472",
            retrieved_on: statutory_instruments_retrieved_on(),
        },
    }
}

/// The 2026 statutory rate table, instrument-level and effective-dated.
///
/// **2026 only, on purpose.** 국민연금's employee share is a legislated
/// 2026→2032 ramp that is citable today, and encoding all eight rows would
/// remove seven future refresh events — but it would also let the engine sail
/// through each January on a rate nobody re-reviewed. The jurisdiction
/// register's `change_rule` requires a new candidate-bound review on any
/// effective-date change regardless, so failing closed is the cheaper error.
#[must_use]
pub fn statutory_contribution_rates() -> Vec<StatutoryRate> {
    vec![
        StatutoryRate {
            code: ContributionCode::NationalPension,
            period: EffectivePeriod::new(date!(2026 - 01 - 01), Some(date!(2027 - 01 - 01))),
            basis: ContributionBasis::MonthlyStandardIncome,
            // 1만분의 475 = 4.75%, the employee share directly: there is no
            // total-then-halve step for 국민연금.
            rate_num: 475,
            rate_den: 10_000,
            // 단수 rule: LOCATED. 국민연금법 제117조 준용s 국고금관리법 for
            // 연금보험료, exactly as 국민건강보험법 제107조 does for 건강보험.
            // An earlier round recorded this bridge as absent and emitted the
            // unrounded product as a disclosed assumption; the article exists
            // and is in force, so the assumption is retired.
            total_rounding: Rounding::Resolved {
                unit: RoundingUnit::Trunc10,
                instrument: national_pension_fraction_instrument(),
            },
            employee_share: ShareRule::WholeOfRate,
            clamp: None,
            instrument: national_pension_rate_instrument(),
            share_instrument: None,
            provenance: "레지스터 §1 항목 1에서 부칙 조문 전문을 인용(시행 중인 파일에서 읽음, HIGH). 연간 고시가 아니라 법률 부칙의 2026→2032 법정 스케줄. 10원 미만 절사의 근거는 국민연금법 제117조(단수의 처리)가 국고금관리법을 준용하는 데 있다 — 법령ID 001781, MST 280269, efYd 20260101(법률 제21203호)로 슬라이스를 고정해 조문 전문을 읽었다. 종전의 「규정 미발견에 따른 공개 가정」은 철회. 종전 인용 2026-06-17은 같은 MST의 나중 슬라이스였다: MST 280269는 20260101·20260617 두 시행일을 가지며 efYd를 고정하지 않으면 나중 것이 온다. 응답에 함께 실리는 조문시행일자는 요청한 슬라이스를 되받는 값이므로 별개의 증거가 아니다.",
        },
        StatutoryRate {
            code: ContributionCode::HealthInsurance,
            period: EffectivePeriod::new(date!(2026 - 01 - 01), Some(date!(2027 - 01 - 01))),
            basis: ContributionBasis::MonthlyRemuneration,
            rate_num: 719,
            rate_den: 10_000,
            total_rounding: Rounding::Resolved {
                unit: RoundingUnit::Trunc10,
                instrument: national_treasury_fraction_instrument(),
            },
            employee_share: ShareRule::Half {
                rounding: HALF_SHARE_ROUNDING,
            },
            clamp: Some(health_premium_clamp()),
            instrument: Instrument {
                name_ko: "국민건강보험법 시행령",
                article_ko: "제44조제1항 「법 제73조제1항에 따른 직장가입자의 보험료율 … 은 각각 1만분의 719로 한다」 <개정 2025.12.23>",
                // The row is in force from 2026-01-01, so its instrument must be
                // the one in force THEN. 제36116호 (시행 2026-02-19) did not touch
                // 제44조 — its 개정 history ends 2025.12.23 — and citing it dated
                // the rate seven weeks after the row it justifies.
                promulgation_ko: "대통령령 제35931호",
                enforced_on: date!(2026 - 01 - 01),
                url: "https://www.law.go.kr/법령/국민건강보험법시행령",
                retrieved_on: statutory_instruments_retrieved_on(),
            },
            share_instrument: Some(fifty_fifty_share_instrument()),
            provenance: "요율은 2026-01-01 시행본을 eflaw로 직접 읽음 (법령ID 002813, MST 280453, 대통령령 제35931호) — 제44조제1항의 최종 개정은 2025.12.23이며 제36116호(시행 2026-02-19)는 이 조문을 건드리지 않았다. 종전 인용 「제36116호, 시행 2026-02-19」은 이 행의 effective_from(2026-01-01)보다 7주 늦은 문서였다. 50/50 분담(제76조제1항)과 10원 절사(제107조)는 이 행의 effective_from에 시행 중인 슬라이스에서 읽음 — MST 265877, efYd 20250423, 법률 제20505호. 종전 인용 「법률 제21065호」는 시행 2026-01-02로 이 행보다 하루 늦은 슬라이스였다. 총액은 10원 절사 후 상·하한 고시로 클램프하며, 절반 분담의 반올림 단위는 미해결(Q-HALF-SHARE-ROUNDING-UNIT).",
        },
        // 장기요양's basis is the 건강보험료액, and its ratio is EXACT
        // (9,448/71,900) until 제9조제1항's 반올림 clause enters force.
        StatutoryRate {
            code: ContributionCode::LongTermCare,
            period: EffectivePeriod::new(date!(2026 - 01 - 01), Some(date!(2026 - 11 - 27))),
            basis: ContributionBasis::HealthInsurancePremium,
            rate_num: 9_448,
            rate_den: 71_900,
            // The 준용 article, not the destination: 장기요양 reaches 제107조 only
            // through 노인장기요양보험법 제64조, and the destination alone is a
            // citation only a reader who already knows the chain can check.
            total_rounding: Rounding::Resolved {
                unit: RoundingUnit::Trunc10,
                instrument: long_term_care_fraction_instrument(),
            },
            employee_share: ShareRule::Half {
                rounding: HALF_SHARE_ROUNDING,
            },
            clamp: None,
            instrument: Instrument {
                name_ko: "노인장기요양보험법 시행령",
                article_ko: "제4조(장기요양보험료율) 「법 제9조제1항에 따른 장기요양보험료율은 100만분의 9,448로 한다」 <개정 2025.12.30>",
                // Same defect as 건강보험 above: 제36325호 is enforced 2026-05-12,
                // four months after this row starts. 제4조's own 개정 ends
                // 2025.12.30 — 제35987호.
                promulgation_ko: "대통령령 제35987호",
                enforced_on: date!(2026 - 01 - 01),
                url: "https://www.law.go.kr/법령/노인장기요양보험법시행령",
                retrieved_on: statutory_instruments_retrieved_on(),
            },
            share_instrument: Some(long_term_care_half_share_instrument()),
            provenance: "요율은 2026-01-01 시행본을 eflaw로 직접 읽음 (법령ID 010526, MST 281843, 대통령령 제35987호) — 제4조의 최종 개정은 2025.12.30이다. 종전 인용 「제36325호, 시행 2026-05-12」은 이 행의 effective_from보다 넉 달 늦은 문서였다. 산정기초는 보수월액이 아니라 건강보험료액. 제9조제1항 2026-05-26 시행본에는 「소수점 이하 다섯째자리에서 반올림」 문구가 없으므로 비율은 절사·반올림 없이 정확분수 — 그 문구는 법률 제21690호의 2026-11-27 시행분에서 삽입된다. 절사와 50/50 분담은 국민건강보험법을 직접 인용하지 않고 준용 조문을 인용한다: 노인장기요양보험법 제64조(→ 법 제107조), 제11조(→ 법 제76조제1항). 둘 다 법령ID 010436, MST 281921, efYd 20251230, 법률 제21257호에서 읽음.",
        },
        StatutoryRate {
            code: ContributionCode::LongTermCare,
            period: EffectivePeriod::new(date!(2026 - 11 - 27), Some(date!(2027 - 01 - 01))),
            basis: ContributionBasis::HealthInsurancePremium,
            // 0.009448 / 0.0719 = 0.1314047287899… → 0.1314 at 4 dp.
            rate_num: 1_314,
            rate_den: 10_000,
            total_rounding: Rounding::Resolved {
                unit: RoundingUnit::Trunc10,
                instrument: long_term_care_fraction_instrument(),
            },
            employee_share: ShareRule::Half {
                rounding: HALF_SHARE_ROUNDING,
            },
            clamp: None,
            instrument: Instrument {
                name_ko: "노인장기요양보험법",
                article_ko: "제9조제1항 (… 비율을 곱하여 산정 <소수점 이하 다섯째자리에서 반올림한다>)",
                promulgation_ko: "법률 제21690호",
                enforced_on: date!(2026 - 11 - 27),
                url: "https://www.law.go.kr/법령/노인장기요양보험법",
                retrieved_on: statutory_instruments_retrieved_on(),
            },
            share_instrument: Some(long_term_care_half_share_instrument()),
            provenance: "동일 공포번호(제21690호)의 두 시행일 슬라이스 중 나중 것. target=law은 이 텍스트를 오늘 이미 반환하므로, 시행일 기준 선택이 없으면 4개월 앞당겨 적용되는 오류가 난다. 절사·분담은 준용 조문(제64조·제11조)을 인용하며, 이 행이 도는 20261127 슬라이스(MST 286217, 법률 제21690호)에서 제11조는 자구까지 동일하고 제64조는 제91조의2·제척기간이 추가되었을 뿐 「제107조 … 단수처리 … 준용한다」는 그대로다.",
        },
        StatutoryRate {
            code: ContributionCode::EmploymentUnemployment,
            period: EffectivePeriod::new(date!(2026 - 01 - 01), Some(date!(2027 - 01 - 01))),
            basis: ContributionBasis::MonthlyRemuneration,
            // 1천분의 18 total, employee bears ½ → 9/1,000.
            rate_num: 9,
            rate_den: 1_000,
            // 단수 rule: STILL NONE LOCATED, and no longer "same as 국민연금" —
            // 국민연금 has 제117조. `Assumed`, not `Resolved`: 시행령
            // 제12조제1항제2호 sets a RATE and prescribes no 단수 rule, so citing
            // it as the instrument that settles the rounding was a citation
            // that did not match what it cited.
            total_rounding: Rounding::Assumed {
                unit: RoundingUnit::ExactWon,
                note_ko: NO_FRACTION_RULE_IN_COLLECTION_ACT,
            },
            employee_share: ShareRule::WholeOfRate,
            clamp: None,
            instrument: employment_insurance_rate_instrument(),
            share_instrument: None,
            provenance: "레지스터 §1 항목 5. 근로자 ½ 부담은 2026-08-01 시행본(법률 제19209호, MST 247481, efYd 20240101)에서 제13조제2항 전문을 직접 읽어 확인 — 「고용보험 가입자인 근로자가 부담하여야 하는 고용보험료는 자기의 보수총액에 제14조제1항에 따른 실업급여의 보험료율의 2분의 1을 곱한 금액으로 한다」. 레지스터의 MEDIUM은 해소됨. 단수는 Rounding::Assumed — 국민연금과 달리 절사 근거 조문이 없다. 시행령 제12조제1항제2호는 요율만 정하며 단수를 정하지 않으므로 그것을 절사 근거로 인용하지 않는다. 2026-08-01 재확인: 단수·끝수는 징수법 본문(MST 247481, efYd 20240101)과 시행령(MST 280527, efYd 20251223) 양쪽 모두 0건이고, 국고금은 시행령 제41조의5제2항제1호의 완납증명 예외(「국고금 관리법 시행령」 제31조 관서운영경비) 한 건뿐이라 단수와 무관하다. 절사 근거 없는 공개 가정으로 남는 것은 여기와 산재뿐이다(오차 <10원).",
        },
        StatutoryRate {
            code: ContributionCode::IndustrialAccident,
            period: EffectivePeriod::new(date!(2026 - 01 - 01), Some(date!(2027 - 01 - 01))),
            basis: ContributionBasis::IndustryTariff,
            rate_num: 0,
            rate_den: 1,
            // 국민건강보험법 제107조 has no application to 산재보험료: it is not
            // reached from 징수법 by any 준용, and 징수법 itself has no 단수
            // article. The earlier citation of it here was a statute that does
            // not govern this row.
            total_rounding: Rounding::Assumed {
                unit: RoundingUnit::ExactWon,
                note_ko: NO_FRACTION_RULE_IN_COLLECTION_ACT,
            },
            employee_share: ShareRule::EmployerOnly,
            clamp: None,
            instrument: Instrument {
                name_ko: "고용보험 및 산업재해보상보험의 보험료징수 등에 관한 법률",
                // 제13조제5항 본문 전문. 「사업주가 전액 부담한다」는 조문에 없는
                // 표현이므로 인용하지 않는다 — 근로자 공제가 없다는 결론은
                // 제13조제5항이 산재보험료를 사업주 부담으로만 산정하고
                // 제13조제2항이 근로자에게 고용보험료만 지우는 데서 나온다.
                article_ko: "제13조제5항 「제1항에 따라 사업주가 부담하여야 하는 산재보험료는 그 사업주가 경영하는 사업에 종사하는 근로자의 개인별 보수총액에 다음 각 호에 따른 산재보험료율을 곱한 금액을 합한 금액으로 한다」 — 근로자 부담분을 정한 항이 없다. 요율은 고용노동부고시 제2025-91호 별지, 유효기간 2026-12-31",
                promulgation_ko: "법률 제19209호",
                enforced_on: date!(2024 - 01 - 01),
                url: "https://www.law.go.kr/법령/고용보험및산업재해보상보험의보험료징수등에관한법률",
                retrieved_on: statutory_instruments_retrieved_on(),
            },
            share_instrument: None,
            provenance: "rate_num=0은 근로자 요율이며(징수법에 근로자 산재 부담 조항 없음) 사업주 부담액이 아니다 — 사업주 총액은 별지 미파싱으로 미상이며 커널은 이를 0이 아닌 NULL로 낸다. 2026-08-01 시행본을 eflaw 2단계로 확정하여 조문 전문을 읽음 (MST 247481, efYd 20240101). 이 법령명한글로 target=eflaw lawSearch를 display=100으로 1·2면 모두 열거하면(totalCnt 174) 정확히 일치하는 행이 56건이고 그중 시행예정은 20261008·20270101 둘뿐이다 — 종전에 적힌 「58개 슬라이스」는 재현되지 않아 실측치로 대체한다(56행 = 서로 다른 시행일자 48개, 서로 다른 MST 38개). 종전 인용 「법률 제21532호, 시행 2026-10-08」은 장래효 슬라이스였고, 종전 article_ko는 인용이 아니라 의역이었다 — 둘 다 정정. 단수는 Rounding::Assumed — 종전에는 국민건강보험법 제107조를 인용했으나 그 조문은 같은 법 「보험료등」에만 미치고 징수법이 이를 준용하는 조항이 없어, 산재보험료에 적용되지 않는 법률을 인용한 것이었다.",
        },
    ]
}

#[must_use]
pub fn national_pension_base_limits() -> Vec<MonthlyBaseLimit> {
    vec![
        MonthlyBaseLimit {
            period: EffectivePeriod::new(date!(2025 - 07 - 01), Some(date!(2026 - 07 - 01))),
            minimum_won: 400_000,
            maximum_won: 6_370_000,
            instrument: Instrument {
                name_ko: "국민연금 기준소득월액 하한액과 상한액",
                article_ko: "가. 하한액 400천원 / 나. 상한액 6,370천원 · 적용기간 2025년도 7월분부터 2026년도 6월분까지",
                promulgation_ko: "보건복지부고시 제2025-24호",
                enforced_on: date!(2025 - 07 - 01),
                url: "https://www.law.go.kr/LSW/admRulInfoR.do?admRulSeq=2100000254486",
                retrieved_on: statutory_instruments_retrieved_on(),
            },
        },
        MonthlyBaseLimit {
            period: EffectivePeriod::new(date!(2026 - 07 - 01), Some(date!(2027 - 07 - 01))),
            minimum_won: 410_000,
            maximum_won: 6_590_000,
            instrument: Instrument {
                name_ko: "국민연금 기준소득월액 하한액과 상한액",
                article_ko: "가. 하한액 410천원 / 나. 상한액 6,590천원 · 적용기간 2026년도 7월분부터 2027년도 6월분까지",
                promulgation_ko: "보건복지부고시 제2026-31호 (발령 2026. 2. 2.)",
                enforced_on: date!(2026 - 07 - 01),
                url: "https://www.law.go.kr/LSW/admRulInfoR.do?admRulSeq=2100000274228",
                retrieved_on: statutory_instruments_retrieved_on(),
            },
        },
    ]
}

#[must_use]
pub fn minimum_wage_rates() -> Vec<MinimumWageRate> {
    vec![MinimumWageRate {
        period: EffectivePeriod::new(date!(2026 - 01 - 01), Some(date!(2027 - 01 - 01))),
        hourly_won: 10_320,
        daily_8h_won: 82_560,
        monthly_209h_won: 2_156_880,
        instrument: Instrument {
            name_ko: "2026년 적용 최저임금 고시",
            article_ko: "1. 모든 산업 시간급 10,320원 · 월환산액 2,156,880원(209시간) · 2. 사업의 종류별 구분 없이 모든 사업장에 동일하게 적용 · 3. 적용기간 2026.1.1.~2026.12.31.",
            promulgation_ko: "고용노동부고시 제2025-47호 (발령 2025. 8. 5.)",
            enforced_on: date!(2026 - 01 - 01),
            url: "https://www.law.go.kr/행정규칙/2026년 적용 최저임금 고시",
            retrieved_on: statutory_instruments_retrieved_on(),
        },
    }]
}

/// 소득세법 시행령 별표 2 — the instrument step 1 refuses to guess at.
///
/// FOUR rows for one calendar year, because the 별표 carries its OWN 시행일자,
/// separate from the decree's, and the version anchor is the semantic
/// 별표HWP파일명 rather than the unstable `flSeq` handle.
///
/// The 2026-02-27 boundary is the dangerous one. Across it the 646 × 11 bracket
/// grid is BYTE-IDENTICAL; only note 3's 자녀세액공제 changed (1명 12,500 →
/// 20,830원). An engine that ingests the grid and hard-codes the credits
/// withholds 8,330원/month too much for a one-child employee — and gets a
/// January 2026 recomputation wrong in the other direction. Watch the whole
/// 별표, not the grid.
#[must_use]
pub fn simplified_withholding_table_instruments() -> Vec<(EffectivePeriod, Instrument)> {
    vec![
        (
            EffectivePeriod::new(date!(2026 - 01 - 02), Some(date!(2026 - 02 - 27))),
            Instrument {
                name_ko: "소득세법 시행령 별표 2 (근로소득 간이세액표)",
                article_ko: "별표 2 — 별표시행일자 2026-01-02, 별표HWP파일명 law0039562025123035947KC_000200E_20260102.hwp (주3 자녀세액공제 1명 12,500원)",
                promulgation_ko: "대통령령 제35947호",
                enforced_on: date!(2026 - 01 - 02),
                url: "https://www.law.go.kr/법령/소득세법시행령",
                retrieved_on: statutory_instruments_retrieved_on(),
            },
        ),
        (
            EffectivePeriod::new(date!(2026 - 02 - 27), Some(date!(2026 - 04 - 23))),
            Instrument {
                name_ko: "소득세법 시행령 별표 2 (근로소득 간이세액표)",
                article_ko: "별표 2 — 별표시행일자 2026-02-27, 별표HWP파일명 law0039562026022736129KC_000200E_20260227.hwp (주3 자녀세액공제 1명 20,830원)",
                promulgation_ko: "대통령령 제36129호",
                enforced_on: date!(2026 - 02 - 27),
                url: "https://www.law.go.kr/법령/소득세법시행령",
                retrieved_on: statutory_instruments_retrieved_on(),
            },
        ),
        (
            EffectivePeriod::new(date!(2026 - 04 - 23), Some(date!(2026 - 07 - 01))),
            Instrument {
                name_ko: "소득세법 시행령 별표 2 (근로소득 간이세액표)",
                article_ko: "별표 2 (근로소득 간이세액표, 제189조제1항 관련) — 별표시행일자 2026-04-23, 별표HWP파일명 law0039562026042336276KC_000200E_20260423.hwp",
                // The HWP filename carries the 공포번호: `...36276KC...`. The
                // decree in force on 2026-04-23 is 제36276호 (공포·시행 같은 날),
                // confirmed by eflaw. 제36343호 is enforced 2026-05-22 — a month
                // AFTER this slice begins, so it cannot be what set this 별표.
                promulgation_ko: "대통령령 제36276호",
                enforced_on: date!(2026 - 04 - 23),
                url: "https://www.law.go.kr/법령/소득세법시행령",
                retrieved_on: statutory_instruments_retrieved_on(),
            },
        ),
        (
            EffectivePeriod::new(date!(2026 - 07 - 01), Some(date!(2027 - 01 - 01))),
            Instrument {
                name_ko: "소득세법 시행령 별표 2 (근로소득 간이세액표)",
                article_ko: "별표 2 — 별표시행일자 2026-07-01, 별표HWP파일명 law0039562026052236343KC_000200E_20260701.hwp",
                promulgation_ko: "대통령령 제36343호 (공포 2026. 5. 22.)",
                enforced_on: date!(2026 - 07 - 01),
                url: "https://www.law.go.kr/법령/소득세법시행령",
                retrieved_on: statutory_instruments_retrieved_on(),
            },
        ),
    ]
}

/// 지방소득세 특별징수 — served on **every** draft, so anchored at the earliest
/// slice in force on the dates it is served.
///
/// The previous round moved this to 시행 2026-07-01 and recorded the move as a
/// correction. It was the same wrong-slice defect the round was closing: MST
/// 282559 (법령ID 001649, 공포번호 21308) serves four 시행일자 slices — 20260101,
/// 20260424, 20260701, 20270101 — and 제103조의13 is **byte-identical** across
/// all four. `local_income_tax_instrument` is returned unconditionally by
/// `build_statutory_insurance_draft`, so a 2026-07-01 anchor post-dated every
/// pay date from January to June. Reverted to 20260101.
///
/// # Reproducing the byte-identity digest
///
/// An earlier revision recorded the digest with no normalization beside it,
/// which made it unfalsifiable: a reviewer who tried four other readings of
/// "sha256 of the 조문단위" reproduced none of them and could not tell a wrong
/// number from a wrong method. The method is the evidence, so it is written out
/// here, and this is the only copy of it in the tree.
///
/// 1. `lawService.do?target=eflaw&MST=282559&efYd=<slice>&type=XML`.
/// 2. Select the `<조문단위>` with 조문번호 103, 조문가지번호 13, 조문여부 조문,
///    and take its **inner** content — the `<조문단위>`/`</조문단위>` tags
///    themselves excluded.
/// 3. Delete every `<조문시행일자>…</조문시행일자>` element. Change nothing else:
///    no trimming, no whitespace or newline normalization.
/// 4. sha256 over the UTF-8 bytes.
///
/// Yields `c1cb99797adb21bbb4eddb3b42808c8c291611226ca15173c1f5e6731f8fc8a1` at
/// all four slices. Two near misses, recorded so the next mismatch is diagnosed
/// instead of re-measured: keeping the `<조문단위>` tags gives `cece5eff…`
/// (also stable across the four), and trimming the inner content gives
/// `f7f21af8…`.
///
/// Neither round-2 guard could see it: 지방세법 has no `payroll_statutory_rates`
/// row, so `CHECK (effective_from >= enforced_on)` has nothing to check, and
/// `no_rate_row_is_in_force_before_the_instrument_that_sets_it` walks the rate
/// table. `no_instrument_the_draft_emits_post_dates_the_pay_date_it_is_emitted_on`
/// is the one that can — it reads what the engine actually returns.
#[must_use]
pub const fn local_income_tax_instrument() -> Instrument {
    Instrument {
        name_ko: "지방세법",
        article_ko: "제103조의13제1항 「… 원천징수하는 소득세 … 의 100분의 10에 해당하는 금액을 소득세 원천징수와 동시에 개인지방소득세로 특별징수하여야 한다」",
        promulgation_ko: "법률 제21308호",
        enforced_on: date!(2026 - 01 - 01),
        url: "https://www.law.go.kr/법령/지방세법",
        retrieved_on: statutory_instruments_retrieved_on(),
    }
}

pub fn contribution_rate_on(
    code: ContributionCode,
    day: Date,
) -> Result<StatutoryRate, KernelError> {
    statutory_contribution_rates()
        .into_iter()
        .find(|rate| rate.code == code && rate.period.contains(day))
        .ok_or_else(|| KernelError::validation(format!("missing payroll rate {code:?} for {day}")))
}

/// 기준소득월액 from the declared 소득월액 — 국민연금법 시행령 제5조.
///
/// Read in force on 2026-08-01 from **`target=eflaw`** — `(법령ID 002833,
/// MST 272577, efYd 2026-01-01)`, 대통령령 제35602호. The pinned `efYd` is the
/// anchor; the response's `조문시행일자` restates it and is not a second
/// observation. The earlier record said `target=law` and 시행 2025-07-01; that
/// endpoint returns the latest **promulgated** text, which is how a
/// not-yet-effective rounding clause reached a record in this crate once
/// already. Re-read under eflaw the text is identical but the anchor is not:
/// the article moved to a 2026-01-01 slice.
///
/// The two 항 this function implements, verbatim:
///
/// * 제1항 「법 제3조제1항제5호에 따른 기준소득월액은 다음 각 호의 하한액과
///   상한액의 범위에서 … 신고한 소득월액에서 **천원 미만을 버린 금액**으로
///   한다」
/// * 제5항 「사용자나 가입자가 신고한 소득월액이 … 고시된 하한액보다 적으면 그
///   하한액을, … 상한액보다 많으면 그 상한액을 기준소득월액으로 한다」
///
/// # The article that decides the order
///
/// **제5항.** The band comparison is made on the 신고한 **소득월액** — the figure
/// as declared, before any 절사 — and when it falls outside the band the answer
/// is the 고시 bound *itself*, not a truncated bound. 절사 (제1항) therefore
/// governs only inside the band. That is written here as the statute's own two
/// steps rather than as a composed clamp/truncate pair, because either
/// composition order merely *happens* to agree today: 제1항제1호·제2호 both end
/// 「이 경우 만원 미만은 반올림한다」, so every bound is a 만원 — hence a 천원 —
/// multiple, and on such bounds truncate-then-clamp and clamp-then-truncate are
/// identical. `the_truncate_clamp_order_is_unobservable_only_because_every_band_bound_is_a_10_000_multiple`
/// pins that and fails the day a 고시 breaks it.
fn national_pension_standard_income(declared_won: i64, limit: MonthlyBaseLimit) -> i64 {
    if declared_won < limit.minimum_won {
        limit.minimum_won
    } else if declared_won > limit.maximum_won {
        limit.maximum_won
    } else {
        declared_won - declared_won % 1_000
    }
}

pub fn national_pension_limit_on(day: Date) -> Result<MonthlyBaseLimit, KernelError> {
    national_pension_base_limits()
        .into_iter()
        .find(|limit| limit.period.contains(day))
        .ok_or_else(|| KernelError::validation(format!("missing pension base limit for {day}")))
}

pub fn minimum_wage_on(day: Date) -> Result<MinimumWageRate, KernelError> {
    minimum_wage_rates()
        .into_iter()
        .find(|rate| rate.period.contains(day))
        .ok_or_else(|| KernelError::validation(format!("missing minimum wage for {day}")))
}

pub fn withholding_table_instrument_on(day: Date) -> Result<Instrument, KernelError> {
    simplified_withholding_table_instruments()
        .into_iter()
        .find(|(period, _)| period.contains(day))
        .map(|(_, instrument)| instrument)
        .ok_or_else(|| {
            KernelError::validation(format!("missing 간이세액표 별표 version for {day}"))
        })
}

pub fn build_employee_payroll_draft(input: PayrollDraftInput) -> Result<PayrollDraft, KernelError> {
    if input.monthly_remuneration_won < 0 {
        return Err(KernelError::validation(
            "monthly remuneration must be non-negative",
        ));
    }
    let tax_row = input.nts_tax_row.ok_or_else(|| {
        KernelError::validation(
            "NTS withholding tax table row is required; payroll must not estimate income tax",
        )
    })?;
    if tax_row.monthly_income_tax_won < 0 || tax_row.local_income_tax_won < 0 {
        return Err(KernelError::validation(
            "NTS tax row amounts must be non-negative",
        ));
    }

    let lines = employee_deduction_lines(
        input.pay_date,
        input.monthly_remuneration_won,
        input.pension_standard_monthly_income_won,
        tax_row.monthly_income_tax_won,
        tax_row.local_income_tax_won,
        tax_row.source.url,
    )?;
    let (total_employee_deductions_won, net_pay_won) =
        deduction_totals(&lines, input.monthly_remuneration_won)?;

    Ok(PayrollDraft {
        pay_date: input.pay_date,
        gross_wage_won: input.monthly_remuneration_won,
        taxable_income_tax_table_version: tax_row.table_version,
        lines,
        total_employee_deductions_won,
        net_pay_won,
    })
}

/// An NTS 간이세액표 row whose figures were materialized from a verified
/// imported source ledger row (`data_import_rows.canonical_row`), so its table
/// version is a runtime string — unlike [`NtsWithholdingTaxRow`], whose
/// `&'static` fields exist for in-crate verified constants. The amounts are
/// still verbatim source figures: this type never carries an estimate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedNtsTaxRow {
    pub table_version: String,
    pub monthly_income_tax_won: i64,
    pub local_income_tax_won: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineCalculationInput {
    pub pay_date: Date,
    pub gross_won: i64,
    pub pension_standard_monthly_income_won: Option<i64>,
    pub tax_row: VerifiedNtsTaxRow,
}

/// One employee's draft deduction breakdown for persistence in
/// `payroll_line_calculations`. Always a DRAFT: the release gate
/// ([`validate_release_gate`]) is what makes a stored calculation payable,
/// never this function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineCalculation {
    pub gross_won: i64,
    pub lines: Vec<DeductionLine>,
    pub total_employee_deductions_won: i64,
    pub net_won: i64,
    pub tax_table_version: String,
}

/// [`build_employee_payroll_draft`] for a source-materialized tax row: same
/// statutory 4-insurance math from the in-crate verified rate tables, income
/// tax verbatim from the supplied verified NTS row. Refuses (like the draft
/// builder) to compute anything without that row.
pub fn build_line_calculation(input: LineCalculationInput) -> Result<LineCalculation, KernelError> {
    if input.gross_won < 0 {
        return Err(KernelError::validation(
            "monthly remuneration must be non-negative",
        ));
    }
    if input.tax_row.monthly_income_tax_won < 0 || input.tax_row.local_income_tax_won < 0 {
        return Err(KernelError::validation(
            "NTS tax row amounts must be non-negative",
        ));
    }
    if input.tax_row.table_version.trim().is_empty() {
        return Err(KernelError::validation(
            "NTS tax table version must be supplied with the verified source row",
        ));
    }

    let lines = employee_deduction_lines(
        input.pay_date,
        input.gross_won,
        input.pension_standard_monthly_income_won,
        input.tax_row.monthly_income_tax_won,
        input.tax_row.local_income_tax_won,
        nts_source().url,
    )?;
    let (total_employee_deductions_won, net_won) = deduction_totals(&lines, input.gross_won)?;

    Ok(LineCalculation {
        gross_won: input.gross_won,
        lines,
        total_employee_deductions_won,
        net_won,
        tax_table_version: input.tax_row.table_version,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// The 4대보험 pipeline.
//
// ORDERED, not merely sequential: 장기요양's basis is the computed 건강보험료액
// (노인장기요양보험법 제9조제1항), so 건강보험 must be resolved first. That
// dependency is an accumulator the pipeline reads, and an unresolved one is a
// loud computation error — never a silent zero.
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatutoryInsuranceInput {
    pub pay_date: Date,
    /// 보수월액.
    pub monthly_remuneration_won: i64,
    /// 기준소득월액 when it differs from 보수월액; clamped by the 고시 band
    /// either way.
    pub pension_standard_monthly_income_won: Option<i64>,
    /// 월 소정근로시간 — the minimum-wage comparator's denominator. `None`
    /// means the comparison cannot be made and is reported as such, never as a
    /// pass.
    pub monthly_standard_hours: Option<i32>,
}

/// One computed 공제 component, carrying its own basis, rate and instrument so
/// the payslip is self-describing under 근로기준법 제42조's 3-year
/// recomputability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatutoryComponent {
    pub code: ContributionCode,
    pub label_ko: &'static str,
    pub basis: ContributionBasis,
    /// `None` for 산재 alone: its 사업종류별 요율 and the basis that rate applies
    /// to are set per employer by an un-ingested 별지, so this engine computes
    /// neither. A `0` here would be a claim about a figure nobody read.
    pub basis_won: Option<i64>,
    /// `None` with `basis_won`, and for the same reason — see above.
    pub rate_num: Option<i64>,
    pub rate_den: Option<i64>,
    pub total_won: Option<i64>,
    pub employee_won: Option<i64>,
    /// `true` for 산재: the absence of an employee line is STATED, not silent.
    pub employer_only: bool,
    /// The unresolved question id that withheld this amount, if any.
    pub blocked_by: Option<&'static str>,
    pub total_rounding_unit: &'static str,
    pub employee_rounding_unit: &'static str,
    pub instrument: Instrument,
    pub share_instrument: Option<Instrument>,
    pub provenance: &'static str,
}

/// A deduction the engine refuses to compute, naming the instrument that would
/// supply it. Never a zero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotComputed {
    pub code: DeductionCode,
    pub label_ko: &'static str,
    pub reason_ko: &'static str,
    pub instrument: Instrument,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MinimumWageCheck {
    pub hourly_floor_won: i64,
    pub monthly_209h_floor_won: i64,
    pub monthly_standard_hours: Option<i32>,
    /// 보수월액 ÷ 월 소정근로시간, floored. `None` without the hours.
    pub effective_hourly_won: Option<i64>,
    /// `None` means "not comparable", which is not a pass.
    pub passes: Option<bool>,
    pub instrument: Instrument,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatutoryInsuranceDraft {
    pub pay_date: Date,
    pub gross_won: i64,
    pub components: Vec<StatutoryComponent>,
    pub not_computed: Vec<NotComputed>,
    pub minimum_wage: MinimumWageCheck,
    /// `None` when any employee component is blocked.
    pub total_employee_insurance_won: Option<i64>,
    pub remainder_after_insurance_won: Option<i64>,
    /// Always false while withholding is not computed. A draft that cannot
    /// state 차인지급액 is not a 급여명세서.
    pub issuable: bool,
    pub blockers: Vec<String>,
}

const PIPELINE_ORDER: [(ContributionCode, &str); 5] = [
    (ContributionCode::NationalPension, "국민연금"),
    (ContributionCode::HealthInsurance, "건강보험"),
    (ContributionCode::LongTermCare, "장기요양보험"),
    (ContributionCode::EmploymentUnemployment, "고용보험"),
    (ContributionCode::IndustrialAccident, "산업재해보상보험"),
];

/// The 4대보험 half of a 급여명세서, to the won, every figure carrying its
/// instrument. Withholding is refused, not approximated.
pub fn build_statutory_insurance_draft(
    input: &StatutoryInsuranceInput,
) -> Result<StatutoryInsuranceDraft, KernelError> {
    if input.monthly_remuneration_won < 0 {
        return Err(KernelError::validation(
            "monthly remuneration must be non-negative",
        ));
    }
    if let Some(hours) = input.monthly_standard_hours
        && hours <= 0
    {
        return Err(KernelError::validation(
            "monthly standard hours must be positive when supplied",
        ));
    }

    let mut components: Vec<StatutoryComponent> = Vec::with_capacity(PIPELINE_ORDER.len());
    let mut blockers: Vec<String> = Vec::new();
    // The accumulator the ordering exists for.
    let mut health_total_won: Option<i64> = None;

    for (code, label_ko) in PIPELINE_ORDER {
        let rate = contribution_rate_on(code, input.pay_date)?;
        // 산재's 사업종류별 요율 lives in 고용노동부고시 제2025-91호's 별지, which
        // is NOT ingested. The employee rate is a true 0 (징수법 제13조제5항 sets
        // 산재보험료 as a 사업주 charge and no 항 charges the worker), but the
        // EMPLOYER figures — the rate, the basis it applies to, and the premium
        // — are unknown, not zero. Emitting 0 for any of them would publish
        // "산재 costs nothing", the silently-zero failure this engine refuses.
        // `employee_won` stays None via `ShareRule::EmployerOnly`.
        let rate_is_ingested = rate.basis != ContributionBasis::IndustryTariff;
        let basis_won = match rate.basis {
            ContributionBasis::MonthlyStandardIncome => Some(national_pension_standard_income(
                input
                    .pension_standard_monthly_income_won
                    .unwrap_or(input.monthly_remuneration_won),
                national_pension_limit_on(input.pay_date)?,
            )),
            ContributionBasis::MonthlyRemuneration => Some(input.monthly_remuneration_won),
            // FAIL LOUD. If 건강보험 has not been resolved by the time
            // 장기요양 is reached, the pipeline order is broken; producing 0
            // here would under-deduct every payslip silently.
            ContributionBasis::HealthInsurancePremium => Some(health_total_won.ok_or_else(|| {
                KernelError::internal(
                    "장기요양보험료의 산정기초는 건강보험료액(노인장기요양보험법 제9조제1항)이다: \
                     건강보험 구성요소가 먼저 산정되어야 한다",
                )
            })?),
            ContributionBasis::IndustryTariff => None,
        };

        let mut blocked_by: Option<&'static str> = None;
        let total_won = match basis_won {
            None => None,
            Some(basis_won) => {
                let raw = checked_mul_i128(basis_won, rate.rate_num)? / i128::from(rate.rate_den);
                match rate.total_rounding.apply(raw) {
                    Ok(rounded) => {
                        let clamped = match rate.clamp {
                            Some(clamp) => rounded
                                .clamp(i128::from(clamp.floor_won), i128::from(clamp.cap_won)),
                            None => rounded,
                        };
                        Some(checked_i128_to_i64(clamped)?)
                    }
                    Err(question_id) => {
                        blocked_by = Some(question_id);
                        None
                    }
                }
            }
        };

        if code == ContributionCode::HealthInsurance {
            health_total_won = total_won;
        }

        let employer_only = matches!(rate.employee_share, ShareRule::EmployerOnly);
        let employee_won = match (total_won, rate.employee_share) {
            (_, ShareRule::EmployerOnly) => None,
            (Some(total), ShareRule::WholeOfRate) => Some(total),
            (Some(total), ShareRule::Half { rounding }) => {
                match rounding.apply(i128::from(total) * 50 / 100) {
                    Ok(share) => Some(checked_i128_to_i64(share)?),
                    Err(question_id) => {
                        blocked_by = Some(question_id);
                        None
                    }
                }
            }
            (None, _) => None,
        };

        if let Some(question_id) = blocked_by {
            blockers.push(format!("{label_ko}: 미해결 반올림 쟁점 {question_id}"));
        }

        components.push(StatutoryComponent {
            code,
            label_ko,
            basis: rate.basis,
            basis_won,
            rate_num: rate_is_ingested.then_some(rate.rate_num),
            rate_den: rate_is_ingested.then_some(rate.rate_den),
            total_won,
            employee_won,
            employer_only,
            blocked_by,
            total_rounding_unit: rounding_unit_label(rate.total_rounding),
            employee_rounding_unit: match rate.employee_share {
                ShareRule::Half { rounding } => rounding_unit_label(rounding),
                ShareRule::WholeOfRate => rounding_unit_label(rate.total_rounding),
                ShareRule::EmployerOnly => "NOT_APPLICABLE",
            },
            instrument: rate.instrument,
            share_instrument: rate.share_instrument,
            provenance: rate.provenance,
        });
    }

    let total_employee_insurance_won = components
        .iter()
        .filter(|component| !component.employer_only)
        .try_fold(0_i64, |total, component| {
            component
                .employee_won
                .and_then(|amount| total.checked_add(amount))
        });
    let remainder_after_insurance_won = total_employee_insurance_won
        .and_then(|total| input.monthly_remuneration_won.checked_sub(total));

    let minimum_wage = check_minimum_wage(
        input.monthly_remuneration_won,
        input.monthly_standard_hours,
        input.pay_date,
    )?;
    match minimum_wage.passes {
        Some(false) => blockers.push(
            "최저임금 미달: 고용노동부고시 제2025-47호 시간급 10,320원에 미달한다".to_owned(),
        ),
        None => blockers.push("최저임금 비교 불가: 월 소정근로시간이 근로계약에 없다".to_owned()),
        Some(true) => {}
    }

    let not_computed = vec![
        NotComputed {
            code: DeductionCode::IncomeTax,
            label_ko: "근로소득세",
            reason_ko: "간이세액표(별표 2) 미탑재 — 이 커널은 세액을 추정하지 않는다",
            instrument: withholding_table_instrument_on(input.pay_date)?,
        },
        NotComputed {
            code: DeductionCode::LocalIncomeTax,
            label_ko: "지방소득세",
            reason_ko: "원천징수 소득세의 100분의 10이므로, 소득세가 없으면 산출할 수 없다",
            instrument: local_income_tax_instrument(),
        },
    ];
    blockers.push("WITHHOLDING_NOT_COMPUTED".to_owned());

    Ok(StatutoryInsuranceDraft {
        pay_date: input.pay_date,
        gross_won: input.monthly_remuneration_won,
        components,
        not_computed,
        minimum_wage,
        total_employee_insurance_won,
        remainder_after_insurance_won,
        // Withholding is always absent in this slice, so this is always false.
        // It is computed rather than hard-coded so that adding the 별표 flips it
        // without a second edit here.
        issuable: blockers.is_empty(),
        blockers,
    })
}

fn rounding_unit_label(rounding: Rounding) -> &'static str {
    match rounding {
        Rounding::Resolved { unit, .. } | Rounding::Assumed { unit, .. } => unit.as_str(),
        Rounding::Unresolved { question_id, .. } => question_id,
    }
}

/// 최저임금법 제10조 — the comparator `minimum_wage_on` never had.
pub fn check_minimum_wage(
    monthly_remuneration_won: i64,
    monthly_standard_hours: Option<i32>,
    pay_date: Date,
) -> Result<MinimumWageCheck, KernelError> {
    let rate = minimum_wage_on(pay_date)?;
    let effective_hourly_won =
        monthly_standard_hours.map(|hours| monthly_remuneration_won / i64::from(hours));
    Ok(MinimumWageCheck {
        hourly_floor_won: rate.hourly_won,
        monthly_209h_floor_won: rate.monthly_209h_won,
        monthly_standard_hours,
        effective_hourly_won,
        passes: effective_hourly_won.map(|hourly| hourly >= rate.hourly_won),
        instrument: rate.instrument,
    })
}

/// The six employee-side deduction lines: the 4대보험 pipeline above + the two
/// income-tax lines verbatim from a verified NTS row (never estimated).
///
/// This is the ONLY arithmetic path. `calculate_run_in_tx` reaches it through
/// [`build_line_calculation`], so the run path and the payslip-draft route
/// cannot disagree about a won.
fn employee_deduction_lines(
    pay_date: Date,
    monthly_remuneration_won: i64,
    pension_standard_monthly_income_won: Option<i64>,
    monthly_income_tax_won: i64,
    local_income_tax_won: i64,
    tax_source_url: &'static str,
) -> Result<Vec<DeductionLine>, KernelError> {
    let draft = build_statutory_insurance_draft(&StatutoryInsuranceInput {
        pay_date,
        monthly_remuneration_won,
        pension_standard_monthly_income_won,
        monthly_standard_hours: None,
    })?;

    let mut lines = Vec::with_capacity(6);
    for component in &draft.components {
        if component.employer_only {
            continue;
        }
        let amount_won = component.employee_won.ok_or_else(|| {
            KernelError::validation(format!(
                "{}: 미해결 반올림 쟁점 {}으로 금액을 산출할 수 없다",
                component.label_ko,
                component.blocked_by.unwrap_or("UNKNOWN"),
            ))
        })?;
        lines.push(DeductionLine {
            code: deduction_code_for(component.code)?,
            label_ko: component.label_ko,
            amount_won,
            source_url: component.instrument.url,
        });
    }

    lines.push(deduction(
        DeductionCode::IncomeTax,
        "근로소득세",
        monthly_income_tax_won,
        tax_source_url,
    ));
    lines.push(deduction(
        DeductionCode::LocalIncomeTax,
        "지방소득세",
        local_income_tax_won,
        tax_source_url,
    ));
    Ok(lines)
}

fn deduction_code_for(code: ContributionCode) -> Result<DeductionCode, KernelError> {
    match code {
        ContributionCode::NationalPension => Ok(DeductionCode::NationalPension),
        ContributionCode::HealthInsurance => Ok(DeductionCode::HealthInsurance),
        ContributionCode::LongTermCare => Ok(DeductionCode::LongTermCare),
        ContributionCode::EmploymentUnemployment => Ok(DeductionCode::EmploymentInsurance),
        ContributionCode::IndustrialAccident => Err(KernelError::conflict(
            "산재보험은 근로자 공제 항목이 아니다",
        )),
    }
}

fn deduction_totals(lines: &[DeductionLine], gross_won: i64) -> Result<(i64, i64), KernelError> {
    let total = lines
        .iter()
        .map(|line| line.amount_won)
        .try_fold(0_i64, |total, amount| {
            total
                .checked_add(amount)
                .ok_or_else(|| KernelError::validation("deduction total overflow"))
        })?;
    let net = gross_won
        .checked_sub(total)
        .ok_or_else(|| KernelError::validation("deductions exceed gross wage"))?;
    Ok((total, net))
}

pub fn build_severance_pay_draft(
    input: SeverancePayInput,
) -> Result<SeverancePayDraft, KernelError> {
    if input.exit_date < input.hire_date {
        return Err(KernelError::validation(
            "exit date must be on or after hire date",
        ));
    }
    if input.average_wage_period_end < input.average_wage_period_start {
        return Err(KernelError::validation(
            "average wage period end must be on or after start",
        ));
    }
    if input.average_wage_period_end > input.exit_date {
        return Err(KernelError::validation(
            "average wage period must not end after the exit date",
        ));
    }
    if input.average_wage_calendar_days <= 0 {
        return Err(KernelError::validation(
            "average wage calendar days must be positive",
        ));
    }
    if input.average_wage_total_won <= 0 {
        return Err(KernelError::validation(
            "average wage total must be positive",
        ));
    }
    // FAIL LOUD: the 통상임금 floor cannot be applied without the ordinary wage.
    // Reject rather than silently compute severance from the average wage alone,
    // which under-pays the absence→exit population this feature targets.
    if input.ordinary_daily_wage_won <= 0 {
        return Err(KernelError::validation(
            "ordinary daily wage (통상임금) is required and must be positive; severance must never fall back to the average-wage-only figure",
        ));
    }

    let service_days = i64::from(
        input
            .exit_date
            .to_julian_day()
            .saturating_sub(input.hire_date.to_julian_day())
            + 1,
    );
    if service_days < 365 {
        return Err(KernelError::validation(
            "statutory severance pay requires at least one year of service",
        ));
    }

    let average_daily_wage_milliwon = checked_i128_to_i64(
        checked_mul_i128(input.average_wage_total_won, 1_000)?
            / i128::from(input.average_wage_calendar_days),
    )?;

    // 통상임금 floor (근로기준법 시행령 제2조②): severance uses the HIGHER of the
    // 1-day average wage and the 1-day ordinary wage. Decide which governs by
    // cross-multiplying (ordinary_daily > average_daily  <=>  ordinary_daily *
    // calendar_days > average_wage_total), so the comparison never loses the
    // fractional part that flooring `average_daily_wage_milliwon` would drop.
    let ordinary_governs = checked_mul_i128(
        input.ordinary_daily_wage_won,
        input.average_wage_calendar_days,
    )? > i128::from(input.average_wage_total_won);

    let statutory_daily_wage_milliwon = if ordinary_governs {
        checked_i128_to_i64(checked_mul_i128(input.ordinary_daily_wage_won, 1_000)?)?
    } else {
        average_daily_wage_milliwon
    };

    // Fold both paths into one formula by choosing the numerator base and its
    // divisor. Ordinary path uses exact won (divisor 1); average path keeps the
    // existing high-precision direct form (divisor = calendar_days) so its
    // rounding is byte-for-byte unchanged when it governs.
    let (base_won, base_divisor) = if ordinary_governs {
        (input.ordinary_daily_wage_won, 1_i64)
    } else {
        (
            input.average_wage_total_won,
            input.average_wage_calendar_days,
        )
    };

    let statutory_30_day_wage_won =
        checked_i128_to_i64(checked_mul_i128(base_won, 30)? / i128::from(base_divisor))?;
    let severance_pay_won = checked_i128_to_i64(
        checked_mul_i128(base_won, 30)?
            .checked_mul(i128::from(service_days))
            .ok_or_else(|| KernelError::validation("severance calculation overflow"))?
            / i128::from(base_divisor)
            / 365,
    )?;

    Ok(SeverancePayDraft {
        hire_date: input.hire_date,
        exit_date: input.exit_date,
        service_days,
        average_wage_period_start: input.average_wage_period_start,
        average_wage_period_end: input.average_wage_period_end,
        average_wage_calendar_days: input.average_wage_calendar_days,
        average_wage_total_won: input.average_wage_total_won,
        average_daily_wage_milliwon,
        ordinary_daily_wage_won: input.ordinary_daily_wage_won,
        statutory_daily_wage_milliwon,
        statutory_30_day_wage_won,
        severance_pay_won,
        source: moel_retirement_pay_source(),
    })
}

pub fn validate_release_gate(input: &PayrollReleaseGateInput) -> Result<(), KernelError> {
    if input.rate_table_version.trim().is_empty() {
        return Err(KernelError::validation(
            "payroll rate table version is required",
        ));
    }
    if input.official_source_urls.is_empty() {
        return Err(KernelError::validation(
            "at least one official source URL is required",
        ));
    }
    if input.golden_cases.is_empty() {
        return Err(KernelError::validation(
            "at least one payroll golden case is required",
        ));
    }
    if let Some(case) = input
        .golden_cases
        .iter()
        .find(|case| case.rate_table_version != input.rate_table_version)
    {
        return Err(KernelError::validation(format!(
            "golden case {} uses mismatched rate table version",
            case.case_id
        )));
    }
    if let Some(case) = input
        .golden_cases
        .iter()
        .find(|case| !case.professionally_validated)
    {
        return Err(KernelError::validation(format!(
            "golden case {} lacks professional validation",
            case.case_id
        )));
    }
    let validation = input.professional_validation.as_ref().ok_or_else(|| {
        KernelError::validation("노무사/세무사 professional validation is required")
    })?;
    if validation.artifact_sha256.len() != 64
        || !validation
            .artifact_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(KernelError::validation(
            "professional validation artifact_sha256 must be a 64-character hex digest",
        ));
    }
    if validation.reviewer_reference.trim().is_empty() {
        return Err(KernelError::validation(
            "professional validation reviewer reference is required",
        ));
    }
    // The golden case is RE-EXECUTED, not merely stored: until this loop
    // existed the professionals' signed figure was compared to nothing, so a
    // golden case could not fail. LAST on purpose — every check above returns
    // on its first failure, so running the arithmetic earlier would surface a
    // mismatch message in place of the specific pre-existing condition that
    // actually regressed.
    //
    // Strictly stricter, and it admits a THIRD failure class to the gate:
    // `build_line_calculation`'s own refusals (blank tax table_version,
    // negative amounts, a pay_date outside the in-crate rate windows) now fail
    // the GATE, not just line calculation. That is deliberate — a case whose
    // declared inputs the kernel cannot execute is not a case that passed.
    for case in &input.golden_cases {
        // .clone() is mandatory: build_line_calculation takes its input by
        // value and moves tax_row.table_version, while this function only
        // borrows &PayrollReleaseGateInput.
        let computed = build_line_calculation(case.inputs.clone()).map_err(|err| {
            KernelError::validation(format!(
                "golden case {} could not be recomputed: {}",
                case.case_id, err.message
            ))
        })?;
        if computed.total_employee_deductions_won != case.expected_total_employee_deductions_won {
            return Err(KernelError::validation(format!(
                "golden case {} expects total employee deductions {} but the payroll kernel computed {}",
                case.case_id,
                case.expected_total_employee_deductions_won,
                computed.total_employee_deductions_won
            )));
        }
    }
    Ok(())
}

fn checked_mul_i128(left: i64, right: i64) -> Result<i128, KernelError> {
    i128::from(left)
        .checked_mul(i128::from(right))
        .ok_or_else(|| KernelError::validation("payroll amount multiplication overflow"))
}

fn checked_i128_to_i64(amount: i128) -> Result<i64, KernelError> {
    i64::try_from(amount).map_err(|_| KernelError::validation("payroll amount overflow"))
}

fn deduction(
    code: DeductionCode,
    label_ko: &'static str,
    amount_won: i64,
    source_url: &'static str,
) -> DeductionLine {
    DeductionLine {
        code,
        label_ko,
        amount_won,
        source_url,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn closed_june_prerequisites() -> PayrollClosePrerequisites {
        PayrollClosePrerequisites {
            period_start: date!(2026 - 06 - 01),
            period_end: date!(2026 - 06 - 30),
            org_month_close_present: true,
            active_exact_payroll_lock_present: true,
            unresolved_attendance_exception_count: 0,
        }
    }

    // These two tests pin the FSM's own refusals. They are named for the
    // function, not for the system, because `transition_payroll_run` has no
    // production caller — see its doc comment. A name like
    // `calculation_is_blocked_without_...` asserted a system property this
    // repository does not have, and would have become a CI-endorsed falsehood
    // the moment this target was wired into a workflow.
    #[test]
    fn fsm_refuses_calculate_command_unconditionally() {
        let error = transition_payroll_run(
            PayrollRunStatus::Staged,
            PayrollRunCommand::Calculate,
            closed_june_prerequisites(),
        )
        .unwrap_err();

        assert_eq!(error.kind, console_kernel_core::ErrorKind::Conflict);
        assert!(
            error
                .message
                .contains("immutable validated release-gate evidence")
        );
    }

    // The refusal message names step-up, audit evidence and an immutable
    // artifact. None of those three is implemented on the production issuance
    // path — this asserts the STRING, which is why the name says so.
    #[test]
    fn fsm_refuses_mark_issued_command_citing_unimplemented_controls() {
        let error = transition_payroll_run(
            PayrollRunStatus::Approved,
            PayrollRunCommand::MarkIssued,
            closed_june_prerequisites(),
        )
        .unwrap_err();

        assert_eq!(error.kind, console_kernel_core::ErrorKind::Conflict);
        assert!(error.message.contains("step-up authorization"));
        assert!(error.message.contains("audit evidence"));
        assert!(error.message.contains("immutable issuance artifact"));
    }

    #[test]
    fn approval_requires_exact_closed_month_and_is_idempotent_after_persistence() {
        let prerequisites = closed_june_prerequisites();
        let approved = transition_payroll_run(
            PayrollRunStatus::ReadyForReview,
            PayrollRunCommand::Approve {
                approver_is_creator: false,
            },
            prerequisites,
        )
        .unwrap();
        assert_eq!(approved.status, PayrollRunStatus::Approved);
        assert!(!approved.idempotent);

        let retry = transition_payroll_run(
            PayrollRunStatus::Approved,
            PayrollRunCommand::Approve {
                approver_is_creator: false,
            },
            PayrollClosePrerequisites {
                org_month_close_present: false,
                ..prerequisites
            },
        )
        .unwrap();
        assert!(retry.idempotent);
    }

    #[test]
    fn approval_fails_closed_on_missing_close_lock_or_exception() {
        let prerequisites = closed_june_prerequisites();
        for missing in [
            PayrollClosePrerequisites {
                org_month_close_present: false,
                ..prerequisites
            },
            PayrollClosePrerequisites {
                active_exact_payroll_lock_present: false,
                ..prerequisites
            },
            PayrollClosePrerequisites {
                unresolved_attendance_exception_count: 1,
                ..prerequisites
            },
            PayrollClosePrerequisites {
                period_start: date!(2026 - 06 - 02),
                ..prerequisites
            },
        ] {
            assert!(
                transition_payroll_run(
                    PayrollRunStatus::ReadyForReview,
                    PayrollRunCommand::Approve {
                        approver_is_creator: false,
                    },
                    missing,
                )
                .is_err()
            );
        }
    }

    #[test]
    fn approval_rejects_creator_self_approval() {
        let error = transition_payroll_run(
            PayrollRunStatus::ReadyForReview,
            PayrollRunCommand::Approve {
                approver_is_creator: true,
            },
            closed_june_prerequisites(),
        )
        .unwrap_err();
        assert_eq!(error.kind, console_kernel_core::ErrorKind::Forbidden);
    }

    /// The declared kernel inputs behind the 373,302 figure every golden-case
    /// fixture in this crate already stores. Pinned green independently by
    /// `builds_employee_deduction_draft_from_effective_rates_and_supplied_nts_row`.
    fn golden_case_inputs() -> LineCalculationInput {
        LineCalculationInput {
            pay_date: date!(2026 - 06 - 27),
            gross_won: 3_000_000,
            pension_standard_monthly_income_won: None,
            tax_row: VerifiedNtsTaxRow {
                table_version: "NTS-간이세액표-fixture-row-v1".to_owned(),
                monthly_income_tax_won: 74_350,
                local_income_tax_won: 7_430,
            },
        }
    }

    fn fixture_tax_row() -> NtsWithholdingTaxRow {
        NtsWithholdingTaxRow {
            table_version: "NTS-간이세액표-fixture-row-v1",
            monthly_income_tax_won: 74_350,
            local_income_tax_won: 7_430,
            source: nts_source(),
        }
    }

    #[test]
    fn selects_2026_rates_and_effective_dated_pension_limits() {
        let june_limit = national_pension_limit_on(date!(2026 - 06 - 27)).unwrap();
        assert_eq!(june_limit.minimum_won, 400_000);
        assert_eq!(june_limit.maximum_won, 6_370_000);

        let july_limit = national_pension_limit_on(date!(2026 - 07 - 01)).unwrap();
        assert_eq!(july_limit.minimum_won, 410_000);
        assert_eq!(july_limit.maximum_won, 6_590_000);

        let pension =
            contribution_rate_on(ContributionCode::NationalPension, date!(2026 - 06 - 27)).unwrap();
        assert_eq!((pension.rate_num, pension.rate_den), (475, 10_000));
        assert_eq!(pension.employee_share, ShareRule::WholeOfRate);
        assert_eq!(pension.instrument.promulgation_ko, "법률 제20903호");

        let minimum_wage = minimum_wage_on(date!(2026 - 06 - 27)).unwrap();
        assert_eq!(minimum_wage.hourly_won, 10_320);
        assert_eq!(minimum_wage.monthly_209h_won, 2_156_880);
    }

    #[test]
    fn refuses_to_estimate_income_tax_without_an_nts_table_row() {
        let result = build_employee_payroll_draft(PayrollDraftInput {
            pay_date: date!(2026 - 06 - 27),
            monthly_remuneration_won: 3_000_000,
            pension_standard_monthly_income_won: None,
            nts_tax_row: None,
        });

        assert!(result.is_err());
        assert!(format!("{:?}", result.err().unwrap()).contains("NTS withholding tax table row"));
    }

    #[test]
    fn builds_employee_deduction_draft_from_effective_rates_and_supplied_nts_row() {
        let draft = build_employee_payroll_draft(PayrollDraftInput {
            pay_date: date!(2026 - 06 - 27),
            monthly_remuneration_won: 3_000_000,
            pension_standard_monthly_income_won: None,
            nts_tax_row: Some(fixture_tax_row()),
        })
        .unwrap();

        assert_eq!(line_amount(&draft, DeductionCode::NationalPension), 142_500);
        assert_eq!(line_amount(&draft, DeductionCode::HealthInsurance), 107_850);
        assert_eq!(line_amount(&draft, DeductionCode::LongTermCare), 14_170);
        assert_eq!(
            line_amount(&draft, DeductionCode::EmploymentInsurance),
            27_000
        );
        assert_eq!(line_amount(&draft, DeductionCode::IncomeTax), 74_350);
        assert_eq!(line_amount(&draft, DeductionCode::LocalIncomeTax), 7_430);
        assert_eq!(draft.total_employee_deductions_won, 373_300);
        assert_eq!(draft.net_pay_won, 2_626_700);
        assert_eq!(
            draft.taxable_income_tax_table_version,
            "NTS-간이세액표-fixture-row-v1"
        );
    }

    #[test]
    fn line_calculation_matches_draft_builder_amounts_exactly() {
        let draft = build_employee_payroll_draft(PayrollDraftInput {
            pay_date: date!(2026 - 06 - 27),
            monthly_remuneration_won: 3_000_000,
            pension_standard_monthly_income_won: None,
            nts_tax_row: Some(fixture_tax_row()),
        })
        .unwrap();
        let calc = build_line_calculation(LineCalculationInput {
            pay_date: date!(2026 - 06 - 27),
            gross_won: 3_000_000,
            pension_standard_monthly_income_won: None,
            tax_row: VerifiedNtsTaxRow {
                table_version: "NTS-간이세액표-fixture-row-v1".to_owned(),
                monthly_income_tax_won: 74_350,
                local_income_tax_won: 7_430,
            },
        })
        .unwrap();

        assert_eq!(calc.lines, draft.lines);
        assert_eq!(
            calc.total_employee_deductions_won,
            draft.total_employee_deductions_won
        );
        assert_eq!(calc.net_won, draft.net_pay_won);
        assert_eq!(calc.tax_table_version, "NTS-간이세액표-fixture-row-v1");
    }

    #[test]
    fn line_calculation_refuses_blank_table_version_and_negative_tax() {
        let base = LineCalculationInput {
            pay_date: date!(2026 - 06 - 27),
            gross_won: 3_000_000,
            pension_standard_monthly_income_won: None,
            tax_row: VerifiedNtsTaxRow {
                table_version: "  ".to_owned(),
                monthly_income_tax_won: 74_350,
                local_income_tax_won: 7_430,
            },
        };
        assert!(build_line_calculation(base.clone()).is_err());

        let mut negative = base;
        negative.tax_row.table_version = "v1".to_owned();
        negative.tax_row.monthly_income_tax_won = -1;
        assert!(build_line_calculation(negative).is_err());
    }

    #[test]
    fn caps_national_pension_basis_by_effective_limit() {
        let draft = build_employee_payroll_draft(PayrollDraftInput {
            pay_date: date!(2026 - 07 - 01),
            monthly_remuneration_won: 10_000_000,
            pension_standard_monthly_income_won: None,
            nts_tax_row: Some(fixture_tax_row()),
        })
        .unwrap();

        // 기준소득월액 상한 6,590,000 × 475/10,000 = 313,025 → 제117조 절사 →
        // 313,020. The run path reaches the same 절사 as the payslip route
        // because both go through `build_statutory_insurance_draft`.
        assert_eq!(line_amount(&draft, DeductionCode::NationalPension), 313_020);
    }

    /// 국민연금법 시행령 제5조제1항 — 「신고한 소득월액에서 천원 미만을 버린
    /// 금액」. Without it every wage that is not already a 천원 multiple is
    /// over-deducted, which for an engine that claims to match a hand
    /// calculation to the won is simply a wrong figure.
    #[test]
    fn truncates_the_pension_basis_below_1_000_won_before_applying_the_rate() {
        let draft = build_statutory_insurance_draft(&StatutoryInsuranceInput {
            pay_date: date!(2026 - 08 - 10),
            monthly_remuneration_won: 3_000_999,
            pension_standard_monthly_income_won: None,
            monthly_standard_hours: Some(209),
        })
        .unwrap();

        let pension = component(&draft, ContributionCode::NationalPension);
        // 3,000,999 → 기준소득월액 3,000,000 (NOT 3,000,999).
        assert_eq!(pension.basis_won, Some(3_000_000));
        // 3,000,000 × 475/10,000 = 142,500. The untruncated basis would give
        // 142,547 — 47원 too much, every month, for this employee.
        assert_eq!(pension.employee_won, Some(142_500));
        assert_ne!(pension.employee_won, Some(3_000_999 * 475 / 10_000));

        // 보수월액 itself is untouched: 절사 is a 기준소득월액 rule, not a
        // 건강보험 one (건강보험 keeps 국민건강보험법 제70조's 보수월액).
        assert_eq!(draft.gross_won, 3_000_999);
        assert_eq!(
            component(&draft, ContributionCode::HealthInsurance).basis_won,
            Some(3_000_999)
        );
    }

    /// 국민연금법 제117조(단수의 처리) — the bridge an earlier round recorded as
    /// absent.
    ///
    /// The golden case cannot catch this: 3,000,000 × 475/10,000 = 142,500 is
    /// already a 10원 multiple, so 절사 and no-절사 agree there and the missing
    /// rule stayed invisible through a full review. This test is at a wage where
    /// it bites.
    #[test]
    fn national_pension_truncates_the_10_won_remainder_under_article_117() {
        let draft = build_statutory_insurance_draft(&StatutoryInsuranceInput {
            pay_date: date!(2026 - 08 - 10),
            // Already a 천원 multiple, so 시행령 제5조제1항 moves nothing and the
            // only rule left to observe is 제117조.
            monthly_remuneration_won: 3_001_000,
            pension_standard_monthly_income_won: None,
            monthly_standard_hours: Some(209),
        })
        .unwrap();

        let pension = component(&draft, ContributionCode::NationalPension);
        assert_eq!(pension.basis_won, Some(3_001_000));
        // 3,001,000 × 475/10,000 = 142,547.5 → 정확분 142,547 → 절사 142,540.
        assert_eq!(pension.employee_won, Some(142_540));
        // The figure this crate emitted while the bridge was recorded as absent.
        assert_ne!(pension.employee_won, Some(142_547));
        assert_eq!(pension.total_rounding_unit, "TRUNC_10_WON");
        assert!(
            pension.instrument.article_ko.contains("1만분의 475"),
            "the RATE instrument stays the 부칙: {}",
            pension.instrument.article_ko
        );

        // Every 국민연금 amount is now a 10원 multiple, at every wage — the
        // property the single wage above only samples.
        for gross in (0..12_000_000).step_by(9_973) {
            let draft = build_statutory_insurance_draft(&StatutoryInsuranceInput {
                pay_date: date!(2026 - 08 - 10),
                monthly_remuneration_won: gross,
                pension_standard_monthly_income_won: None,
                monthly_standard_hours: Some(209),
            })
            .unwrap();
            let won = component(&draft, ContributionCode::NationalPension)
                .employee_won
                .unwrap();
            assert_eq!(won % 10, 0, "보수월액 {gross} → 연금보험료 {won}");
        }
    }

    /// 산재 serves no figure this engine can ground, so it serves none at all.
    #[test]
    fn industrial_accident_emits_null_basis_and_null_rate_never_a_zero() {
        let draft = build_statutory_insurance_draft(&golden_case_input()).unwrap();
        let industrial = component(&draft, ContributionCode::IndustrialAccident);

        // The 사업종류별 요율 is in 고용노동부고시 제2025-91호's un-ingested 별지
        // and is per-employer. A 0 would read as "산재 costs nothing".
        assert_eq!(industrial.basis_won, None);
        assert_eq!(industrial.rate_num, None);
        assert_eq!(industrial.rate_den, None);
        assert_eq!(industrial.total_won, None);
        // The employee side is a REAL absence, not an unknown: 징수법 제13조제5항
        // charges the 사업주 and no 항 charges the worker. That distinction is
        // what `employer_only` carries.
        assert_eq!(industrial.employee_won, None);
        assert!(industrial.employer_only);
        // And no other component was quietly nulled along with it.
        for code in [
            ContributionCode::NationalPension,
            ContributionCode::HealthInsurance,
            ContributionCode::LongTermCare,
            ContributionCode::EmploymentUnemployment,
        ] {
            let other = component(&draft, code);
            assert!(other.basis_won.is_some(), "{code:?} basis");
            assert!(other.rate_num.is_some(), "{code:?} rate_num");
            assert!(other.rate_den.is_some(), "{code:?} rate_den");
        }
    }

    /// The order question 제5조 poses — 절사 then clamp, or clamp then 절사 —
    /// and the article that makes it unobservable.
    #[test]
    fn the_truncate_clamp_order_is_unobservable_only_because_every_band_bound_is_a_10_000_multiple()
    {
        // 제5조제1항제1호·제2호: 「이 경우 만원 미만은 반올림한다」. Every
        // 하한액/상한액 the 고시 may publish is therefore a 만원 multiple.
        for limit in national_pension_base_limits() {
            assert_eq!(
                limit.minimum_won % 10_000,
                0,
                "하한액 {} is not a 만원 multiple — 제5조제1항제1호 says it must be, \
                 and the 절사/clamp order stops being academic the moment it is not",
                limit.minimum_won
            );
            assert_eq!(
                limit.maximum_won % 10_000,
                0,
                "상한액 {}",
                limit.maximum_won
            );
        }

        // On such bounds the two candidate orders agree at every won, including
        // across both band edges. `national_pension_standard_income` implements
        // 제5항-then-제1항 and matches both.
        let limit = national_pension_limit_on(date!(2026 - 08 - 10)).unwrap();
        let edges = [limit.minimum_won, limit.maximum_won];
        for anchor in edges {
            for delta in -1_500_i64..=1_500 {
                let declared = anchor + delta;
                let truncate_then_clamp = (declared - declared.rem_euclid(1_000))
                    .clamp(limit.minimum_won, limit.maximum_won);
                let clamped = declared.clamp(limit.minimum_won, limit.maximum_won);
                let clamp_then_truncate = clamped - clamped.rem_euclid(1_000);
                assert_eq!(truncate_then_clamp, clamp_then_truncate, "at {declared}");
                assert_eq!(
                    national_pension_standard_income(declared, limit),
                    truncate_then_clamp,
                    "at {declared}"
                );
            }
        }

        // And the divergence is real, not theoretical: give the band a 하한액
        // that is NOT a 천원 multiple and the two orders disagree by 500원 at
        // 410,400. No lawful 고시 can produce that band — which is exactly why
        // the assertion above is the load-bearing one.
        let unlawful = MonthlyBaseLimit {
            minimum_won: 410_500,
            ..limit
        };
        let declared = 410_400_i64;
        let truncate_then_clamp =
            (declared - declared % 1_000).clamp(unlawful.minimum_won, unlawful.maximum_won);
        let clamped = declared.clamp(unlawful.minimum_won, unlawful.maximum_won);
        let clamp_then_truncate = clamped - clamped % 1_000;
        assert_eq!(truncate_then_clamp, 410_500);
        assert_eq!(clamp_then_truncate, 410_000);
        assert_ne!(truncate_then_clamp, clamp_then_truncate);
        // 제5항 governs: 410,400 < 하한액 → 기준소득월액 IS the 하한액.
        assert_eq!(
            national_pension_standard_income(declared, unlawful),
            410_500
        );
    }

    /// The measured cost of `Q-HALF-SHARE-ROUNDING-UNIT`, computed rather than
    /// asserted in prose. `docs/specs/payroll.md` quotes this number; the test
    /// exists so the doc can never drift from the engine again — the previous
    /// figure (4,500 / 50.0%) counted only 건강보험's half and silently omitted
    /// 장기요양's.
    #[test]
    fn the_agreement_gate_blocks_6_762_of_9_001_sampled_wages() {
        let mut blocked = 0_usize;
        let mut health_half_alone = 0_usize;
        let mut total = 0_usize;
        for remuneration in (1_000_000..=10_000_000).step_by(1_000) {
            total += 1;
            let draft = build_statutory_insurance_draft(&StatutoryInsuranceInput {
                pay_date: date!(2026 - 08 - 10),
                monthly_remuneration_won: remuneration,
                pension_standard_monthly_income_won: None,
                monthly_standard_hours: Some(209),
            })
            .unwrap();
            let health_blocked = component(&draft, ContributionCode::HealthInsurance)
                .blocked_by
                .is_some();
            let care_blocked = component(&draft, ContributionCode::LongTermCare)
                .blocked_by
                .is_some();
            if health_blocked || care_blocked {
                blocked += 1;
            }
            if health_blocked {
                health_half_alone += 1;
            }
        }
        assert_eq!(total, 9_001);
        assert_eq!(blocked, 6_762, "보수월액 1M–10M in 1,000원 steps");
        // The old doc figure, reproduced so its provenance is not a mystery: it
        // is 건강보험's half counted alone.
        assert_eq!(health_half_alone, 4_500);
    }

    #[test]
    fn release_gate_requires_validated_golden_case_and_professional_artifact() {
        let unvalidated = PayrollReleaseGateInput {
            rate_table_version: "KR-2026-official-rates-v1".to_string(),
            official_source_urls: vec![nps_source().url.to_string(), nhis_source().url.to_string()],
            golden_cases: vec![GoldenPayrollCase {
                case_id: "golden-fixture-unvalidated".to_string(),
                rate_table_version: "KR-2026-official-rates-v1".to_string(),
                professionally_validated: false,
                inputs: golden_case_inputs(),
                expected_total_employee_deductions_won: 373_300,
            }],
            professional_validation: None,
        };
        assert!(validate_release_gate(&unvalidated).is_err());

        let validated = PayrollReleaseGateInput {
            rate_table_version: "KR-2026-official-rates-v1".to_string(),
            official_source_urls: vec![
                nps_source().url.to_string(),
                nhis_source().url.to_string(),
                nts_source().url.to_string(),
                minimum_wage_source().url.to_string(),
            ],
            golden_cases: vec![GoldenPayrollCase {
                case_id: "golden-fixture-professionally-reviewed".to_string(),
                rate_table_version: "KR-2026-official-rates-v1".to_string(),
                professionally_validated: true,
                inputs: golden_case_inputs(),
                expected_total_employee_deductions_won: 373_300,
            }],
            professional_validation: Some(ProfessionalValidation {
                reviewer_kind: ProfessionalReviewerKind::LaborAttorney,
                reviewed_on: date!(2026 - 06 - 27),
                artifact_sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                    .to_string(),
                reviewer_reference: "licensed-reviewer-record".to_string(),
            }),
        };
        validate_release_gate(&validated).unwrap();
    }

    /// A gate input that satisfies every condition. Each test below mutates
    /// EXACTLY ONE field of it, so a failure names one defect.
    fn satisfied_release_gate() -> PayrollReleaseGateInput {
        PayrollReleaseGateInput {
            rate_table_version: "KR-2026-official-rates-v1".to_string(),
            official_source_urls: vec![nps_source().url.to_string(), nhis_source().url.to_string()],
            golden_cases: vec![GoldenPayrollCase {
                case_id: "GC-FIXTURE-A".to_string(),
                rate_table_version: "KR-2026-official-rates-v1".to_string(),
                professionally_validated: true,
                inputs: golden_case_inputs(),
                expected_total_employee_deductions_won: 373_300,
            }],
            professional_validation: Some(ProfessionalValidation {
                reviewer_kind: ProfessionalReviewerKind::LaborAttorney,
                reviewed_on: date!(2026 - 06 - 27),
                // Placeholder digest, and it STAYS a placeholder.
                artifact_sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                    .to_string(),
                reviewer_reference: "licensed-reviewer-record".to_string(),
            }),
        }
    }

    #[test]
    fn release_gate_rejects_a_golden_case_total_the_kernel_does_not_recompute() {
        // THE LOAD-BEARING TEST. Until this passes, a golden case is a stored
        // assertion that cannot fail: the professionals' signed figure is never
        // compared to anything the kernel produces. Only the EXPECTATION moves;
        // the declared inputs are the ones that really compute 373,302.
        let mut disagrees = satisfied_release_gate();
        disagrees.golden_cases[0].expected_total_employee_deductions_won = 373_301;

        let error = validate_release_gate(&disagrees).unwrap_err();

        assert_eq!(
            error.message,
            "golden case GC-FIXTURE-A expects total employee deductions 373301 \
             but the payroll kernel computed 373300"
        );
    }

    #[test]
    fn release_gate_accepts_a_golden_case_whose_declared_inputs_recompute_to_its_expected_total() {
        validate_release_gate(&satisfied_release_gate()).unwrap();
    }

    #[test]
    fn release_gate_rejects_a_blank_rate_table_version() {
        // ONE field, like every other test here. Blanking the golden case's
        // copy too was unreachable setup — the blank check returns before the
        // mismatch check is reached — and it cost the test its grip on that
        // precedence: with both blank they matched, so a reordering that put
        // the mismatch check first went unnoticed. Leaving the case's version
        // intact makes this record mismatched as well as blank, so the pinned
        // message now asserts WHICH refusal wins.
        let mut blank = satisfied_release_gate();
        blank.rate_table_version = "  ".to_string();

        assert_eq!(
            validate_release_gate(&blank).unwrap_err().message,
            "payroll rate table version is required"
        );
    }

    #[test]
    fn release_gate_rejects_an_empty_official_source_url_list() {
        let mut no_urls = satisfied_release_gate();
        no_urls.official_source_urls = vec![];

        assert_eq!(
            validate_release_gate(&no_urls).unwrap_err().message,
            "at least one official source URL is required"
        );
    }

    #[test]
    fn release_gate_rejects_a_case_marked_not_professionally_validated() {
        let mut unvalidated = satisfied_release_gate();
        unvalidated.golden_cases[0].professionally_validated = false;

        assert_eq!(
            validate_release_gate(&unvalidated).unwrap_err().message,
            "golden case GC-FIXTURE-A lacks professional validation"
        );
    }

    #[test]
    fn release_gate_rejects_an_absent_professional_validation() {
        let mut unsigned = satisfied_release_gate();
        unsigned.professional_validation = None;

        assert_eq!(
            validate_release_gate(&unsigned).unwrap_err().message,
            "노무사/세무사 professional validation is required"
        );
    }

    #[test]
    fn release_gate_rejects_an_artifact_digest_that_is_not_64_hex() {
        let mut not_hex = satisfied_release_gate();
        not_hex
            .professional_validation
            .as_mut()
            .unwrap()
            .artifact_sha256 = "z".repeat(64);

        assert_eq!(
            validate_release_gate(&not_hex).unwrap_err().message,
            "professional validation artifact_sha256 must be a 64-character hex digest"
        );
    }

    #[test]
    fn release_gate_names_the_case_whose_declared_inputs_cannot_be_recomputed() {
        // 2027-03-01 is inside the pension base-limit window (ends 2027-07-01)
        // but outside the contribution-rate window (ends 2027-01-01), so the
        // kernel refuses rather than mismatching. The gate must attribute that
        // refusal to a case_id; only the gate-owned prefix is pinned here, not
        // the inner kernel text.
        let mut unrecomputable = satisfied_release_gate();
        unrecomputable.golden_cases[0].inputs.pay_date = date!(2027 - 03 - 01);

        let message = validate_release_gate(&unrecomputable).unwrap_err().message;

        assert!(
            message.starts_with("golden case GC-FIXTURE-A could not be recomputed: "),
            "expected a case-attributed recomputation failure, got: {message}"
        );
    }

    #[test]
    fn release_gate_recomputes_every_golden_case_and_not_only_the_first() {
        // Line coverage cannot see this: the loop body's lines are already
        // executed by every single-case test. `for case in &input.golden_cases`
        // narrowed to `.first()` or `[0]` would keep them green while every
        // case after the first went unchecked — a signed batch where only the
        // first figure is ever re-derived.
        let mut two_cases = satisfied_release_gate();
        let mut second = two_cases.golden_cases[0].clone();
        second.case_id = "GC-FIXTURE-B".to_string();
        second.expected_total_employee_deductions_won = 373_301;
        two_cases.golden_cases.push(second);

        assert_eq!(
            validate_release_gate(&two_cases).unwrap_err().message,
            "golden case GC-FIXTURE-B expects total employee deductions 373301 \
             but the payroll kernel computed 373300"
        );
    }

    #[test]
    fn release_gate_rejects_a_gate_carrying_no_golden_case_at_all() {
        // The recomputation loop is VACUOUS over an empty list — it iterates
        // nothing and returns Ok. This check is therefore the only thing
        // between "the gate re-executes the signed figures" and "the gate
        // passed having compared nothing", and it had no test of its own.
        let mut caseless = satisfied_release_gate();
        caseless.golden_cases.clear();

        assert_eq!(
            validate_release_gate(&caseless).unwrap_err().message,
            "at least one payroll golden case is required"
        );
    }

    #[test]
    fn release_gate_rejects_a_case_signed_against_a_different_rate_table_version() {
        // Recomputation cannot catch this. `rate_table_version` is a label: it
        // reaches no kernel input, so a case signed against last year's table
        // still recomputes to its own expected total and passes the arithmetic.
        // Only this string compare notices.
        let mut mismatched = satisfied_release_gate();
        mismatched.golden_cases[0].rate_table_version = "KR-2025-official-rates-v1".to_string();

        assert_eq!(
            validate_release_gate(&mismatched).unwrap_err().message,
            "golden case GC-FIXTURE-A uses mismatched rate table version"
        );
    }

    #[test]
    fn release_gate_rejects_a_blank_reviewer_reference() {
        // The remaining unexercised pre-existing condition. A 노무사/세무사
        // sign-off whose reviewer cannot be identified is an anonymous one.
        let mut anonymous = satisfied_release_gate();
        anonymous
            .professional_validation
            .as_mut()
            .unwrap()
            .reviewer_reference = "   ".to_string();

        assert_eq!(
            validate_release_gate(&anonymous).unwrap_err().message,
            "professional validation reviewer reference is required"
        );
    }

    #[test]
    fn release_gate_recomputes_a_case_on_its_declared_pension_standard_monthly_income() {
        // `pension_standard_monthly_income_won` is the one optional kernel
        // input, and it is the only one that changes a figure without changing
        // the gross. 2,000,000 sits inside the 2026-06 base window
        // (400,000..6,370,000), so the clamp does not erase it: the 국민연금
        // line drops by 47,500 and the total with it. A gate that ignored the
        // declared basis would compute 373,302 here.
        let mut capped = satisfied_release_gate();
        capped.golden_cases[0]
            .inputs
            .pension_standard_monthly_income_won = Some(2_000_000);
        capped.golden_cases[0].expected_total_employee_deductions_won = 325_800;

        validate_release_gate(&capped).unwrap();

        // ...and the gross-based figure is now the WRONG answer for this case.
        capped.golden_cases[0].expected_total_employee_deductions_won = 373_300;
        assert_eq!(
            validate_release_gate(&capped).unwrap_err().message,
            "golden case GC-FIXTURE-A expects total employee deductions 373300 \
             but the payroll kernel computed 325800"
        );
    }

    #[test]
    fn builds_severance_pay_from_moel_average_wage_formula() {
        // Ordinary daily wage (90,000) is BELOW the average daily wage (100,000),
        // so the average-wage path governs and the historical figures must be
        // byte-for-byte unchanged.
        let draft = build_severance_pay_draft(SeverancePayInput {
            hire_date: date!(2024 - 01 - 01),
            exit_date: date!(2026 - 06 - 30),
            average_wage_period_start: date!(2026 - 04 - 01),
            average_wage_period_end: date!(2026 - 06 - 30),
            average_wage_calendar_days: 91,
            average_wage_total_won: 9_100_000,
            ordinary_daily_wage_won: 90_000,
        })
        .unwrap();

        assert_eq!(draft.service_days, 912);
        assert_eq!(draft.average_daily_wage_milliwon, 100_000_000);
        // max(average 100,000, ordinary 90,000) => average governs.
        assert_eq!(draft.statutory_daily_wage_milliwon, 100_000_000);
        assert_eq!(draft.statutory_30_day_wage_won, 3_000_000);
        assert_eq!(draft.severance_pay_won, 7_495_890);
        assert_eq!(draft.source, moel_retirement_pay_source());
    }

    #[test]
    fn ordinary_wage_floor_governs_when_three_month_window_is_depressed() {
        // Absence→exit population: unpaid leave halved the 3-month wage total, so
        // the average daily wage collapses to 50,000/day. The employee's ordinary
        // daily wage is still 100,000. 통상임금 floor must govern and roughly double
        // the severance versus the average-wage-only figure.
        let draft = build_severance_pay_draft(SeverancePayInput {
            hire_date: date!(2024 - 01 - 01),
            exit_date: date!(2026 - 06 - 30),
            average_wage_period_start: date!(2026 - 04 - 01),
            average_wage_period_end: date!(2026 - 06 - 30),
            average_wage_calendar_days: 91,
            average_wage_total_won: 4_550_000, // depressed window: 50,000/day
            ordinary_daily_wage_won: 100_000,
        })
        .unwrap();

        // Average path would have produced only the depressed figure.
        assert_eq!(draft.average_daily_wage_milliwon, 50_000_000);
        let average_only_severance = 4_550_000_i128 * 30 * 912 / 91 / 365;
        assert_eq!(average_only_severance, 3_747_945);

        // Ordinary wage floor governs: max(50,000, 100,000) = 100,000.
        assert_eq!(draft.statutory_daily_wage_milliwon, 100_000_000);
        assert_eq!(draft.statutory_30_day_wage_won, 3_000_000);
        assert_eq!(draft.severance_pay_won, 7_495_890);
        assert!(
            draft.severance_pay_won > i64::try_from(average_only_severance).unwrap(),
            "ordinary-wage floor must exceed the depressed average-wage figure"
        );
    }

    #[test]
    fn severance_pay_refuses_short_service_or_missing_wage_basis() {
        let short_service = build_severance_pay_draft(SeverancePayInput {
            hire_date: date!(2026 - 01 - 01),
            exit_date: date!(2026 - 06 - 30),
            average_wage_period_start: date!(2026 - 04 - 01),
            average_wage_period_end: date!(2026 - 06 - 30),
            average_wage_calendar_days: 91,
            average_wage_total_won: 9_100_000,
            ordinary_daily_wage_won: 100_000,
        });
        assert!(short_service.is_err());

        let missing_wage = build_severance_pay_draft(SeverancePayInput {
            hire_date: date!(2024 - 01 - 01),
            exit_date: date!(2026 - 06 - 30),
            average_wage_period_start: date!(2026 - 04 - 01),
            average_wage_period_end: date!(2026 - 06 - 30),
            average_wage_calendar_days: 91,
            average_wage_total_won: 0,
            ordinary_daily_wage_won: 100_000,
        });
        assert!(missing_wage.is_err());
    }

    #[test]
    fn severance_pay_refuses_missing_ordinary_wage_floor() {
        // FAIL LOUD: absent/non-positive ordinary wage must be rejected, never
        // silently degraded to average-wage-only.
        let missing_ordinary = build_severance_pay_draft(SeverancePayInput {
            hire_date: date!(2024 - 01 - 01),
            exit_date: date!(2026 - 06 - 30),
            average_wage_period_start: date!(2026 - 04 - 01),
            average_wage_period_end: date!(2026 - 06 - 30),
            average_wage_calendar_days: 91,
            average_wage_total_won: 9_100_000,
            ordinary_daily_wage_won: 0,
        });
        assert!(missing_ordinary.is_err());
        assert!(format!("{:?}", missing_ordinary.err().unwrap()).contains("ordinary daily wage"));
    }

    fn line_amount(draft: &PayrollDraft, code: DeductionCode) -> i64 {
        draft
            .lines
            .iter()
            .find(|line| line.code == code)
            .unwrap()
            .amount_won
    }

    // ─────────────────────────────────────────────────────────────────────
    // GC-2026-07-KR-MONTHLY-A — the golden case.
    //
    // 월급제 정규직, 완전출근, 무공제사유. 보수월액 3,000,000 is chosen so every
    // component lands on a 10원 boundary, which makes the case independent of
    // the two UNRESOLVED rounding questions: a golden case whose answer depends
    // on an unanswered question is not a golden case.
    //
    // If the engine disagrees with these numbers, the ENGINE is wrong.
    // ─────────────────────────────────────────────────────────────────────

    fn golden_case_input() -> StatutoryInsuranceInput {
        StatutoryInsuranceInput {
            pay_date: date!(2026 - 08 - 10),
            monthly_remuneration_won: 3_000_000,
            pension_standard_monthly_income_won: None,
            monthly_standard_hours: Some(209),
        }
    }

    fn component(draft: &StatutoryInsuranceDraft, code: ContributionCode) -> &StatutoryComponent {
        draft
            .components
            .iter()
            .find(|component| component.code == code)
            .unwrap()
    }

    #[test]
    fn golden_case_gc_2026_07_kr_monthly_a_matches_the_hand_calculation_to_the_won() {
        let draft = build_statutory_insurance_draft(&golden_case_input()).unwrap();

        // 1. 기준소득월액 = clamp(3,000,000, 410,000, 6,590,000) — unbound.
        let pension = component(&draft, ContributionCode::NationalPension);
        assert_eq!(pension.basis_won, Some(3_000_000));
        // 2. 3,000,000 × 475/10,000 = 142,500.0 — 단수 없음.
        assert_eq!(pension.employee_won, Some(142_500));

        // 3. 3,000,000 × 719/10,000 = 215,700 → trunc10 215,700 → clamp 미구속.
        let health = component(&draft, ContributionCode::HealthInsurance);
        assert_eq!(health.total_won, Some(215_700));
        // 4. 215,700 × 50/100 = 107,850 — 10원 배수라 trunc10 ≡ round10.
        assert_eq!(health.employee_won, Some(107_850));

        // 5. 215,700 × 9,448/71,900 = 28,344 EXACTLY → trunc10 → 28,340.
        //    This is the one place 단수 절사 actually bites.
        let ltc = component(&draft, ContributionCode::LongTermCare);
        assert_eq!(ltc.basis, ContributionBasis::HealthInsurancePremium);
        assert_eq!(ltc.basis_won, Some(215_700));
        assert_eq!(ltc.total_won, Some(28_340));
        // 6. 28,340 × 50/100 = 14,170.
        assert_eq!(ltc.employee_won, Some(14_170));

        // 7. 3,000,000 × 9/1,000 = 27,000.0 — 단수 없음.
        let employment = component(&draft, ContributionCode::EmploymentUnemployment);
        assert_eq!(employment.employee_won, Some(27_000));
        // The exact won is an ASSUMPTION, not a rule, and the type says so.
        // `Resolved` here would name 시행령 제12조제1항제2호 — which sets a rate
        // and prescribes no 단수 — as the instrument settling the rounding.
        assert_eq!(employment.total_rounding_unit, "EXACT_WON");

        // 8. 산재 — 사업주 전액 부담. STATED absent, never a zero deduction.
        let iacc = component(&draft, ContributionCode::IndustrialAccident);
        assert!(iacc.employer_only);
        assert_eq!(iacc.employee_won, None);
        assert!(iacc.instrument.article_ko.contains("제13조제5항"));

        // Neither 징수법 row cites an instrument for its 단수: none exists.
        // 국민건강보험법 제107조, which 산재 used to cite, does not reach 징수법
        // 보험료 by any 준용.
        for code in [
            ContributionCode::EmploymentUnemployment,
            ContributionCode::IndustrialAccident,
        ] {
            let rounding = contribution_rate_on(code, golden_case_input().pay_date)
                .unwrap()
                .total_rounding;
            assert!(
                matches!(
                    rounding,
                    Rounding::Assumed {
                        unit: RoundingUnit::ExactWon,
                        ..
                    }
                ),
                "{code:?}: 단수 근거가 없는데 인용하는 형태로 되돌아갔다 ({rounding:?})"
            );
        }
        // And the EMPLOYER total is absent, not 0. The 사업종류별 요율 lives in
        // 고시 제2025-91호's unparsed 별지, so "산재 premium = 0원" would be a
        // false statement — the exact silently-zero failure this engine refuses.
        assert_eq!(iacc.total_won, None);

        // 4대보험 공제계 / 잔액.
        assert_eq!(draft.total_employee_insurance_won, Some(291_520));
        assert_eq!(draft.remainder_after_insurance_won, Some(2_708_480));

        // 최저임금: 3,000,000 ÷ 209 = 14,354 ≥ 10,320.
        assert_eq!(draft.minimum_wage.effective_hourly_won, Some(14_354));
        assert_eq!(draft.minimum_wage.passes, Some(true));
        assert_eq!(draft.minimum_wage.hourly_floor_won, 10_320);
        assert_eq!(draft.minimum_wage.monthly_209h_floor_won, 2_156_880);
    }

    #[test]
    fn golden_case_refuses_withholding_and_is_not_issuable() {
        // The deferral is EXPLICIT, in the draft, naming the instrument — never
        // a silent zero. 차인지급액 is therefore unavailable, which is exactly
        // why the draft cannot be issued.
        let draft = build_statutory_insurance_draft(&golden_case_input()).unwrap();

        assert!(!draft.issuable);
        assert!(
            draft
                .blockers
                .contains(&"WITHHOLDING_NOT_COMPUTED".to_owned())
        );

        let income_tax = draft
            .not_computed
            .iter()
            .find(|row| row.code == DeductionCode::IncomeTax)
            .unwrap();
        assert!(
            income_tax
                .instrument
                .name_ko
                .contains("소득세법 시행령 별표 2")
        );
        // The version anchor is the semantic 별표HWP파일명, not the unstable flSeq.
        assert!(
            income_tax
                .instrument
                .article_ko
                .contains("law0039562026052236343KC_000200E_20260701.hwp")
        );
        assert!(!income_tax.instrument.article_ko.contains("flSeq"));

        let local = draft
            .not_computed
            .iter()
            .find(|row| row.code == DeductionCode::LocalIncomeTax)
            .unwrap();
        assert!(local.instrument.article_ko.contains("제103조의13제1항"));

        // And no zero-amount tax line is smuggled into the components.
        assert!(
            !draft
                .components
                .iter()
                .any(|c| c.label_ko.contains("소득세"))
        );
    }

    #[test]
    fn the_naive_direct_rate_on_remuneration_is_two_won_wrong_for_long_term_care() {
        // The defect this increment exists to fix. The pre-existing model was
        // `employee_ppm: 4_724, basis: MonthlyRemuneration`:
        //   3,000,000 × 4,724/1,000,000 = 14,172.
        // The statutory chain (제9조제1항: basis is the 건강보험료액) gives 14,170.
        // Two won, on the simplest possible employee — which is why it survived.
        let naive = 3_000_000_i64 * 4_724 / 1_000_000;
        assert_eq!(naive, 14_172);

        let draft = build_statutory_insurance_draft(&golden_case_input()).unwrap();
        assert_eq!(
            component(&draft, ContributionCode::LongTermCare).employee_won,
            Some(14_170)
        );
        assert_ne!(
            component(&draft, ContributionCode::LongTermCare).employee_won,
            Some(naive)
        );
    }

    #[test]
    fn golden_case_survives_the_2026_11_27_ratio_amendment() {
        // From 2026-11-27 제9조제1항's 반올림 clause enters force and the ratio
        // becomes 1,314/10,000: 215,700 × 1,314 / 10,000 = 28,342 → trunc10
        // 28,340. Identical. This also proves the engine reads the effective-date
        // slice rather than the latest promulgated text — `target=law` would have
        // handed it the 2026-11-27 wording four months early.
        let mut december = golden_case_input();
        december.pay_date = date!(2026 - 12 - 10);
        let draft = build_statutory_insurance_draft(&december).unwrap();

        let ltc = component(&draft, ContributionCode::LongTermCare);
        assert_eq!((ltc.rate_num, ltc.rate_den), (Some(1_314), Some(10_000)));
        assert_eq!(ltc.total_won, Some(28_340));
        assert_eq!(ltc.employee_won, Some(14_170));

        let august = build_statutory_insurance_draft(&golden_case_input()).unwrap();
        assert_eq!(
            component(&august, ContributionCode::LongTermCare).rate_num,
            Some(9_448)
        );
        assert_eq!(
            draft.total_employee_insurance_won,
            august.total_employee_insurance_won
        );
    }

    #[test]
    fn the_agreement_gate_blocks_rather_than_picking_a_rounding_default() {
        // 보수월액 1,000,010 → 건강보험 총 = 71,900.719 → trunc10 = 71,900.
        // Half = 35,950 → 10원 배수 → agree. Find one that does NOT agree and
        // prove the component is withheld, not guessed.
        let disagreeing = (1_000_000..1_100_000)
            .find(|gross| {
                let total = (i128::from(*gross) * 719 / 10_000 / 10) * 10;
                (total / 2) % 10 != 0
            })
            .expect("a 10원-boundary miss must exist in this range");

        let draft = build_statutory_insurance_draft(&StatutoryInsuranceInput {
            pay_date: date!(2026 - 08 - 10),
            monthly_remuneration_won: disagreeing,
            pension_standard_monthly_income_won: None,
            monthly_standard_hours: Some(209),
        })
        .unwrap();

        let health = component(&draft, ContributionCode::HealthInsurance);
        assert!(health.total_won.is_some(), "the TOTAL is resolved");
        assert_eq!(health.employee_won, None, "the HALF is not");
        assert_eq!(health.blocked_by, Some(QUESTION_HALF_SHARE_ROUNDING));
        assert_eq!(draft.total_employee_insurance_won, None);
        assert_eq!(draft.remainder_after_insurance_won, None);
        assert!(!draft.issuable);
        assert!(
            draft
                .blockers
                .iter()
                .any(|blocker| blocker.contains(QUESTION_HALF_SHARE_ROUNDING))
        );

        // And the wired run path REFUSES rather than emitting a guessed won.
        let refused = build_line_calculation(LineCalculationInput {
            pay_date: date!(2026 - 08 - 10),
            gross_won: disagreeing,
            pension_standard_monthly_income_won: None,
            tax_row: VerifiedNtsTaxRow {
                table_version: "v1".to_owned(),
                monthly_income_tax_won: 0,
                local_income_tax_won: 0,
            },
        });
        assert!(
            refused
                .unwrap_err()
                .message
                .contains(QUESTION_HALF_SHARE_ROUNDING)
        );
    }

    #[test]
    fn health_premium_floor_binds_for_a_low_paid_part_timer() {
        // 「월별 건강보험료액의 상한과 하한에 관한 고시」 제2025-222호: the 하한
        // 20,160원 binds below 보수월액 280,389원.
        let draft = build_statutory_insurance_draft(&StatutoryInsuranceInput {
            pay_date: date!(2026 - 08 - 10),
            monthly_remuneration_won: 200_000,
            pension_standard_monthly_income_won: None,
            monthly_standard_hours: Some(20),
        })
        .unwrap();

        let health = component(&draft, ContributionCode::HealthInsurance);
        // Unclamped this would be 200,000 × 719/10,000 = 14,380.
        assert_eq!(health.total_won, Some(20_160));
        assert_eq!(health.employee_won, Some(10_080));
        // 장기요양's basis follows the CLAMPED health premium, not 보수월액.
        assert_eq!(
            component(&draft, ContributionCode::LongTermCare).basis_won,
            Some(20_160)
        );
        // 기준소득월액 하한 410,000 also binds: 410,000 × 475/10,000 = 19,475
        // → 국민연금법 제117조 절사 → 19,470.
        assert_eq!(
            component(&draft, ContributionCode::NationalPension).employee_won,
            Some(19_470)
        );
    }

    #[test]
    fn minimum_wage_check_fails_a_sub_floor_contract_and_refuses_to_pass_without_hours() {
        let below = build_statutory_insurance_draft(&StatutoryInsuranceInput {
            pay_date: date!(2026 - 08 - 10),
            monthly_remuneration_won: 2_000_000,
            pension_standard_monthly_income_won: None,
            monthly_standard_hours: Some(209),
        })
        .unwrap();
        // 2,000,000 ÷ 209 = 9,569 < 10,320.
        assert_eq!(below.minimum_wage.passes, Some(false));
        assert!(below.blockers.iter().any(|b| b.contains("최저임금 미달")));

        // No hours => NOT comparable, and "not comparable" is not a pass.
        let unknown = build_statutory_insurance_draft(&StatutoryInsuranceInput {
            monthly_standard_hours: None,
            ..golden_case_input()
        })
        .unwrap();
        assert_eq!(unknown.minimum_wage.passes, None);
        assert!(unknown.blockers.iter().any(|b| b.contains("비교 불가")));
    }

    #[test]
    fn long_term_care_reads_the_computed_health_premium_not_remuneration() {
        // The dependency is structural, not statement order: every LTC row's
        // basis must be HealthInsurancePremium, and its computed basis_won must
        // equal the health component's own total.
        for pay_date in [date!(2026 - 08 - 10), date!(2026 - 12 - 10)] {
            for gross in [1_000_000_i64, 3_000_000, 6_000_000] {
                let draft = build_statutory_insurance_draft(&StatutoryInsuranceInput {
                    pay_date,
                    monthly_remuneration_won: gross,
                    pension_standard_monthly_income_won: None,
                    monthly_standard_hours: Some(209),
                })
                .unwrap();
                let health_total = component(&draft, ContributionCode::HealthInsurance).total_won;
                let ltc = component(&draft, ContributionCode::LongTermCare);
                assert_eq!(ltc.basis, ContributionBasis::HealthInsurancePremium);
                assert_eq!(ltc.basis_won, health_total);
                assert_ne!(ltc.basis_won, Some(gross));
            }
        }
    }

    // ── Property tests. Deterministic sweep, no proptest dependency. ──

    #[test]
    fn property_net_never_exceeds_gross_and_deductions_are_never_negative() {
        for gross in (0..12_000_000).step_by(9_973) {
            let draft = build_statutory_insurance_draft(&StatutoryInsuranceInput {
                pay_date: date!(2026 - 08 - 10),
                monthly_remuneration_won: gross,
                pension_standard_monthly_income_won: None,
                monthly_standard_hours: Some(209),
            })
            .unwrap();

            for component in &draft.components {
                if let Some(total) = component.total_won {
                    assert!(total >= 0, "gross {gross}: negative total premium");
                }
                if let Some(employee) = component.employee_won {
                    assert!(employee >= 0, "gross {gross}: negative employee share");
                }
                if component.employer_only {
                    assert!(component.employee_won.is_none());
                }
            }

            if let Some(total) = draft.total_employee_insurance_won {
                assert!(total >= 0, "gross {gross}: negative deduction total");
                let remainder = draft.remainder_after_insurance_won.unwrap();
                assert!(remainder <= gross, "gross {gross}: remainder exceeds gross");
                assert_eq!(remainder, gross - total, "gross {gross}: remainder drift");
            }
        }
    }

    #[test]
    fn property_component_lines_sum_to_the_stated_total() {
        for gross in (0..12_000_000).step_by(7_919) {
            let draft = build_statutory_insurance_draft(&StatutoryInsuranceInput {
                pay_date: date!(2026 - 08 - 10),
                monthly_remuneration_won: gross,
                pension_standard_monthly_income_won: None,
                monthly_standard_hours: Some(209),
            })
            .unwrap();

            let summed: Option<i64> = draft
                .components
                .iter()
                .filter(|component| !component.employer_only)
                .try_fold(0_i64, |total, component| {
                    component.employee_won.map(|amount| total + amount)
                });
            assert_eq!(
                summed, draft.total_employee_insurance_won,
                "gross {gross}: lines do not sum to the total"
            );
        }
    }

    #[test]
    fn property_full_payslip_lines_sum_to_totals_and_net_never_exceeds_gross() {
        // Same three properties over the wired `build_line_calculation` path,
        // which is what `calculate_run_in_tx` persists.
        for gross in (0..12_000_000).step_by(11_113) {
            let Ok(calc) = build_line_calculation(LineCalculationInput {
                pay_date: date!(2026 - 08 - 10),
                gross_won: gross,
                pension_standard_monthly_income_won: None,
                tax_row: VerifiedNtsTaxRow {
                    table_version: "property-sweep".to_owned(),
                    monthly_income_tax_won: 0,
                    local_income_tax_won: 0,
                },
            }) else {
                // The rounding gate refused; nothing to assert about amounts.
                continue;
            };

            assert!(calc.lines.iter().all(|line| line.amount_won >= 0));
            assert_eq!(
                calc.lines.iter().map(|line| line.amount_won).sum::<i64>(),
                calc.total_employee_deductions_won
            );
            assert_eq!(calc.net_won, gross - calc.total_employee_deductions_won);
            assert!(calc.net_won <= calc.gross_won);
        }
    }

    #[test]
    fn the_withholding_table_version_is_resolved_per_pay_date_across_all_of_2026() {
        // Four 별표 slices in one calendar year. A missing slice would make the
        // whole draft error for that month, so the coverage is the assertion.
        for (pay_date, expected_hwp) in [
            (
                date!(2026 - 01 - 15),
                "law0039562025123035947KC_000200E_20260102.hwp",
            ),
            (
                date!(2026 - 02 - 28),
                "law0039562026022736129KC_000200E_20260227.hwp",
            ),
            (
                date!(2026 - 06 - 15),
                "law0039562026042336276KC_000200E_20260423.hwp",
            ),
            (
                date!(2026 - 08 - 10),
                "law0039562026052236343KC_000200E_20260701.hwp",
            ),
        ] {
            let instrument = withholding_table_instrument_on(pay_date).unwrap();
            assert!(
                instrument.article_ko.contains(expected_hwp),
                "{pay_date}: expected 별표 {expected_hwp}, got {}",
                instrument.article_ko
            );
            // And a whole-2026 draft never fails for want of a 별표 version.
            build_statutory_insurance_draft(&StatutoryInsuranceInput {
                pay_date,
                ..golden_case_input()
            })
            .unwrap();
        }

        // Note 3's 자녀세액공제 changed at 2026-02-27 while the 646 × 11 grid
        // stayed byte-identical. Pin BOTH figures so an ingest that reads only
        // the grid cannot pass: it would withhold 8,330원/month too much.
        assert!(
            withholding_table_instrument_on(date!(2026 - 01 - 15))
                .unwrap()
                .article_ko
                .contains("12,500원")
        );
        assert!(
            withholding_table_instrument_on(date!(2026 - 02 - 28))
                .unwrap()
                .article_ko
                .contains("20,830원")
        );
    }

    #[test]
    fn every_rate_row_carries_an_instrument_article_and_effective_date() {
        // Release-gate condition 1 in executable form: no agency explainer page
        // may stand in for the document that sets a number.
        for rate in statutory_contribution_rates() {
            assert!(!rate.instrument.name_ko.trim().is_empty());
            assert!(!rate.instrument.article_ko.trim().is_empty());
            assert!(!rate.instrument.promulgation_ko.trim().is_empty());
            assert!(rate.instrument.url.starts_with("https://www.law.go.kr/"));
            assert!(!rate.provenance.trim().is_empty());
            assert!(rate.rate_den > 0);
        }
        for limit in national_pension_base_limits() {
            assert!(limit.instrument.promulgation_ko.contains("보건복지부고시"));
        }
        assert!(
            minimum_wage_rates()[0]
                .instrument
                .promulgation_ko
                .contains("고용노동부고시 제2025-47호")
        );
        // The dead anchor is gone: total.comwel.or.kr answered HTTP 400.
        assert!(
            !statutory_contribution_rates()
                .iter()
                .any(|rate| rate.instrument.url.contains("comwel.or.kr"))
        );
    }

    /// The kernel half of `payroll_statutory_rates`'
    /// `..._not_backdated_before_instrument` CHECK.
    ///
    /// The DB constraint cannot see this table, and the register-agreement test
    /// compares every citation to the PAY DATE — which is why three rows passed
    /// while citing a document enforced after their own period began.
    #[test]
    fn no_rate_row_is_in_force_before_the_instrument_that_sets_it() {
        for rate in statutory_contribution_rates() {
            // EVERY instrument the row carries, not just the rate one. Reading
            // only `rate.instrument` is how 제107조 and 제76조제1항 sat at
            // 시행 2026-01-02 on rows in force from 2026-01-01 while both this
            // test and the DB CHECK stayed green: neither could see them.
            let rounding_instrument = match rate.total_rounding {
                Rounding::Resolved { instrument, .. } => Some(instrument),
                Rounding::Assumed { .. } | Rounding::Unresolved { .. } => None,
            };
            let share_rounding_instrument = match rate.employee_share {
                ShareRule::Half {
                    rounding: Rounding::Resolved { instrument, .. },
                } => Some(instrument),
                _ => None,
            };
            for instrument in [
                Some(rate.instrument),
                rounding_instrument,
                share_rounding_instrument,
                rate.share_instrument,
                rate.clamp.map(|clamp| clamp.instrument),
            ]
            .into_iter()
            .flatten()
            {
                assert!(
                    instrument.enforced_on <= rate.period.from,
                    "{:?}: 시행 {}부터인 행이 {}에 시행된 문서({} {})를 인용한다",
                    rate.code,
                    rate.period.from,
                    instrument.enforced_on,
                    instrument.promulgation_ko,
                    instrument.article_ko,
                );
            }
        }
        for (period, instrument) in simplified_withholding_table_instruments() {
            assert!(
                instrument.enforced_on <= period.from,
                "별표 2 슬라이스 {}: {} 시행 문서({})를 인용한다",
                period.from,
                instrument.enforced_on,
                instrument.promulgation_ko,
            );
        }
        for limit in national_pension_base_limits() {
            assert!(limit.instrument.enforced_on <= limit.period.from);
        }
        for wage in minimum_wage_rates() {
            assert!(wage.instrument.enforced_on <= wage.period.from);
        }
    }

    /// The same rule, applied to what the engine actually HANDS OUT.
    ///
    /// The loop above walks the rate table, so it can only see instruments that
    /// hang off a rate row. `local_income_tax_instrument()` has no row — 지방세법
    /// is not a 요율 — and is returned unconditionally on every draft, which is
    /// how it came to sit at 시행 2026-07-01 while both that loop and the DB's
    /// `CHECK (effective_from >= enforced_on)` stayed green through a January
    /// pay date. This test needs no enumeration: it sweeps the year, builds the
    /// draft, and checks every instrument in the value returned. A new
    /// unconditional instrument is covered the day it is added.
    #[test]
    fn no_instrument_the_draft_emits_post_dates_the_pay_date_it_is_emitted_on() {
        let mut days_covered = 0;
        let mut pay_date = date!(2026 - 01 - 01);
        while pay_date < date!(2027 - 01 - 01) {
            // Dates the engine refuses outright emit nothing, so there is
            // nothing to check; `days_covered` keeps that from passing vacuously.
            if let Ok(draft) = build_statutory_insurance_draft(&StatutoryInsuranceInput {
                pay_date,
                monthly_remuneration_won: 3_000_000,
                pension_standard_monthly_income_won: None,
                monthly_standard_hours: Some(209),
            }) {
                days_covered += 1;
                let emitted = draft
                    .components
                    .iter()
                    .flat_map(|component| [Some(component.instrument), component.share_instrument])
                    .chain(draft.not_computed.iter().map(|row| Some(row.instrument)))
                    .chain(std::iter::once(Some(draft.minimum_wage.instrument)))
                    .flatten();
                for instrument in emitted {
                    assert!(
                        instrument.enforced_on <= pay_date,
                        "지급일 {pay_date}의 초안이 {}에 시행된 문서({} {} {})를 인용한다",
                        instrument.enforced_on,
                        instrument.name_ko,
                        instrument.promulgation_ko,
                        instrument.article_ko,
                    );
                }
            }
            pay_date = pay_date.next_day().expect("2026 has a next day");
        }
        assert!(
            days_covered >= 300,
            "sweep only produced {days_covered} drafts — it cannot be proving much"
        );
    }
}
