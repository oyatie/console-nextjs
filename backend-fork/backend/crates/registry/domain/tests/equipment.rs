#![allow(clippy::unwrap_used)]

use console_kernel_core::{BranchId, EquipmentId};
use console_registry_domain::{
    EquipmentNo, EquipmentStatus, MoneyWon, SubstituteEquipmentProfile, SubstituteMatchKind, Ton,
    rank_substitute_candidates,
};

#[test]
fn equipment_no_derives_the_same_mid_formula_columns_as_the_workbook() {
    let equipment_no = EquipmentNo::parse("CFO25-0290").unwrap();

    assert_eq!(equipment_no.manufacturer_code(), "C");
    assert_eq!(equipment_no.kind_code(), "F");
    assert_eq!(equipment_no.power_code(), "O");
    assert_eq!(equipment_no.sequence_code(), "290");
}

#[test]
fn equipment_no_rejects_values_that_cannot_produce_prefix_columns() {
    let err = EquipmentNo::parse("CF-290").unwrap_err();

    assert!(err.message.contains("equipment number"));
}

#[test]
fn equipment_status_covers_actual_master_list_values() {
    for status in ["임대", "예비", "폐기", "대체", "매각"] {
        assert!(
            EquipmentStatus::parse(status).is_ok(),
            "{status} should parse"
        );
    }

    assert!(EquipmentStatus::parse("상태").is_err());
}

#[test]
fn money_won_allows_negative_residual_values() {
    let residual = MoneyWon::new(-10_650_084);

    assert_eq!(residual.amount(), -10_650_084);
}

#[test]
fn ton_keeps_original_text_and_derives_milli_tons_when_numeric() {
    let numeric = Ton::parse("2.5T");
    assert_eq!(numeric.as_text(), "2.5T");
    assert_eq!(numeric.milli_tons(), Some(2500));

    let undecided = Ton::parse("미정");
    assert_eq!(undecided.as_text(), "미정");
    assert_eq!(undecided.milli_tons(), None);
}

#[test]
fn substitute_matching_keeps_exact_ton_before_nearest_above() {
    let branch_id = BranchId::new();
    let down = substitute_profile(branch_id, "CFO25-0290", "좌식", "2.5T");
    let exact = substitute_profile(branch_id, "DFO25-0106", "좌식", "2.5T");
    let above = substitute_profile(branch_id, "CFO35-0075", "좌식", "3.5T");

    let ranked = rank_substitute_candidates(&down, [above.clone(), exact.clone()]);

    assert_eq!(ranked.len(), 2);
    assert_eq!(
        ranked[0].equipment.equipment_no.as_str(),
        exact.equipment_no.as_str()
    );
    assert_eq!(ranked[0].kind, SubstituteMatchKind::ExactTon);
    assert_eq!(
        ranked[1].equipment.equipment_no.as_str(),
        above.equipment_no.as_str()
    );
    assert_eq!(ranked[1].kind, SubstituteMatchKind::NearestAbove);
}

#[test]
fn substitute_matching_excludes_wrong_spec_power_and_lower_capacity() {
    let branch_id = BranchId::new();
    let down = substitute_profile(branch_id, "CFO25-0290", "좌식", "2.5T");
    let wrong_spec = substitute_profile(branch_id, "CFB25-0284", "입식", "2.5T");
    let wrong_power = substitute_profile(branch_id, "CFB25-0100", "좌식", "2.5T");
    let lower_ton = substitute_profile(branch_id, "CFO18-9998", "좌식", "1.8T");

    let ranked = rank_substitute_candidates(&down, [wrong_spec, wrong_power, lower_ton]);

    assert!(ranked.is_empty());
}

#[test]
fn substitute_matching_keeps_unknown_ton_conservative() {
    let branch_id = BranchId::new();
    let down = substitute_profile(branch_id, "EOB00-0067", "입식", "미정");
    let unknown_candidate = substitute_profile(branch_id, "EOB00-0442", "입식", "미정");
    let numeric_candidate = substitute_profile(branch_id, "EOB15-9999", "입식", "1.5T");

    let ranked = rank_substitute_candidates(&down, [numeric_candidate, unknown_candidate.clone()]);

    assert_eq!(ranked.len(), 1);
    assert_eq!(
        ranked[0].equipment.equipment_no.as_str(),
        unknown_candidate.equipment_no.as_str()
    );
    assert_eq!(ranked[0].kind, SubstituteMatchKind::UnknownTonExactText);
}

fn substitute_profile(
    branch_id: BranchId,
    equipment_no: &str,
    specification: &str,
    ton: &str,
) -> SubstituteEquipmentProfile {
    SubstituteEquipmentProfile {
        id: EquipmentId::new(),
        branch_id,
        equipment_no: EquipmentNo::parse(equipment_no).unwrap(),
        status: EquipmentStatus::Spare,
        specification: specification.to_owned(),
        ton: Ton::parse(ton),
    }
}
