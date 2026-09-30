//! Registry domain.
//!
//! Pure equipment/customer/site entities and value objects only. Excel parsing
//! and Postgres upserts live in outer crates.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use console_kernel_core::{BranchId, CustomerId, EquipmentId, KernelError, SiteId};

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct EquipmentNo(String);

impl EquipmentNo {
    pub fn parse(value: impl Into<String>) -> Result<Self, KernelError> {
        let value = value.into().trim().to_string();
        let chars: Vec<char> = value.chars().collect();
        let valid = chars.len() == 10
            && chars[0..3].iter().all(|c| c.is_ascii_uppercase())
            && chars[3..5].iter().all(|c| c.is_ascii_alphanumeric())
            && chars[5] == '-'
            && chars[6..10].iter().all(|c| c.is_ascii_digit());

        if valid {
            Ok(Self(value))
        } else {
            Err(KernelError::validation(format!(
                "invalid equipment number {value:?}: expected shape AAANN-NNNN"
            )))
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn manufacturer_code(&self) -> &str {
        &self.0[0..1]
    }

    #[must_use]
    pub fn kind_code(&self) -> &str {
        &self.0[1..2]
    }

    #[must_use]
    pub fn power_code(&self) -> &str {
        &self.0[2..3]
    }

    #[must_use]
    pub fn sequence_code(&self) -> &str {
        &self.0[7..10]
    }
}

impl TryFrom<String> for EquipmentNo {
    type Error = KernelError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<EquipmentNo> for String {
    fn from(value: EquipmentNo) -> Self {
        value.0
    }
}

impl std::fmt::Display for EquipmentNo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EquipmentStatus {
    Rented,
    Spare,
    Disposed,
    Replacement,
    Sold,
}

impl EquipmentStatus {
    pub fn parse(value: &str) -> Result<Self, KernelError> {
        match value.trim() {
            "임대" => Ok(Self::Rented),
            "예비" => Ok(Self::Spare),
            "폐기" => Ok(Self::Disposed),
            "대체" => Ok(Self::Replacement),
            "매각" => Ok(Self::Sold),
            other => Err(KernelError::validation(format!(
                "unknown equipment status {other:?}"
            ))),
        }
    }

    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Rented => "임대",
            Self::Spare => "예비",
            Self::Disposed => "폐기",
            Self::Replacement => "대체",
            Self::Sold => "매각",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct MoneyWon(i64);

impl MoneyWon {
    #[must_use]
    pub const fn new(amount: i64) -> Self {
        Self(amount)
    }

    #[must_use]
    pub const fn amount(self) -> i64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Ton {
    text: String,
    milli_tons: Option<i32>,
}

impl Ton {
    #[must_use]
    pub fn parse(value: &str) -> Self {
        let text = value.trim().to_string();
        let milli_tons = text.strip_suffix('T').and_then(|number| {
            number
                .parse::<f64>()
                .ok()
                .map(|tons| (tons * 1000.0).round() as i32)
        });
        Self { text, milli_tons }
    }

    #[must_use]
    pub fn as_text(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub const fn milli_tons(&self) -> Option<i32> {
        self.milli_tons
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Customer {
    id: CustomerId,
    branch_id: BranchId,
    name: String,
}

impl Customer {
    pub fn new(
        id: CustomerId,
        branch_id: BranchId,
        name: impl Into<String>,
    ) -> Result<Self, KernelError> {
        let name = normalize_required(name.into(), "customer name")?;
        Ok(Self {
            id,
            branch_id,
            name,
        })
    }

    #[must_use]
    pub const fn id(&self) -> CustomerId {
        self.id
    }

    #[must_use]
    pub const fn branch_id(&self) -> BranchId {
        self.branch_id
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Site {
    id: SiteId,
    branch_id: BranchId,
    customer_id: CustomerId,
    name: String,
}

impl Site {
    pub fn new(
        id: SiteId,
        branch_id: BranchId,
        customer_id: CustomerId,
        name: impl Into<String>,
    ) -> Result<Self, KernelError> {
        let name = normalize_required(name.into(), "site name")?;
        Ok(Self {
            id,
            branch_id,
            customer_id,
            name,
        })
    }

    #[must_use]
    pub const fn id(&self) -> SiteId {
        self.id
    }

    #[must_use]
    pub const fn branch_id(&self) -> BranchId {
        self.branch_id
    }

    #[must_use]
    pub const fn customer_id(&self) -> CustomerId {
        self.customer_id
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Equipment {
    id: EquipmentId,
    branch_id: BranchId,
    equipment_no: EquipmentNo,
    management_no: Option<String>,
    customer_id: CustomerId,
    site_id: SiteId,
    status: EquipmentStatus,
}

impl Equipment {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        id: EquipmentId,
        branch_id: BranchId,
        equipment_no: EquipmentNo,
        management_no: Option<String>,
        customer_id: CustomerId,
        site_id: SiteId,
        status: EquipmentStatus,
    ) -> Self {
        Self {
            id,
            branch_id,
            equipment_no,
            management_no: management_no.and_then(|value| {
                let trimmed = value.trim().to_string();
                (!trimmed.is_empty()).then_some(trimmed)
            }),
            customer_id,
            site_id,
            status,
        }
    }

    #[must_use]
    pub const fn id(&self) -> EquipmentId {
        self.id
    }

    #[must_use]
    pub const fn branch_id(&self) -> BranchId {
        self.branch_id
    }

    #[must_use]
    pub fn equipment_no(&self) -> &EquipmentNo {
        &self.equipment_no
    }

    #[must_use]
    pub fn management_no(&self) -> Option<&str> {
        self.management_no.as_deref()
    }

    #[must_use]
    pub const fn customer_id(&self) -> CustomerId {
        self.customer_id
    }

    #[must_use]
    pub const fn site_id(&self) -> SiteId {
        self.site_id
    }

    #[must_use]
    pub const fn status(&self) -> EquipmentStatus {
        self.status
    }
}

/// Compatibility rules for 대체장비 matching.
///
/// Source requirement: "대체장비: 유효설비/동일 기종ton수/입식 좌식/납산리튬엔진 유사시".
/// In the imported master-list schema this means:
/// - candidate equipment must be an active spare (`상태=예비`) selected outside
///   this pure function by the repository;
/// - `규격` must match exactly after trimming (for example 입식 vs 좌식);
/// - 동력 compatibility is derived from the 장비No prefix power code, keeping
///   `B` electric/lead-lithium style units separate from diesel (`O` in the
///   workbook, `D` in the brief) and LPG (`L`) units;
/// - numeric tons match when candidate ton is equal to the down unit or is the
///   nearest higher capacity; unknown `미정` tons only match the same unknown
///   text because no safe capacity ordering exists.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SubstituteEquipmentProfile {
    pub id: EquipmentId,
    pub branch_id: BranchId,
    pub equipment_no: EquipmentNo,
    pub status: EquipmentStatus,
    pub specification: String,
    pub ton: Ton,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubstituteMatchKind {
    ExactTon,
    NearestAbove,
    UnknownTonExactText,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RankedSubstituteCandidate {
    pub equipment: SubstituteEquipmentProfile,
    pub kind: SubstituteMatchKind,
    pub ton_delta_milli: Option<i32>,
}

/// Filter and rank already-loaded spare candidates for one down unit.
#[must_use]
pub fn rank_substitute_candidates<I>(
    down: &SubstituteEquipmentProfile,
    candidates: I,
) -> Vec<RankedSubstituteCandidate>
where
    I: IntoIterator<Item = SubstituteEquipmentProfile>,
{
    let mut ranked = candidates
        .into_iter()
        .filter_map(|candidate| substitute_match(down, candidate))
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        match_priority(left.kind)
            .cmp(&match_priority(right.kind))
            .then_with(|| {
                left.ton_delta_milli
                    .unwrap_or(i32::MAX)
                    .cmp(&right.ton_delta_milli.unwrap_or(i32::MAX))
            })
            .then_with(|| {
                left.equipment
                    .equipment_no
                    .as_str()
                    .cmp(right.equipment.equipment_no.as_str())
            })
    });
    ranked
}

fn substitute_match(
    down: &SubstituteEquipmentProfile,
    candidate: SubstituteEquipmentProfile,
) -> Option<RankedSubstituteCandidate> {
    if candidate.id == down.id
        || candidate.status != EquipmentStatus::Spare
        || !same_specification(&down.specification, &candidate.specification)
        || !compatible_power(&down.equipment_no, &candidate.equipment_no)
    {
        return None;
    }

    match (down.ton.milli_tons(), candidate.ton.milli_tons()) {
        (Some(down_ton), Some(candidate_ton)) if candidate_ton == down_ton => {
            Some(RankedSubstituteCandidate {
                equipment: candidate,
                kind: SubstituteMatchKind::ExactTon,
                ton_delta_milli: Some(0),
            })
        }
        (Some(down_ton), Some(candidate_ton)) if candidate_ton > down_ton => {
            Some(RankedSubstituteCandidate {
                equipment: candidate,
                kind: SubstituteMatchKind::NearestAbove,
                ton_delta_milli: Some(candidate_ton - down_ton),
            })
        }
        (None, None) if down.ton.as_text() == candidate.ton.as_text() => {
            Some(RankedSubstituteCandidate {
                equipment: candidate,
                kind: SubstituteMatchKind::UnknownTonExactText,
                ton_delta_milli: None,
            })
        }
        _ => None,
    }
}

fn same_specification(left: &str, right: &str) -> bool {
    left.trim() == right.trim()
}

fn compatible_power(left: &EquipmentNo, right: &EquipmentNo) -> bool {
    power_family(left.power_code()) == power_family(right.power_code())
}

fn power_family(power_code: &str) -> &str {
    match power_code {
        "B" => "battery",
        "O" | "D" => "diesel",
        "L" => "lpg",
        other => other,
    }
}

fn match_priority(kind: SubstituteMatchKind) -> u8 {
    match kind {
        SubstituteMatchKind::ExactTon | SubstituteMatchKind::UnknownTonExactText => 0,
        SubstituteMatchKind::NearestAbove => 1,
    }
}

fn normalize_required(value: String, field: &str) -> Result<String, KernelError> {
    let trimmed = value.trim().to_string();
    if trimmed.is_empty() {
        Err(KernelError::validation(format!("{field} is required")))
    } else {
        Ok(trimmed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equipment_no_accepts_aaann_nnnn_shape() {
        for value in ["CFO25-0290", "DFO30-0010", "ABC1A-1234", "  XYZ99-0001  "] {
            let parsed = EquipmentNo::parse(value).expect("valid equipment number");
            assert_eq!(parsed.as_str(), value.trim());
        }
    }

    #[test]
    fn equipment_no_rejects_wrong_length_case_dash_and_digits() {
        for value in [
            "",
            "CF-290",
            "CFO25-029",
            "CFO25-02901",
            "cfo25-0290",
            "CFo25-0290",
            "CFO250290",
            "CFO25_0290",
            "CFO25-029A",
            "CFO25-ABCD",
            "---00-0000",
            "12345-6789",
        ] {
            assert!(
                EquipmentNo::parse(value).is_err(),
                "expected reject for {value:?}"
            );
        }
    }

    #[test]
    fn equipment_status_korean_wire_roundtrips_and_rejects_unknown() {
        for status in [
            EquipmentStatus::Rented,
            EquipmentStatus::Spare,
            EquipmentStatus::Disposed,
            EquipmentStatus::Replacement,
            EquipmentStatus::Sold,
        ] {
            let wire = status.as_db_str();
            assert_eq!(EquipmentStatus::parse(wire).unwrap(), status);
            assert_eq!(
                EquipmentStatus::parse(&format!("  {wire}  ")).unwrap(),
                status
            );
        }

        for bad in [
            "",
            "rented",
            "RENT",
            "Rented",
            "상태",
            "예비예비",
            "임대중",
            "SPARE",
        ] {
            assert!(
                EquipmentStatus::parse(bad).is_err(),
                "expected reject for {bad:?}"
            );
        }
    }

    #[test]
    fn ton_parses_t_suffix_milli_tons_and_keeps_plain_text() {
        let numeric = Ton::parse("12.5T");
        assert_eq!(numeric.as_text(), "12.5T");
        assert_eq!(numeric.milli_tons(), Some(12_500));

        let whole = Ton::parse("  3T  ");
        assert_eq!(whole.as_text(), "3T");
        assert_eq!(whole.milli_tons(), Some(3_000));

        let plain = Ton::parse("12.5");
        assert_eq!(plain.as_text(), "12.5");
        assert_eq!(plain.milli_tons(), None);

        let undecided = Ton::parse("미정");
        assert_eq!(undecided.as_text(), "미정");
        assert_eq!(undecided.milli_tons(), None);
    }

    #[test]
    fn customer_and_site_require_non_empty_names() {
        let branch = BranchId::new();
        let customer_id = CustomerId::new();
        let site_id = SiteId::new();

        assert!(Customer::new(customer_id, branch, "  ").is_err());
        assert!(Customer::new(customer_id, branch, "").is_err());
        let customer = Customer::new(customer_id, branch, "  Acme  ").unwrap();
        assert_eq!(customer.name(), "Acme");

        assert!(Site::new(site_id, branch, customer_id, "\t").is_err());
        assert!(Site::new(site_id, branch, customer_id, "").is_err());
        let site = Site::new(site_id, branch, customer_id, "  Site A  ").unwrap();
        assert_eq!(site.name(), "Site A");
    }

    #[test]
    fn equipment_drops_blank_management_no() {
        let equipment = Equipment::new(
            EquipmentId::new(),
            BranchId::new(),
            EquipmentNo::parse("CFO25-0290").unwrap(),
            Some("   ".into()),
            CustomerId::new(),
            SiteId::new(),
            EquipmentStatus::Spare,
        );
        assert_eq!(equipment.management_no(), None);
        assert_eq!(equipment.status(), EquipmentStatus::Spare);
        assert_eq!(equipment.equipment_no().as_str(), "CFO25-0290");
    }
}
