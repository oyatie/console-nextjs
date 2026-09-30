//! Pure docs/evidence-object domain invariants.
//!
//! This crate owns EV object value types and state-machine rules only. It has no
//! SQLx, REST, storage, request-context, workorder, or compliance dependency.

use console_kernel_core::KernelError;
pub use console_kernel_core::{
    EvidenceCopyId, EvidenceCustodyEventId, EvidenceExportId, EvidenceLegalHoldId,
    EvidenceObjectId, EvidenceTsaProofId,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct EvidenceCode(String);

impl EvidenceCode {
    /// # Errors
    /// Returns `KernelError::validation` unless the value is a canonical `EV-`
    /// code. The value is normalized to uppercase for storage/display.
    pub fn new(raw: impl Into<String>) -> Result<Self, KernelError> {
        let value = raw.into().trim().to_ascii_uppercase();
        let Some(suffix) = value.strip_prefix("EV-") else {
            return Err(KernelError::validation("evidence code must start with EV-"));
        };
        let valid_len = (3..=40).contains(&suffix.len());
        let valid_chars = suffix
            .chars()
            .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '-');
        if valid_len && valid_chars {
            Ok(Self(value))
        } else {
            Err(KernelError::validation(
                "evidence code must match ^EV-[A-Z0-9-]{3,40}$",
            ))
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::fmt::Display for EvidenceCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct Sha256Digest(String);

impl Sha256Digest {
    /// # Errors
    /// Returns `KernelError::validation` unless the value is exactly 64 hex
    /// characters. Hex is normalized to lower-case before storage.
    pub fn new(raw: impl Into<String>) -> Result<Self, KernelError> {
        let value = raw.into().trim().to_ascii_lowercase();
        let valid = value.len() == 64 && value.chars().all(|ch| ch.is_ascii_hexdigit());
        if valid {
            Ok(Self(value))
        } else {
            Err(KernelError::validation(
                "sha256 digest must be 64 lowercase hex characters",
            ))
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::fmt::Display for Sha256Digest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EvidenceClassification {
    General,
    Internal,
    Sensitive,
    Confidential,
    Secret,
}

impl EvidenceClassification {
    /// # Errors
    /// Returns `KernelError::validation` for unknown database values.
    pub fn parse(value: &str) -> Result<Self, KernelError> {
        match value.trim() {
            "GENERAL" => Ok(Self::General),
            "INTERNAL" => Ok(Self::Internal),
            "SENSITIVE" => Ok(Self::Sensitive),
            "CONFIDENTIAL" => Ok(Self::Confidential),
            "SECRET" => Ok(Self::Secret),
            _ => Err(KernelError::validation("unknown evidence classification")),
        }
    }

    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::General => "GENERAL",
            Self::Internal => "INTERNAL",
            Self::Sensitive => "SENSITIVE",
            Self::Confidential => "CONFIDENTIAL",
            Self::Secret => "SECRET",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceSourceType {
    RecordArchive,
    InboxDoc,
    MailAttachment,
    IngestJob,
    WorkOrderEvidenceMedia,
    ExternalDocument,
}

impl EvidenceSourceType {
    /// # Errors
    /// Returns `KernelError::validation` for unknown database values.
    pub fn parse(value: &str) -> Result<Self, KernelError> {
        match value.trim() {
            "record_archive" => Ok(Self::RecordArchive),
            "inbox_doc" => Ok(Self::InboxDoc),
            "mail_attachment" => Ok(Self::MailAttachment),
            "ingest_job" => Ok(Self::IngestJob),
            "work_order_evidence_media" => Ok(Self::WorkOrderEvidenceMedia),
            "external_document" => Ok(Self::ExternalDocument),
            _ => Err(KernelError::validation("unknown evidence source type")),
        }
    }

    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::RecordArchive => "record_archive",
            Self::InboxDoc => "inbox_doc",
            Self::MailAttachment => "mail_attachment",
            Self::IngestJob => "ingest_job",
            Self::WorkOrderEvidenceMedia => "work_order_evidence_media",
            Self::ExternalDocument => "external_document",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EvidenceSourceRef {
    pub source_type: EvidenceSourceType,
    pub source_id: String,
    pub source_code: Option<String>,
}

impl EvidenceSourceRef {
    /// # Errors
    /// Returns `KernelError::validation` when source identifiers are empty or too long.
    pub fn new(
        source_type: EvidenceSourceType,
        source_id: impl Into<String>,
        source_code: Option<String>,
    ) -> Result<Self, KernelError> {
        let source_id = normalize_required_text(source_id.into(), 200, "source_id")?;
        let source_code = normalize_optional_text(source_code, 120, "source_code")?;
        Ok(Self {
            source_type,
            source_id,
            source_code,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EvidenceCopyKind {
    Original,
    Derivative,
}

/// Immutable evidentiary meaning of a stored evidence copy.
///
/// A WORM-sealed original is the only copy that can carry evidentiary weight.
/// Derivatives remain explicitly non-evidentiary even when their own storage
/// replica is WORM verified: they are useful renderings, not a substitute for
/// the original record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EvidenceCopyEvidentiaryStatus {
    VerifiedOriginal,
    OriginalUnverified,
    NonEvidentiaryDerivative,
}

impl EvidenceCopyEvidentiaryStatus {
    /// # Errors
    /// Returns `KernelError::validation` for unknown persisted values.
    pub fn parse(value: &str) -> Result<Self, KernelError> {
        match value.trim() {
            "VERIFIED_ORIGINAL" => Ok(Self::VerifiedOriginal),
            "ORIGINAL_UNVERIFIED" => Ok(Self::OriginalUnverified),
            "NON_EVIDENTIARY_DERIVATIVE" => Ok(Self::NonEvidentiaryDerivative),
            _ => Err(KernelError::validation(
                "unknown evidence copy evidentiary status",
            )),
        }
    }

    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::VerifiedOriginal => "VERIFIED_ORIGINAL",
            Self::OriginalUnverified => "ORIGINAL_UNVERIFIED",
            Self::NonEvidentiaryDerivative => "NON_EVIDENTIARY_DERIVATIVE",
        }
    }
}

impl EvidenceCopyKind {
    /// # Errors
    /// Returns `KernelError::validation` when original/derivative shape rules are violated.
    pub fn validate(
        self,
        parent_copy_id: Option<EvidenceCopyId>,
        derivative_kind: Option<DerivativeKind>,
    ) -> Result<(), KernelError> {
        match self {
            Self::Original if parent_copy_id.is_none() && derivative_kind.is_none() => Ok(()),
            Self::Original => Err(KernelError::validation(
                "original evidence copies cannot have parent_copy_id or derivative_kind",
            )),
            Self::Derivative if parent_copy_id.is_some() && derivative_kind.is_some() => Ok(()),
            Self::Derivative => Err(KernelError::validation(
                "derivative evidence copies require parent_copy_id and derivative_kind",
            )),
        }
    }

    /// # Errors
    /// Returns `KernelError::validation` for unknown database values.
    pub fn parse(value: &str) -> Result<Self, KernelError> {
        match value.trim() {
            "ORIGINAL" => Ok(Self::Original),
            "DERIVATIVE" => Ok(Self::Derivative),
            _ => Err(KernelError::validation("unknown evidence copy kind")),
        }
    }

    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Original => "ORIGINAL",
            Self::Derivative => "DERIVATIVE",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DerivativeKind {
    Redacted,
    Thumbnail,
    Transcoded,
    Excerpt,
    ExportManifest,
    NormalizedText,
    Other,
}

impl DerivativeKind {
    /// # Errors
    /// Returns `KernelError::validation` for unknown database values.
    pub fn parse(value: &str) -> Result<Self, KernelError> {
        match value.trim() {
            "REDACTED" => Ok(Self::Redacted),
            "THUMBNAIL" => Ok(Self::Thumbnail),
            "TRANSCODED" => Ok(Self::Transcoded),
            "EXCERPT" => Ok(Self::Excerpt),
            "EXPORT_MANIFEST" => Ok(Self::ExportManifest),
            "NORMALIZED_TEXT" => Ok(Self::NormalizedText),
            "OTHER" => Ok(Self::Other),
            _ => Err(KernelError::validation("unknown evidence derivative kind")),
        }
    }

    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Redacted => "REDACTED",
            Self::Thumbnail => "THUMBNAIL",
            Self::Transcoded => "TRANSCODED",
            Self::Excerpt => "EXCERPT",
            Self::ExportManifest => "EXPORT_MANIFEST",
            Self::NormalizedText => "NORMALIZED_TEXT",
            Self::Other => "OTHER",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WormStorageStatus {
    Pending,
    Verified,
    Failed,
}

impl WormStorageStatus {
    /// # Errors
    /// Returns `KernelError::validation` for unknown database values.
    pub fn parse(value: &str) -> Result<Self, KernelError> {
        match value.trim() {
            "PENDING" => Ok(Self::Pending),
            "VERIFIED" => Ok(Self::Verified),
            "FAILED" => Ok(Self::Failed),
            _ => Err(KernelError::validation("unknown WORM storage status")),
        }
    }

    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Verified => "VERIFIED",
            Self::Failed => "FAILED",
        }
    }

    #[must_use]
    pub const fn is_verified(self) -> bool {
        matches!(self, Self::Verified)
    }
}

impl EvidenceCopyEvidentiaryStatus {
    /// Returns the only permitted meaning for a copy's immutable kind and its
    /// current WORM replication state. The database stores this same relation
    /// as a generated column; adapters verify it when decoding persisted data.
    #[must_use]
    pub const fn expected(copy_kind: EvidenceCopyKind, worm_status: WormStorageStatus) -> Self {
        match copy_kind {
            EvidenceCopyKind::Derivative => Self::NonEvidentiaryDerivative,
            EvidenceCopyKind::Original if worm_status.is_verified() => Self::VerifiedOriginal,
            EvidenceCopyKind::Original => Self::OriginalUnverified,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TsaProofStatus {
    Missing,
    Pending,
    Verified,
    Failed,
    Revoked,
    ExpiredCa,
}

impl TsaProofStatus {
    /// # Errors
    /// Returns `KernelError::validation` for unknown database values.
    pub fn parse(value: &str) -> Result<Self, KernelError> {
        match value.trim() {
            "MISSING" => Ok(Self::Missing),
            "PENDING" => Ok(Self::Pending),
            "VERIFIED" => Ok(Self::Verified),
            "FAILED" => Ok(Self::Failed),
            "REVOKED" => Ok(Self::Revoked),
            "EXPIRED_CA" => Ok(Self::ExpiredCa),
            _ => Err(KernelError::validation("unknown TSA proof status")),
        }
    }

    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Missing => "MISSING",
            Self::Pending => "PENDING",
            Self::Verified => "VERIFIED",
            Self::Failed => "FAILED",
            Self::Revoked => "REVOKED",
            Self::ExpiredCa => "EXPIRED_CA",
        }
    }

    #[must_use]
    pub const fn is_verified(self) -> bool {
        matches!(self, Self::Verified)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CustodyStage {
    Registered,
    HashRecorded,
    TsaSubmitted,
    TsaVerified,
    WormReplicated,
    CustodyTransferred,
    UnderReview,
    AdmissibilityEvaluated,
    LegalHoldApplied,
    LegalHoldReleased,
    Exported,
    Archived,
    DisposalRequested,
    Disposed,
}

impl CustodyStage {
    /// # Errors
    /// Returns `KernelError::validation` for unknown database values.
    pub fn parse(value: &str) -> Result<Self, KernelError> {
        match value.trim() {
            "REGISTERED" => Ok(Self::Registered),
            "HASH_RECORDED" => Ok(Self::HashRecorded),
            "TSA_SUBMITTED" => Ok(Self::TsaSubmitted),
            "TSA_VERIFIED" => Ok(Self::TsaVerified),
            "WORM_REPLICATED" => Ok(Self::WormReplicated),
            "CUSTODY_TRANSFERRED" => Ok(Self::CustodyTransferred),
            "UNDER_REVIEW" => Ok(Self::UnderReview),
            "ADMISSIBILITY_EVALUATED" => Ok(Self::AdmissibilityEvaluated),
            "LEGAL_HOLD_APPLIED" => Ok(Self::LegalHoldApplied),
            "LEGAL_HOLD_RELEASED" => Ok(Self::LegalHoldReleased),
            "EXPORTED" => Ok(Self::Exported),
            "ARCHIVED" => Ok(Self::Archived),
            "DISPOSAL_REQUESTED" => Ok(Self::DisposalRequested),
            "DISPOSED" => Ok(Self::Disposed),
            _ => Err(KernelError::validation("unknown evidence custody stage")),
        }
    }

    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Registered => "REGISTERED",
            Self::HashRecorded => "HASH_RECORDED",
            Self::TsaSubmitted => "TSA_SUBMITTED",
            Self::TsaVerified => "TSA_VERIFIED",
            Self::WormReplicated => "WORM_REPLICATED",
            Self::CustodyTransferred => "CUSTODY_TRANSFERRED",
            Self::UnderReview => "UNDER_REVIEW",
            Self::AdmissibilityEvaluated => "ADMISSIBILITY_EVALUATED",
            Self::LegalHoldApplied => "LEGAL_HOLD_APPLIED",
            Self::LegalHoldReleased => "LEGAL_HOLD_RELEASED",
            Self::Exported => "EXPORTED",
            Self::Archived => "ARCHIVED",
            Self::DisposalRequested => "DISPOSAL_REQUESTED",
            Self::Disposed => "DISPOSED",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LegalHoldState {
    Clear,
    Active,
}

impl LegalHoldState {
    /// # Errors
    /// Returns `KernelError::validation` for unknown database values.
    pub fn parse(value: &str) -> Result<Self, KernelError> {
        match value.trim() {
            "CLEAR" => Ok(Self::Clear),
            "ACTIVE" => Ok(Self::Active),
            _ => Err(KernelError::validation("unknown legal hold state")),
        }
    }

    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Clear => "CLEAR",
            Self::Active => "ACTIVE",
        }
    }

    #[must_use]
    pub const fn blocks_destructive_operations(self) -> bool {
        matches!(self, Self::Active)
    }

    /// # Errors
    /// Returns `KernelError::conflict` when an active legal hold blocks disposal
    /// or another destructive cleanup.
    pub fn ensure_destructive_operation_allowed(self) -> Result<(), KernelError> {
        if self.blocks_destructive_operations() {
            Err(KernelError::conflict(
                "active legal hold blocks evidence destructive operation",
            ))
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LegalHoldStatus {
    Active,
    Released,
}

impl LegalHoldStatus {
    /// # Errors
    /// Returns `KernelError::validation` for unknown database values.
    pub fn parse(value: &str) -> Result<Self, KernelError> {
        match value.trim() {
            "ACTIVE" => Ok(Self::Active),
            "RELEASED" => Ok(Self::Released),
            _ => Err(KernelError::validation("unknown legal hold status")),
        }
    }

    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Active => "ACTIVE",
            Self::Released => "RELEASED",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AdmissibilityStatus {
    Admissible,
    ReviewNeeded,
    Blocked,
    Inadmissible,
}

impl AdmissibilityStatus {
    /// # Errors
    /// Returns `KernelError::validation` for unknown database values.
    pub fn parse(value: &str) -> Result<Self, KernelError> {
        match value.trim() {
            "ADMISSIBLE" => Ok(Self::Admissible),
            "REVIEW_NEEDED" => Ok(Self::ReviewNeeded),
            "BLOCKED" => Ok(Self::Blocked),
            "INADMISSIBLE" => Ok(Self::Inadmissible),
            _ => Err(KernelError::validation("unknown admissibility status")),
        }
    }

    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Admissible => "ADMISSIBLE",
            Self::ReviewNeeded => "REVIEW_NEEDED",
            Self::Blocked => "BLOCKED",
            Self::Inadmissible => "INADMISSIBLE",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AdmissibilityReason {
    Sha256Missing,
    Sha256Mismatch,
    OriginalCopyMissing,
    OriginalNotWormVerified,
    DerivativeParentMissing,
    TsaMissing,
    TsaPending,
    TsaUnverified,
    TsaImprintMismatch,
    CustodyChainGap,
    CustodyChainDigestMismatch,
    SourceObjectMissingOrDenied,
    ActiveLegalHold,
    Disposed,
    ExportManifestUnsigned,
    PolicyReviewRequired,
}

impl AdmissibilityReason {
    /// # Errors
    /// Returns `KernelError::validation` for unknown database values.
    pub fn parse(value: &str) -> Result<Self, KernelError> {
        match value.trim() {
            "SHA256_MISSING" => Ok(Self::Sha256Missing),
            "SHA256_MISMATCH" => Ok(Self::Sha256Mismatch),
            "ORIGINAL_COPY_MISSING" => Ok(Self::OriginalCopyMissing),
            "ORIGINAL_NOT_WORM_VERIFIED" => Ok(Self::OriginalNotWormVerified),
            "DERIVATIVE_PARENT_MISSING" => Ok(Self::DerivativeParentMissing),
            "TSA_MISSING" => Ok(Self::TsaMissing),
            "TSA_PENDING" => Ok(Self::TsaPending),
            "TSA_UNVERIFIED" => Ok(Self::TsaUnverified),
            "TSA_IMPRINT_MISMATCH" => Ok(Self::TsaImprintMismatch),
            "CUSTODY_CHAIN_GAP" => Ok(Self::CustodyChainGap),
            "CUSTODY_CHAIN_DIGEST_MISMATCH" => Ok(Self::CustodyChainDigestMismatch),
            "SOURCE_OBJECT_MISSING_OR_DENIED" => Ok(Self::SourceObjectMissingOrDenied),
            "ACTIVE_LEGAL_HOLD" => Ok(Self::ActiveLegalHold),
            "DISPOSED" => Ok(Self::Disposed),
            "EXPORT_MANIFEST_UNSIGNED" => Ok(Self::ExportManifestUnsigned),
            "POLICY_REVIEW_REQUIRED" => Ok(Self::PolicyReviewRequired),
            _ => Err(KernelError::validation("unknown admissibility reason")),
        }
    }

    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Sha256Missing => "SHA256_MISSING",
            Self::Sha256Mismatch => "SHA256_MISMATCH",
            Self::OriginalCopyMissing => "ORIGINAL_COPY_MISSING",
            Self::OriginalNotWormVerified => "ORIGINAL_NOT_WORM_VERIFIED",
            Self::DerivativeParentMissing => "DERIVATIVE_PARENT_MISSING",
            Self::TsaMissing => "TSA_MISSING",
            Self::TsaPending => "TSA_PENDING",
            Self::TsaUnverified => "TSA_UNVERIFIED",
            Self::TsaImprintMismatch => "TSA_IMPRINT_MISMATCH",
            Self::CustodyChainGap => "CUSTODY_CHAIN_GAP",
            Self::CustodyChainDigestMismatch => "CUSTODY_CHAIN_DIGEST_MISMATCH",
            Self::SourceObjectMissingOrDenied => "SOURCE_OBJECT_MISSING_OR_DENIED",
            Self::ActiveLegalHold => "ACTIVE_LEGAL_HOLD",
            Self::Disposed => "DISPOSED",
            Self::ExportManifestUnsigned => "EXPORT_MANIFEST_UNSIGNED",
            Self::PolicyReviewRequired => "POLICY_REVIEW_REQUIRED",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EvidenceStorageRef {
    pub provider: String,
    pub object_id: String,
    pub key_ref: Option<String>,
    pub version_id: Option<String>,
}

impl EvidenceStorageRef {
    /// # Errors
    /// Returns `KernelError::validation` when the safe storage reference is empty or too long.
    pub fn new(
        provider: impl Into<String>,
        object_id: impl Into<String>,
        key_ref: Option<String>,
        version_id: Option<String>,
    ) -> Result<Self, KernelError> {
        Ok(Self {
            provider: normalize_required_text(provider.into(), 80, "storage_provider")?,
            object_id: normalize_required_text(object_id.into(), 300, "storage_object_id")?,
            key_ref: normalize_optional_text(key_ref, 500, "storage_key_ref")?,
            version_id: normalize_optional_text(version_id, 200, "storage_version_id")?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AdmissibilityInputs {
    pub original_copy_present: bool,
    pub original_worm_verified: bool,
    pub tsa_status: Option<TsaProofStatus>,
    pub tsa_imprint_matches_original: bool,
    pub custody_chain_intact: bool,
    pub source_resolvable: bool,
    pub disposed: bool,
    pub active_legal_hold: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AdmissibilitySummary {
    pub status: AdmissibilityStatus,
    pub reasons: Vec<AdmissibilityReason>,
    pub inputs: AdmissibilityInputs,
}

#[must_use]
pub fn evaluate_admissibility(inputs: AdmissibilityInputs) -> AdmissibilitySummary {
    let mut reasons = Vec::new();

    if inputs.disposed {
        reasons.push(AdmissibilityReason::Disposed);
    }
    if !inputs.original_copy_present {
        reasons.push(AdmissibilityReason::OriginalCopyMissing);
    } else if !inputs.original_worm_verified {
        reasons.push(AdmissibilityReason::OriginalNotWormVerified);
    }
    match inputs.tsa_status {
        Some(TsaProofStatus::Verified) if !inputs.tsa_imprint_matches_original => {
            reasons.push(AdmissibilityReason::TsaImprintMismatch);
        }
        Some(TsaProofStatus::Verified) => {}
        Some(TsaProofStatus::Pending) => reasons.push(AdmissibilityReason::TsaPending),
        Some(TsaProofStatus::Missing) | None => reasons.push(AdmissibilityReason::TsaMissing),
        Some(_) => reasons.push(AdmissibilityReason::TsaUnverified),
    }
    if !inputs.custody_chain_intact {
        reasons.push(AdmissibilityReason::CustodyChainGap);
    }
    if !inputs.source_resolvable {
        reasons.push(AdmissibilityReason::SourceObjectMissingOrDenied);
    }
    if inputs.active_legal_hold {
        reasons.push(AdmissibilityReason::ActiveLegalHold);
    }

    let contradiction = reasons.iter().any(|reason| {
        matches!(
            reason,
            AdmissibilityReason::Disposed
                | AdmissibilityReason::TsaImprintMismatch
                | AdmissibilityReason::CustodyChainGap
                | AdmissibilityReason::SourceObjectMissingOrDenied
        )
    });
    let blocking = inputs.active_legal_hold && !contradiction;
    let incomplete = !reasons.is_empty();
    let status = if contradiction {
        AdmissibilityStatus::Inadmissible
    } else if blocking {
        AdmissibilityStatus::Blocked
    } else if incomplete {
        AdmissibilityStatus::ReviewNeeded
    } else {
        AdmissibilityStatus::Admissible
    };

    AdmissibilitySummary {
        status,
        reasons,
        inputs,
    }
}

fn normalize_required_text(
    value: String,
    max_chars: usize,
    field: &str,
) -> Result<String, KernelError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(KernelError::validation(format!("{field} is required")));
    }
    if trimmed.chars().count() > max_chars {
        return Err(KernelError::validation(format!("{field} is too long")));
    }
    Ok(trimmed.to_owned())
}

fn normalize_optional_text(
    value: Option<String>,
    max_chars: usize,
    field: &str,
) -> Result<Option<String>, KernelError> {
    value
        .map(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else if trimmed.chars().count() > max_chars {
                Err(KernelError::validation(format!("{field} is too long")))
            } else {
                Ok(Some(trimmed.to_owned()))
            }
        })
        .transpose()
        .map(Option::flatten)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn sha256_digest_accepts_hex_and_normalizes_lowercase() {
        let digest =
            Sha256Digest::new("ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789")
                .unwrap();

        assert_eq!(
            digest.as_str(),
            "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
        );
        assert!(Sha256Digest::new("not-a-digest").is_err());
    }

    #[test]
    fn evidence_copy_kind_enforces_original_and_derivative_shape() {
        assert!(EvidenceCopyKind::Original.validate(None, None).is_ok());
        assert!(
            EvidenceCopyKind::Original
                .validate(Some(EvidenceCopyId::new()), None)
                .is_err()
        );
        assert!(
            EvidenceCopyKind::Derivative
                .validate(Some(EvidenceCopyId::new()), Some(DerivativeKind::Redacted))
                .is_ok()
        );
        assert!(
            EvidenceCopyKind::Derivative
                .validate(None, Some(DerivativeKind::Redacted))
                .is_err()
        );
        assert!(
            EvidenceCopyKind::Derivative
                .validate(Some(EvidenceCopyId::new()), None)
                .is_err()
        );
    }

    #[test]
    fn legal_hold_active_blocks_destructive_operations() {
        assert!(LegalHoldState::Active.blocks_destructive_operations());
        assert!(!LegalHoldState::Clear.blocks_destructive_operations());
    }

    #[test]
    fn evidence_copy_evidentiary_status_has_no_derivative_equivalence() {
        assert_eq!(
            EvidenceCopyEvidentiaryStatus::parse("VERIFIED_ORIGINAL").unwrap(),
            EvidenceCopyEvidentiaryStatus::VerifiedOriginal
        );
        assert_eq!(
            EvidenceCopyEvidentiaryStatus::parse("NON_EVIDENTIARY_DERIVATIVE").unwrap(),
            EvidenceCopyEvidentiaryStatus::NonEvidentiaryDerivative
        );
        assert_eq!(
            EvidenceCopyEvidentiaryStatus::expected(
                EvidenceCopyKind::Original,
                WormStorageStatus::Verified,
            ),
            EvidenceCopyEvidentiaryStatus::VerifiedOriginal
        );
        assert_eq!(
            EvidenceCopyEvidentiaryStatus::expected(
                EvidenceCopyKind::Original,
                WormStorageStatus::Pending,
            ),
            EvidenceCopyEvidentiaryStatus::OriginalUnverified
        );
        assert_eq!(
            EvidenceCopyEvidentiaryStatus::expected(
                EvidenceCopyKind::Derivative,
                WormStorageStatus::Verified,
            ),
            EvidenceCopyEvidentiaryStatus::NonEvidentiaryDerivative
        );
        assert!(EvidenceCopyEvidentiaryStatus::parse("VERIFIED_DERIVATIVE").is_err());
    }

    #[test]
    fn admissibility_distinguishes_ready_review_blocked_and_inadmissible() {
        let ready = evaluate_admissibility(AdmissibilityInputs {
            original_copy_present: true,
            original_worm_verified: true,
            tsa_status: Some(TsaProofStatus::Verified),
            tsa_imprint_matches_original: true,
            custody_chain_intact: true,
            source_resolvable: true,
            disposed: false,
            active_legal_hold: false,
        });
        assert_eq!(ready.status, AdmissibilityStatus::Admissible);
        assert!(ready.reasons.is_empty());

        let pending = evaluate_admissibility(AdmissibilityInputs {
            tsa_status: Some(TsaProofStatus::Pending),
            ..ready.inputs
        });
        assert_eq!(pending.status, AdmissibilityStatus::ReviewNeeded);
        assert_eq!(pending.reasons, vec![AdmissibilityReason::TsaPending]);

        let held = evaluate_admissibility(AdmissibilityInputs {
            active_legal_hold: true,
            ..ready.inputs
        });
        assert_eq!(held.status, AdmissibilityStatus::Blocked);
        assert_eq!(held.reasons, vec![AdmissibilityReason::ActiveLegalHold]);

        let mismatch = evaluate_admissibility(AdmissibilityInputs {
            tsa_imprint_matches_original: false,
            ..ready.inputs
        });
        assert_eq!(mismatch.status, AdmissibilityStatus::Inadmissible);
        assert_eq!(
            mismatch.reasons,
            vec![AdmissibilityReason::TsaImprintMismatch]
        );
    }

    #[test]
    fn evidence_vocab_roundtrips_parse_and_db_str() {
        for v in [
            EvidenceClassification::General,
            EvidenceClassification::Internal,
            EvidenceClassification::Sensitive,
            EvidenceClassification::Confidential,
            EvidenceClassification::Secret,
        ] {
            assert_eq!(EvidenceClassification::parse(v.as_db_str()).unwrap(), v);
        }
        assert!(EvidenceClassification::parse("TOP_SECRET").is_err());

        for v in [
            EvidenceSourceType::RecordArchive,
            EvidenceSourceType::InboxDoc,
            EvidenceSourceType::MailAttachment,
            EvidenceSourceType::IngestJob,
            EvidenceSourceType::WorkOrderEvidenceMedia,
            EvidenceSourceType::ExternalDocument,
        ] {
            assert_eq!(EvidenceSourceType::parse(v.as_db_str()).unwrap(), v);
        }
        assert!(EvidenceSourceType::parse("usb_drive").is_err());

        for v in [
            WormStorageStatus::Pending,
            WormStorageStatus::Verified,
            WormStorageStatus::Failed,
        ] {
            assert_eq!(WormStorageStatus::parse(v.as_db_str()).unwrap(), v);
        }
        assert!(WormStorageStatus::parse("ok").is_err());

        for v in [
            TsaProofStatus::Missing,
            TsaProofStatus::Pending,
            TsaProofStatus::Verified,
            TsaProofStatus::Failed,
            TsaProofStatus::Revoked,
            TsaProofStatus::ExpiredCa,
        ] {
            assert_eq!(TsaProofStatus::parse(v.as_db_str()).unwrap(), v);
        }
        assert!(TsaProofStatus::parse("unknown").is_err());

        for v in [
            CustodyStage::Registered,
            CustodyStage::HashRecorded,
            CustodyStage::TsaSubmitted,
            CustodyStage::TsaVerified,
            CustodyStage::WormReplicated,
            CustodyStage::CustodyTransferred,
            CustodyStage::UnderReview,
            CustodyStage::AdmissibilityEvaluated,
            CustodyStage::LegalHoldApplied,
            CustodyStage::LegalHoldReleased,
            CustodyStage::Exported,
            CustodyStage::Archived,
        ] {
            assert_eq!(CustodyStage::parse(v.as_db_str()).unwrap(), v);
        }
        assert!(CustodyStage::parse("deleted").is_err());

        for v in [LegalHoldState::Clear, LegalHoldState::Active] {
            assert_eq!(LegalHoldState::parse(v.as_db_str()).unwrap(), v);
        }
        for v in [LegalHoldStatus::Active, LegalHoldStatus::Released] {
            assert_eq!(LegalHoldStatus::parse(v.as_db_str()).unwrap(), v);
        }
        assert!(LegalHoldStatus::parse("PENDING").is_err());

        for v in [
            AdmissibilityStatus::Admissible,
            AdmissibilityStatus::ReviewNeeded,
            AdmissibilityStatus::Blocked,
            AdmissibilityStatus::Inadmissible,
        ] {
            assert_eq!(AdmissibilityStatus::parse(v.as_db_str()).unwrap(), v);
        }
        assert!(AdmissibilityStatus::parse("maybe").is_err());
    }
}
