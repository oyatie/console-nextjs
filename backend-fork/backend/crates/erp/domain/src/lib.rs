//! ERP domain kernel.
//!
//! Pure double-entry, VAT, AR/AP, inventory, and e-tax invoice relay guardrails.
//! This crate must not issue invoices, talk to HomeTax/NTS, or infer regulated
//! accounting behavior without a source-versioned rule and 세무사 validation.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_kernel_core::KernelError;
use time::Date;
use time::macros::date;

const BPS_DENOMINATOR: i128 = 10_000;

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
pub enum AccountKind {
    Asset,
    Liability,
    Equity,
    Revenue,
    Expense,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountCode {
    Cash,
    AccountsReceivable,
    SalesRevenue,
    VatPayable,
    AccountsPayable,
    Inventory,
    VatReceivable,
    OperatingExpense,
    CostOfGoodsSold,
}

impl AccountCode {
    #[must_use]
    pub const fn kind(self) -> AccountKind {
        match self {
            Self::Cash | Self::AccountsReceivable | Self::Inventory | Self::VatReceivable => {
                AccountKind::Asset
            }
            Self::VatPayable | Self::AccountsPayable => AccountKind::Liability,
            Self::SalesRevenue => AccountKind::Revenue,
            Self::OperatingExpense | Self::CostOfGoodsSold => AccountKind::Expense,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebitCredit {
    Debit,
    Credit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceDocumentKind {
    SalesTaxInvoice,
    CustomerReceipt,
    VendorInvoice,
    VendorPayment,
    InventoryMovement,
    Adjustment,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceDocumentRef {
    pub kind: SourceDocumentKind,
    pub document_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalLine {
    pub account: AccountCode,
    pub side: DebitCredit,
    pub amount_won: i64,
    pub memo: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntry {
    pub occurred_on: Date,
    pub description: String,
    pub source_document: SourceDocumentRef,
    pub lines: Vec<JournalLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedJournalEntry {
    pub entry: JournalEntry,
    pub debit_total_won: i64,
    pub credit_total_won: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VatRate {
    pub period: EffectivePeriod,
    /// Basis points. 1,000 bps = 10%.
    pub rate_bps: u32,
    pub source: OfficialSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VatHalfYear {
    First,
    Second,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VatTaxPeriod {
    pub year: i32,
    pub half: VatHalfYear,
    pub period: EffectivePeriod,
    pub source: OfficialSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaxInvoiceDraft {
    pub invoice_id: String,
    pub issue_on: Date,
    pub supplier_business_registration_no: String,
    pub recipient_business_registration_no: String,
    pub supply_amount_won: i64,
    pub vat_amount_won: i64,
    pub total_amount_won: i64,
    pub source: OfficialSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SalesInvoiceInput {
    pub invoice_id: String,
    pub issue_on: Date,
    pub supplier_business_registration_no: String,
    pub recipient_business_registration_no: String,
    pub supply_amount_won: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurchaseInvoiceInput {
    pub invoice_id: String,
    pub issue_on: Date,
    pub supply_amount_won: i64,
    pub vat_amount_won: i64,
    pub cost_target: PurchaseCostTarget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PurchaseCostTarget {
    Inventory,
    OperatingExpense,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentInput {
    pub payment_id: String,
    pub paid_on: Date,
    pub amount_won: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryConsumptionInput {
    pub movement_id: String,
    pub occurred_on: Date,
    pub item_id: String,
    pub work_order_id: String,
    pub quantity_on_hand_before: i64,
    pub quantity_consumed: i64,
    pub unit_cost_won: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryConsumptionResult {
    pub quantity_on_hand_after: i64,
    pub cost_won: i64,
    pub journal: ValidatedJournalEntry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElectronicTaxInvoiceRelayMode {
    HomeTaxManual,
    NtsRegisteredErpSystem,
    NtsRegisteredAspGateway,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElectronicTaxInvoiceRelayReadiness {
    pub mode: ElectronicTaxInvoiceRelayMode,
    pub official_protocol_verified: bool,
    pub nts_system_registration_present: bool,
    pub standard_certification_present: bool,
    pub production_credentials_present: bool,
    pub signing_certificate_present: bool,
    pub dedicated_inbox_registered: bool,
    pub source: OfficialSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElectronicTaxInvoiceRelayEnvelope {
    pub invoice_id: String,
    pub issue_on: Date,
    pub supply_amount_won: i64,
    pub vat_amount_won: i64,
    pub total_amount_won: i64,
    pub relay_mode: ElectronicTaxInvoiceRelayMode,
    pub source_url: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfessionalReviewerKind {
    TaxAccountant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountingGoldenCase {
    pub case_id: String,
    pub rate_table_version: String,
    pub professionally_validated: bool,
    pub expected_debit_total_won: i64,
    pub expected_credit_total_won: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfessionalValidation {
    pub reviewer_kind: ProfessionalReviewerKind,
    pub reviewed_on: Date,
    pub artifact_sha256: String,
    pub reviewer_reference: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountingReleaseGateInput {
    pub rate_table_version: String,
    pub official_source_urls: Vec<String>,
    pub golden_cases: Vec<AccountingGoldenCase>,
    pub professional_validation: Option<ProfessionalValidation>,
}

#[must_use]
pub const fn erp_sources_verified_on() -> Date {
    date!(2026 - 06 - 27)
}

#[must_use]
pub const fn nts_vat_source() -> OfficialSource {
    OfficialSource {
        authority: "국세청",
        title: "부가가치세 세율",
        url: "https://www.nts.go.kr/nts/cm/cntnts/cntntsView.do?cntntsId=7696&mi=2275",
        retrieved_on: erp_sources_verified_on(),
    }
}

#[must_use]
pub const fn nts_vat_reporting_source() -> OfficialSource {
    OfficialSource {
        authority: "국세청",
        title: "부가가치세 신고·납부기간",
        url: "https://www.nts.go.kr/nts/cm/cntnts/cntntsView.do?cntntsId=7693&mi=2272",
        retrieved_on: erp_sources_verified_on(),
    }
}

#[must_use]
pub const fn nts_e_tax_invoice_source() -> OfficialSource {
    OfficialSource {
        authority: "국세청",
        title: "전자(세금)계산서 발급방법 및 발급절차",
        url: "https://www.nts.go.kr/nts/cm/cntnts/cntntsView.do?cntntsId=7788&mi=2462",
        retrieved_on: erp_sources_verified_on(),
    }
}

#[must_use]
pub const fn nts_e_tax_invoice_system_operator_source() -> OfficialSource {
    OfficialSource {
        authority: "국세청",
        title: "전자(세금)계산서 시스템사업자 표준인증 안내",
        url: "https://www.nts.go.kr/nts/na/ntt/selectNttInfo.do?bbsId=1120&mi=2205&nttSn=1351085",
        retrieved_on: erp_sources_verified_on(),
    }
}

#[must_use]
pub fn vat_rates() -> Vec<VatRate> {
    vec![VatRate {
        period: EffectivePeriod::new(date!(2013 - 01 - 01), None),
        rate_bps: 1_000,
        source: nts_vat_source(),
    }]
}

pub fn vat_rate_on(day: Date) -> Result<VatRate, KernelError> {
    vat_rates()
        .into_iter()
        .find(|rate| rate.period.contains(day))
        .ok_or_else(|| KernelError::validation(format!("missing VAT rate for {day}")))
}

pub fn vat_tax_period_for(day: Date) -> Result<VatTaxPeriod, KernelError> {
    let year = day.year();
    if day.month() <= time::Month::June {
        Ok(VatTaxPeriod {
            year,
            half: VatHalfYear::First,
            period: EffectivePeriod::new(
                fixed_calendar_date(year, time::Month::January, 1)?,
                Some(fixed_calendar_date(year, time::Month::July, 1)?),
            ),
            source: nts_vat_reporting_source(),
        })
    } else {
        let next_year = year
            .checked_add(1)
            .ok_or_else(|| KernelError::validation("VAT period year overflow"))?;
        Ok(VatTaxPeriod {
            year,
            half: VatHalfYear::Second,
            period: EffectivePeriod::new(
                fixed_calendar_date(year, time::Month::July, 1)?,
                Some(fixed_calendar_date(next_year, time::Month::January, 1)?),
            ),
            source: nts_vat_reporting_source(),
        })
    }
}

pub fn calculate_standard_vat_won(supply_amount_won: i64, day: Date) -> Result<i64, KernelError> {
    if supply_amount_won < 0 {
        return Err(KernelError::validation(
            "VAT supply amount must be non-negative",
        ));
    }
    amount_by_bps_floor_won(supply_amount_won, vat_rate_on(day)?.rate_bps)
}

pub fn build_tax_invoice_draft(input: SalesInvoiceInput) -> Result<TaxInvoiceDraft, KernelError> {
    validate_business_registration_no(&input.supplier_business_registration_no)?;
    validate_business_registration_no(&input.recipient_business_registration_no)?;
    if input.supply_amount_won <= 0 {
        return Err(KernelError::validation(
            "tax invoice supply amount must be positive",
        ));
    }
    let vat_amount_won = calculate_standard_vat_won(input.supply_amount_won, input.issue_on)?;
    let total_amount_won = checked_add_won(input.supply_amount_won, vat_amount_won)?;
    Ok(TaxInvoiceDraft {
        invoice_id: input.invoice_id,
        issue_on: input.issue_on,
        supplier_business_registration_no: input.supplier_business_registration_no,
        recipient_business_registration_no: input.recipient_business_registration_no,
        supply_amount_won: input.supply_amount_won,
        vat_amount_won,
        total_amount_won,
        source: nts_vat_source(),
    })
}

pub fn build_sales_invoice_journal(
    draft: &TaxInvoiceDraft,
) -> Result<ValidatedJournalEntry, KernelError> {
    validate_journal_entry(JournalEntry {
        occurred_on: draft.issue_on,
        description: format!("Sales tax invoice {}", draft.invoice_id),
        source_document: SourceDocumentRef {
            kind: SourceDocumentKind::SalesTaxInvoice,
            document_id: draft.invoice_id.clone(),
        },
        lines: vec![
            line(
                AccountCode::AccountsReceivable,
                DebitCredit::Debit,
                draft.total_amount_won,
                "매출채권",
            ),
            line(
                AccountCode::SalesRevenue,
                DebitCredit::Credit,
                draft.supply_amount_won,
                "매출",
            ),
            line(
                AccountCode::VatPayable,
                DebitCredit::Credit,
                draft.vat_amount_won,
                "부가세예수금",
            ),
        ],
    })
}

pub fn build_customer_receipt_journal(
    input: PaymentInput,
) -> Result<ValidatedJournalEntry, KernelError> {
    validate_positive_amount(input.amount_won, "customer receipt amount")?;
    validate_journal_entry(JournalEntry {
        occurred_on: input.paid_on,
        description: format!("Customer receipt {}", input.payment_id),
        source_document: SourceDocumentRef {
            kind: SourceDocumentKind::CustomerReceipt,
            document_id: input.payment_id,
        },
        lines: vec![
            line(
                AccountCode::Cash,
                DebitCredit::Debit,
                input.amount_won,
                "현금",
            ),
            line(
                AccountCode::AccountsReceivable,
                DebitCredit::Credit,
                input.amount_won,
                "매출채권 회수",
            ),
        ],
    })
}

pub fn build_purchase_invoice_journal(
    input: PurchaseInvoiceInput,
) -> Result<ValidatedJournalEntry, KernelError> {
    validate_positive_amount(input.supply_amount_won, "purchase supply amount")?;
    if input.vat_amount_won < 0 {
        return Err(KernelError::validation(
            "purchase VAT amount must be non-negative",
        ));
    }
    let total = checked_add_won(input.supply_amount_won, input.vat_amount_won)?;
    let target_account = match input.cost_target {
        PurchaseCostTarget::Inventory => AccountCode::Inventory,
        PurchaseCostTarget::OperatingExpense => AccountCode::OperatingExpense,
    };
    validate_journal_entry(JournalEntry {
        occurred_on: input.issue_on,
        description: format!("Vendor invoice {}", input.invoice_id),
        source_document: SourceDocumentRef {
            kind: SourceDocumentKind::VendorInvoice,
            document_id: input.invoice_id,
        },
        lines: vec![
            line(
                target_account,
                DebitCredit::Debit,
                input.supply_amount_won,
                "매입",
            ),
            line(
                AccountCode::VatReceivable,
                DebitCredit::Debit,
                input.vat_amount_won,
                "부가세대급금",
            ),
            line(
                AccountCode::AccountsPayable,
                DebitCredit::Credit,
                total,
                "미지급금",
            ),
        ],
    })
}

pub fn build_vendor_payment_journal(
    input: PaymentInput,
) -> Result<ValidatedJournalEntry, KernelError> {
    validate_positive_amount(input.amount_won, "vendor payment amount")?;
    validate_journal_entry(JournalEntry {
        occurred_on: input.paid_on,
        description: format!("Vendor payment {}", input.payment_id),
        source_document: SourceDocumentRef {
            kind: SourceDocumentKind::VendorPayment,
            document_id: input.payment_id,
        },
        lines: vec![
            line(
                AccountCode::AccountsPayable,
                DebitCredit::Debit,
                input.amount_won,
                "미지급금 지급",
            ),
            line(
                AccountCode::Cash,
                DebitCredit::Credit,
                input.amount_won,
                "현금",
            ),
        ],
    })
}

pub fn consume_inventory_to_work_order(
    input: InventoryConsumptionInput,
) -> Result<InventoryConsumptionResult, KernelError> {
    if input.quantity_on_hand_before < 0 {
        return Err(KernelError::validation(
            "quantity on hand must be non-negative",
        ));
    }
    validate_positive_amount(input.quantity_consumed, "quantity consumed")?;
    validate_positive_amount(input.unit_cost_won, "unit cost")?;
    if input.quantity_consumed > input.quantity_on_hand_before {
        return Err(KernelError::validation(
            "inventory movement would make stock negative",
        ));
    }
    let quantity_on_hand_after = input
        .quantity_on_hand_before
        .checked_sub(input.quantity_consumed)
        .ok_or_else(|| KernelError::validation("inventory movement quantity overflow"))?;
    let cost_won = input
        .quantity_consumed
        .checked_mul(input.unit_cost_won)
        .ok_or_else(|| KernelError::validation("inventory cost overflow"))?;
    let journal = validate_journal_entry(JournalEntry {
        occurred_on: input.occurred_on,
        description: format!("WO {} consumed item {}", input.work_order_id, input.item_id),
        source_document: SourceDocumentRef {
            kind: SourceDocumentKind::InventoryMovement,
            document_id: input.movement_id,
        },
        lines: vec![
            line(
                AccountCode::CostOfGoodsSold,
                DebitCredit::Debit,
                cost_won,
                "작업지시 부품원가",
            ),
            line(
                AccountCode::Inventory,
                DebitCredit::Credit,
                cost_won,
                "재고 출고",
            ),
        ],
    })?;
    Ok(InventoryConsumptionResult {
        quantity_on_hand_after,
        cost_won,
        journal,
    })
}

pub fn validate_journal_entry(entry: JournalEntry) -> Result<ValidatedJournalEntry, KernelError> {
    if entry.lines.len() < 2 {
        return Err(KernelError::validation(
            "journal entry must have at least two lines",
        ));
    }
    let mut debit_total_won = 0_i64;
    let mut credit_total_won = 0_i64;
    for line in &entry.lines {
        validate_positive_amount(line.amount_won, "journal line amount")?;
        match line.side {
            DebitCredit::Debit => {
                debit_total_won = checked_add_won(debit_total_won, line.amount_won)?;
            }
            DebitCredit::Credit => {
                credit_total_won = checked_add_won(credit_total_won, line.amount_won)?;
            }
        }
    }
    if debit_total_won != credit_total_won {
        return Err(KernelError::validation(format!(
            "unbalanced journal entry: debit {debit_total_won} != credit {credit_total_won}"
        )));
    }
    Ok(ValidatedJournalEntry {
        entry,
        debit_total_won,
        credit_total_won,
    })
}

pub fn prepare_electronic_tax_invoice_relay(
    draft: &TaxInvoiceDraft,
    readiness: &ElectronicTaxInvoiceRelayReadiness,
) -> Result<ElectronicTaxInvoiceRelayEnvelope, KernelError> {
    if readiness.mode != ElectronicTaxInvoiceRelayMode::NtsRegisteredErpSystem {
        return Err(KernelError::validation(
            "programmatic e-tax relay requires an NTS-registered ERP system mode; HomeTax manual and ASP gateway modes are not enabled by this product policy",
        ));
    }
    if !readiness.official_protocol_verified {
        return Err(KernelError::validation(
            "official NTS e-tax relay protocol has not been verified",
        ));
    }
    if !readiness.nts_system_registration_present || !readiness.standard_certification_present {
        return Err(KernelError::validation(
            "NTS system registration and standard certification are required for e-tax relay",
        ));
    }
    if !readiness.production_credentials_present
        || !readiness.signing_certificate_present
        || !readiness.dedicated_inbox_registered
    {
        return Err(KernelError::validation(
            "production credentials, signing certificate, and dedicated inbox registration are required for e-tax relay",
        ));
    }
    Ok(ElectronicTaxInvoiceRelayEnvelope {
        invoice_id: draft.invoice_id.clone(),
        issue_on: draft.issue_on,
        supply_amount_won: draft.supply_amount_won,
        vat_amount_won: draft.vat_amount_won,
        total_amount_won: draft.total_amount_won,
        relay_mode: readiness.mode,
        source_url: readiness.source.url,
    })
}

pub fn validate_accounting_release_gate(
    input: &AccountingReleaseGateInput,
) -> Result<(), KernelError> {
    if input.rate_table_version.trim().is_empty() {
        return Err(KernelError::validation(
            "accounting rate table version is required",
        ));
    }
    if input.official_source_urls.is_empty() {
        return Err(KernelError::validation(
            "at least one official accounting/tax source URL is required",
        ));
    }
    if input.golden_cases.is_empty() {
        return Err(KernelError::validation(
            "at least one accounting golden case is required",
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
            "golden case {} lacks 세무사 validation",
            case.case_id
        )));
    }
    let validation = input
        .professional_validation
        .as_ref()
        .ok_or_else(|| KernelError::validation("세무사 professional validation is required"))?;
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
    Ok(())
}

fn fixed_calendar_date(year: i32, month: time::Month, day: u8) -> Result<Date, KernelError> {
    Date::from_calendar_date(year, month, day)
        .map_err(|_| KernelError::validation("invalid fixed VAT period date"))
}

fn amount_by_bps_floor_won(base_won: i64, bps: u32) -> Result<i64, KernelError> {
    let amount = i128::from(base_won)
        .checked_mul(i128::from(bps))
        .ok_or_else(|| KernelError::validation("ERP amount multiplication overflow"))?
        / BPS_DENOMINATOR;
    i64::try_from(amount).map_err(|_| KernelError::validation("ERP amount overflow"))
}

fn checked_add_won(left: i64, right: i64) -> Result<i64, KernelError> {
    left.checked_add(right)
        .ok_or_else(|| KernelError::validation("ERP amount addition overflow"))
}

fn validate_positive_amount(amount: i64, label: &str) -> Result<(), KernelError> {
    if amount <= 0 {
        return Err(KernelError::validation(format!("{label} must be positive")));
    }
    Ok(())
}

fn validate_business_registration_no(value: &str) -> Result<(), KernelError> {
    let digits = value
        .chars()
        .filter(|character| character.is_ascii_digit())
        .count();
    if digits != 10 {
        return Err(KernelError::validation(
            "business registration number must contain 10 digits",
        ));
    }
    Ok(())
}

fn line(
    account: AccountCode,
    side: DebitCredit,
    amount_won: i64,
    memo: &'static str,
) -> JournalLine {
    JournalLine {
        account,
        side,
        amount_won,
        memo: memo.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_dates_to_official_six_month_vat_tax_periods() {
        let first = vat_tax_period_for(date!(2026 - 06 - 30)).unwrap();
        assert_eq!(first.year, 2026);
        assert_eq!(first.half, VatHalfYear::First);
        assert!(first.period.contains(date!(2026 - 01 - 01)));
        assert!(!first.period.contains(date!(2026 - 07 - 01)));

        let second = vat_tax_period_for(date!(2026 - 12 - 31)).unwrap();
        assert_eq!(second.year, 2026);
        assert_eq!(second.half, VatHalfYear::Second);
        assert!(second.period.contains(date!(2026 - 07 - 01)));
        assert!(!second.period.contains(date!(2027 - 01 - 01)));
    }

    fn sales_input() -> SalesInvoiceInput {
        SalesInvoiceInput {
            invoice_id: "INV-2026-0001".to_string(),
            issue_on: date!(2026 - 06 - 27),
            supplier_business_registration_no: "123-45-67890".to_string(),
            recipient_business_registration_no: "111-22-33333".to_string(),
            supply_amount_won: 1_000_000,
        }
    }

    #[test]
    fn validates_double_entry_and_rejects_unbalanced_journals() {
        let unbalanced = JournalEntry {
            occurred_on: date!(2026 - 06 - 27),
            description: "bad entry".to_string(),
            source_document: SourceDocumentRef {
                kind: SourceDocumentKind::Adjustment,
                document_id: "ADJ-1".to_string(),
            },
            lines: vec![
                line(AccountCode::Cash, DebitCredit::Debit, 100, "cash"),
                line(
                    AccountCode::SalesRevenue,
                    DebitCredit::Credit,
                    90,
                    "revenue",
                ),
            ],
        };
        assert!(validate_journal_entry(unbalanced).is_err());

        let balanced = validate_journal_entry(JournalEntry {
            occurred_on: date!(2026 - 06 - 27),
            description: "good entry".to_string(),
            source_document: SourceDocumentRef {
                kind: SourceDocumentKind::Adjustment,
                document_id: "ADJ-2".to_string(),
            },
            lines: vec![
                line(AccountCode::Cash, DebitCredit::Debit, 100, "cash"),
                line(
                    AccountCode::SalesRevenue,
                    DebitCredit::Credit,
                    100,
                    "revenue",
                ),
            ],
        })
        .unwrap();
        assert_eq!(balanced.debit_total_won, 100);
        assert_eq!(balanced.credit_total_won, 100);
    }

    #[test]
    fn posts_sales_tax_invoice_to_ar_revenue_and_vat_payable() {
        let draft = build_tax_invoice_draft(sales_input()).unwrap();
        assert_eq!(draft.vat_amount_won, 100_000);
        assert_eq!(draft.total_amount_won, 1_100_000);

        let journal = build_sales_invoice_journal(&draft).unwrap();
        assert_eq!(journal.debit_total_won, 1_100_000);
        assert_eq!(journal.credit_total_won, 1_100_000);
        assert_line(
            &journal,
            AccountCode::AccountsReceivable,
            DebitCredit::Debit,
            1_100_000,
        );
        assert_line(
            &journal,
            AccountCode::SalesRevenue,
            DebitCredit::Credit,
            1_000_000,
        );
        assert_line(
            &journal,
            AccountCode::VatPayable,
            DebitCredit::Credit,
            100_000,
        );
    }

    #[test]
    fn posts_procurement_invoice_to_inventory_vat_receivable_and_ap() {
        let journal = build_purchase_invoice_journal(PurchaseInvoiceInput {
            invoice_id: "VINV-2026-0001".to_string(),
            issue_on: date!(2026 - 06 - 27),
            supply_amount_won: 500_000,
            vat_amount_won: 50_000,
            cost_target: PurchaseCostTarget::Inventory,
        })
        .unwrap();

        assert_eq!(journal.debit_total_won, 550_000);
        assert_eq!(journal.credit_total_won, 550_000);
        assert_line(
            &journal,
            AccountCode::Inventory,
            DebitCredit::Debit,
            500_000,
        );
        assert_line(
            &journal,
            AccountCode::VatReceivable,
            DebitCredit::Debit,
            50_000,
        );
        assert_line(
            &journal,
            AccountCode::AccountsPayable,
            DebitCredit::Credit,
            550_000,
        );
    }

    #[test]
    fn consumes_inventory_to_work_order_without_negative_stock() {
        let consumed = consume_inventory_to_work_order(InventoryConsumptionInput {
            movement_id: "MOVE-1".to_string(),
            occurred_on: date!(2026 - 06 - 27),
            item_id: "PART-1".to_string(),
            work_order_id: "WO-1".to_string(),
            quantity_on_hand_before: 8,
            quantity_consumed: 3,
            unit_cost_won: 12_000,
        })
        .unwrap();
        assert_eq!(consumed.quantity_on_hand_after, 5);
        assert_eq!(consumed.cost_won, 36_000);
        assert_line(
            &consumed.journal,
            AccountCode::CostOfGoodsSold,
            DebitCredit::Debit,
            36_000,
        );
        assert_line(
            &consumed.journal,
            AccountCode::Inventory,
            DebitCredit::Credit,
            36_000,
        );

        let negative = consume_inventory_to_work_order(InventoryConsumptionInput {
            movement_id: "MOVE-2".to_string(),
            occurred_on: date!(2026 - 06 - 27),
            item_id: "PART-1".to_string(),
            work_order_id: "WO-1".to_string(),
            quantity_on_hand_before: 2,
            quantity_consumed: 3,
            unit_cost_won: 12_000,
        });
        assert!(negative.is_err());
    }

    #[test]
    fn gates_e_tax_invoice_relay_on_verified_registered_erp_credentials() {
        let draft = build_tax_invoice_draft(sales_input()).unwrap();
        let not_ready = ElectronicTaxInvoiceRelayReadiness {
            mode: ElectronicTaxInvoiceRelayMode::HomeTaxManual,
            official_protocol_verified: false,
            nts_system_registration_present: false,
            standard_certification_present: false,
            production_credentials_present: false,
            signing_certificate_present: false,
            dedicated_inbox_registered: false,
            source: nts_e_tax_invoice_source(),
        };
        assert!(prepare_electronic_tax_invoice_relay(&draft, &not_ready).is_err());

        let ready = ElectronicTaxInvoiceRelayReadiness {
            mode: ElectronicTaxInvoiceRelayMode::NtsRegisteredErpSystem,
            official_protocol_verified: true,
            nts_system_registration_present: true,
            standard_certification_present: true,
            production_credentials_present: true,
            signing_certificate_present: true,
            dedicated_inbox_registered: true,
            source: nts_e_tax_invoice_system_operator_source(),
        };
        let envelope = prepare_electronic_tax_invoice_relay(&draft, &ready).unwrap();
        assert_eq!(envelope.invoice_id, "INV-2026-0001");
        assert_eq!(envelope.vat_amount_won, 100_000);
        assert_eq!(
            envelope.relay_mode,
            ElectronicTaxInvoiceRelayMode::NtsRegisteredErpSystem
        );
    }

    #[test]
    fn release_gate_requires_tax_accountant_validated_golden_case() {
        let blocked = AccountingReleaseGateInput {
            rate_table_version: "KR-VAT-2026-v1".to_string(),
            official_source_urls: vec![nts_vat_source().url.to_string()],
            golden_cases: vec![AccountingGoldenCase {
                case_id: "vat-sales-unvalidated".to_string(),
                rate_table_version: "KR-VAT-2026-v1".to_string(),
                professionally_validated: false,
                expected_debit_total_won: 1_100_000,
                expected_credit_total_won: 1_100_000,
            }],
            professional_validation: None,
        };
        assert!(validate_accounting_release_gate(&blocked).is_err());

        let allowed = AccountingReleaseGateInput {
            rate_table_version: "KR-VAT-2026-v1".to_string(),
            official_source_urls: vec![
                nts_vat_source().url.to_string(),
                nts_e_tax_invoice_source().url.to_string(),
                nts_vat_reporting_source().url.to_string(),
            ],
            golden_cases: vec![AccountingGoldenCase {
                case_id: "vat-sales-tax-accountant-reviewed".to_string(),
                rate_table_version: "KR-VAT-2026-v1".to_string(),
                professionally_validated: true,
                expected_debit_total_won: 1_100_000,
                expected_credit_total_won: 1_100_000,
            }],
            professional_validation: Some(ProfessionalValidation {
                reviewer_kind: ProfessionalReviewerKind::TaxAccountant,
                reviewed_on: date!(2026 - 06 - 27),
                artifact_sha256: "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
                    .to_string(),
                reviewer_reference: "tax-accountant-review-record".to_string(),
            }),
        };
        validate_accounting_release_gate(&allowed).unwrap();
    }

    fn assert_line(
        journal: &ValidatedJournalEntry,
        account: AccountCode,
        side: DebitCredit,
        amount_won: i64,
    ) {
        assert!(journal.entry.lines.iter().any(|line| {
            line.account == account && line.side == side && line.amount_won == amount_won
        }));
    }
}
