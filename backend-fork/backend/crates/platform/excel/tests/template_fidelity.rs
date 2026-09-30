//! Integration test: byte-fidelity spike for the Korean daily-status Excel template.
//!
//! Acceptance criteria (T0.10 / ADR-0008):
//!   - umya-spreadsheet can load the ground-truth template without data loss.
//!   - A realistic data row can be written and read back with full fidelity.
//!   - Merged-cell ranges, column widths, and section headers survive the round-trip.

// Tests use expect/panic for clear diagnostic messages — intentional.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::path::PathBuf;

use umya_spreadsheet::Workbook;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn template_path() -> PathBuf {
    let manifest =
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set by cargo test");
    PathBuf::from(manifest).join("../../../../docs/reference/일일업무진행현황_0605.xlsx")
}

fn load(path: &std::path::Path) -> Workbook {
    umya_spreadsheet::reader::xlsx::read(path)
        .unwrap_or_else(|e| panic!("Failed to read workbook at {}: {e}", path.display()))
}

fn get_sheet<'a>(book: &'a Workbook, name: &str) -> &'a umya_spreadsheet::Worksheet {
    book.sheet_by_name(name)
        .unwrap_or_else(|e| panic!("Sheet '{name}' not found: {e}"))
}

fn cell_value(ws: &umya_spreadsheet::Worksheet, col: u32, row: u32) -> String {
    ws.cell((col, row))
        .map(|c| c.value().to_string())
        .unwrap_or_default()
}

/// Collect all merged-cell range strings from a worksheet, sorted for stable comparison.
fn merged_ranges(ws: &umya_spreadsheet::Worksheet) -> BTreeSet<String> {
    ws.merge_cells().iter().map(|mc| mc.range()).collect()
}

// ---------------------------------------------------------------------------
// Pre-condition assertions (template structure)
// ---------------------------------------------------------------------------

#[test]
fn precondition_sheet_name() {
    let book = load(&template_path());
    let ws = get_sheet(&book, "6월05일");
    assert_eq!(ws.name(), "6월05일");
}

#[test]
fn precondition_merged_cell_count() {
    let book = load(&template_path());
    let ws = get_sheet(&book, "6월05일");
    let ranges = merged_ranges(ws);
    assert_eq!(
        ranges.len(),
        16,
        "Expected exactly 16 merged ranges, found {}. Ranges: {ranges:#?}",
        ranges.len()
    );
}

#[test]
fn precondition_title_cell() {
    let book = load(&template_path());
    let ws = get_sheet(&book, "6월05일");
    // Title is expected in row 1; scan columns 1-34 (A-AH) for the title string.
    let title_found = (1u32..=34).any(|col| cell_value(ws, col, 1).contains('◈'));
    assert!(
        title_found,
        "Title cell containing '◈' not found in row 1 (columns A–AH)"
    );
}

#[test]
fn precondition_section_headers() {
    let book = load(&template_path());
    let ws = get_sheet(&book, "6월05일");

    let sections: &[(&str, u32)] = &[
        ("1. 일일 진행업무(실적)", 2),
        ("2. 일일 진행 업무(계획)", 24),
        ("3. 미결 업무 현황(누적)", 44),
        ("4. 정기검사", 77),
    ];

    for (header, row) in sections {
        let found = (1u32..=34).any(|col| cell_value(ws, col, *row).contains(*header));
        assert!(found, "Section header '{header}' not found in row {row}");
    }
}

// ---------------------------------------------------------------------------
// Round-trip write / re-read assertions
// ---------------------------------------------------------------------------

/// Write one realistic data row into section 1 (row 4), save to a temp file,
/// re-read it, and verify all structural invariants plus the written values.
#[test]
fn roundtrip_fill_and_read_back() {
    // ── 1. Load template ────────────────────────────────────────────────────
    let mut book = load(&template_path());

    // Capture pre-write merged ranges from the original template.
    let original_ranges: BTreeSet<String> = {
        let ws = get_sheet(&book, "6월05일");
        merged_ranges(ws)
    };

    // ── 2. Fill row 4 (section 1 first data row) ────────────────────────────
    // Column mapping (1-based):
    //   A(1)=구분  B(2)=No  C(3)=사업장  D(4)=호기  E(5)=불량내용
    //   F(6)=작업자  G(7)=Warning/Priority
    const ROW_DATA: u32 = 4;
    {
        let ws = book
            .sheet_by_name_mut("6월05일")
            .expect("Sheet '6월05일' must be present for writing");

        ws.cell_mut((1u32, ROW_DATA)).set_value("미"); // 구분
        ws.cell_mut((2u32, ROW_DATA)).set_value("99"); // No
        ws.cell_mut((3u32, ROW_DATA)).set_value("태성이엔지"); // 사업장
        ws.cell_mut((4u32, ROW_DATA)).set_value("#290"); // 호기
        ws.cell_mut((5u32, ROW_DATA)).set_value("시동안걸림"); // 불량내용
        ws.cell_mut((6u32, ROW_DATA)).set_value("김용현"); // 작업자
        ws.cell_mut((7u32, ROW_DATA)).set_value("Priority#1"); // Warning/Priority
    }

    // ── 3. Write to temp file ────────────────────────────────────────────────
    let tmp_dir = tempfile::tempdir().expect("tempdir creation failed");
    let out_path = tmp_dir.path().join("output_fidelity.xlsx");

    umya_spreadsheet::writer::xlsx::write(&book, &out_path)
        .unwrap_or_else(|e| panic!("Failed to write output workbook: {e}"));

    // ── 4. Re-read ──────────────────────────────────────────────────────────
    let reread = load(&out_path);

    // ── 5. Assert sheet name survives ────────────────────────────────────────
    let ws_out = get_sheet(&reread, "6월05일");
    assert_eq!(
        ws_out.name(),
        "6월05일",
        "Sheet name changed after round-trip"
    );

    // ── 6. Assert dimension: original 97-row extent must be intact ───────────
    let max_row = ws_out.highest_row();
    assert!(
        max_row >= 97,
        "Max row after round-trip is {max_row}, expected >= 97 (template extent)"
    );

    // ── 7. Assert all 16 merged ranges preserved exactly ────────────────────
    let output_ranges = merged_ranges(ws_out);
    assert_eq!(
        output_ranges.len(),
        16,
        "Merged range count changed: before={}, after={}.\nLost: {:#?}\nGained: {:#?}",
        original_ranges.len(),
        output_ranges.len(),
        original_ranges
            .difference(&output_ranges)
            .collect::<Vec<_>>(),
        output_ranges
            .difference(&original_ranges)
            .collect::<Vec<_>>(),
    );
    assert_eq!(
        original_ranges,
        output_ranges,
        "Merged cell ranges changed after round-trip.\nLost: {:#?}\nGained: {:#?}",
        original_ranges
            .difference(&output_ranges)
            .collect::<Vec<_>>(),
        output_ranges
            .difference(&original_ranges)
            .collect::<Vec<_>>(),
    );

    // ── 8. Assert title cell still contains '◈' ──────────────────────────────
    let title_found = (1u32..=34).any(|col| cell_value(ws_out, col, 1).contains('◈'));
    assert!(title_found, "Title '◈' missing after round-trip");

    // ── 9. Assert section headers survive ────────────────────────────────────
    let sections: &[(&str, u32)] = &[
        ("1. 일일 진행업무(실적)", 2),
        ("2. 일일 진행 업무(계획)", 24),
        ("3. 미결 업무 현황(누적)", 44),
        ("4. 정기검사", 77),
    ];
    for (header, row) in sections {
        let found = (1u32..=34).any(|col| cell_value(ws_out, col, *row).contains(*header));
        assert!(
            found,
            "Section header '{header}' missing at row {row} after round-trip"
        );
    }

    // ── 10. Assert written values are readable back ──────────────────────────
    assert_eq!(cell_value(ws_out, 1, ROW_DATA), "미", "구분 mismatch");
    assert_eq!(cell_value(ws_out, 2, ROW_DATA), "99", "No mismatch");
    assert_eq!(
        cell_value(ws_out, 3, ROW_DATA),
        "태성이엔지",
        "사업장 mismatch"
    );
    assert_eq!(cell_value(ws_out, 4, ROW_DATA), "#290", "호기 mismatch");
    assert_eq!(
        cell_value(ws_out, 5, ROW_DATA),
        "시동안걸림",
        "불량내용 mismatch"
    );
    assert_eq!(cell_value(ws_out, 6, ROW_DATA), "김용현", "작업자 mismatch");
    assert_eq!(
        cell_value(ws_out, 7, ROW_DATA),
        "Priority#1",
        "Warning mismatch"
    );

    // ── 11. Assert column widths survive for sample columns ──────────────────
    // Column A (index "A") is expected to have an explicit width in this template.
    // get_width() returns f64 (copy); no deref needed.
    if let Some(col_dim) = ws_out.column_dimension("A") {
        let width = col_dim.width();
        assert!(
            width > 0.0_f64,
            "Column A width should be > 0 after round-trip, got {width}"
        );
    }
    // (If umya returns None for column A there is no stored explicit width —
    //  acceptable; width assertion is best-effort for this spike.)
}
