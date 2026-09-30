//! Integration tests for the promoted template-fill engine.
//!
//! These tests exercise the public bytes-in/bytes-out platform API, not
//! umya-spreadsheet internals.

// Tests use expect/panic for direct diagnostics around workbook fixtures.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::io::Cursor;
use std::path::PathBuf;

use console_platform_excel::{
    CellWrite, DAILY_STATUS_TEMPLATE, DailyStatusSection, SectionFill, TemplateRow,
    fill_template_bytes, roundtrip_workbook_bytes, umya_spreadsheet,
};
use umya_spreadsheet::{Workbook, Worksheet};

fn fixture_path(name: &str) -> PathBuf {
    let manifest =
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set by cargo test");
    PathBuf::from(manifest)
        .join("../../../../docs/reference")
        .join(name)
}

fn fixture_bytes(name: &str) -> Vec<u8> {
    std::fs::read(fixture_path(name)).expect("reference workbook must be readable")
}

fn load_from_bytes(bytes: &[u8]) -> Workbook {
    umya_spreadsheet::reader::xlsx::read_reader(Cursor::new(bytes), true)
        .expect("workbook bytes must be readable")
}

fn sheet<'a>(book: &'a Workbook, name: &str) -> &'a Worksheet {
    book.sheet_by_name(name)
        .unwrap_or_else(|e| panic!("Sheet '{name}' not found: {e}"))
}

fn cell_value(ws: &Worksheet, col: u32, row: u32) -> String {
    ws.cell((col, row))
        .map(|cell| cell.value().to_string())
        .unwrap_or_default()
}

fn merged_ranges(ws: &Worksheet) -> BTreeSet<String> {
    ws.merge_cells().iter().map(|range| range.range()).collect()
}

fn daily_status_row(no: u32, label: &str) -> TemplateRow {
    TemplateRow::new([
        CellWrite::text(1, "미"),
        CellWrite::text(2, no.to_string()),
        CellWrite::text(3, "2026-06-12"),
        CellWrite::text(4, format!("{label}-사업장")),
        CellWrite::text(5, format!("#{no:03}")),
        CellWrite::text(6, "GTS30D"),
        CellWrite::text(7, format!("{label}-VIN")),
        CellWrite::text(8, format!("{label}-불량내용")),
        CellWrite::text(9, "김테스트"),
        CellWrite::text(10, "2026-06-13"),
        CellWrite::text(11, "2026-06-14"),
        CellWrite::text(12, format!("{label}-비고")),
        CellWrite::text(13, "Priority#2"),
    ])
}

fn pending_backlog_row(no: u32) -> TemplateRow {
    TemplateRow::new([
        CellWrite::text(1, "미"),
        CellWrite::text(2, no.to_string()),
        CellWrite::text(3, "2026-06-01"),
        CellWrite::text(4, format!("미결-{no:02}")),
        CellWrite::text(5, format!("#{no:03}")),
        CellWrite::text(6, "D30SE-7"),
        CellWrite::text(7, format!("PENDING-VIN-{no:02}")),
        CellWrite::text(8, format!("미결 누적 항목 {no:02}")),
        CellWrite::text(9, "정테스트"),
        CellWrite::text(10, "2026-06-20"),
        CellWrite::text(12, format!("추가 행 {no:02}")),
        CellWrite::text(13, "Priority#3"),
        CellWrite::text(14, "부"),
    ])
}

fn inspection_row(no: u32, site: &str) -> TemplateRow {
    TemplateRow::new([
        CellWrite::text(2, no.to_string()),
        CellWrite::text(3, site),
        CellWrite::text(4, format!("검사차량-{no:02}")),
        CellWrite::text(5, format!("#{no:03}")),
        CellWrite::text(6, "S33D"),
        CellWrite::text(7, format!("SERIAL-{no:02}")),
        CellWrite::text(8, "정기검사"),
        CellWrite::text(9, "2026.06.01~2026.07.31"),
        CellWrite::text(12, "비고"),
    ])
}

#[test]
fn daily_status_descriptor_matches_real_template_sections() {
    assert_eq!(DAILY_STATUS_TEMPLATE.sheet_name(), "6월05일");
    assert_eq!(DAILY_STATUS_TEMPLATE.sections().len(), 4);

    let results = DAILY_STATUS_TEMPLATE
        .section(DailyStatusSection::Results)
        .expect("results section descriptor");
    assert_eq!(results.title_row(), 2);
    assert_eq!(results.header_row(), 3);
    assert_eq!(results.first_data_row(), 4);
    assert_eq!(results.last_data_row(), 22);
    assert_eq!(results.first_column(), 1);
    assert_eq!(results.last_column(), 13);

    let plans = DAILY_STATUS_TEMPLATE
        .section(DailyStatusSection::Plans)
        .expect("plans section descriptor");
    assert_eq!(plans.title_row(), 24);
    assert_eq!(plans.header_row(), 25);
    assert_eq!(plans.first_data_row(), 26);
    assert_eq!(plans.last_data_row(), 42);

    let pending = DAILY_STATUS_TEMPLATE
        .section(DailyStatusSection::PendingBacklog)
        .expect("pending section descriptor");
    assert_eq!(pending.title_row(), 44);
    assert_eq!(pending.header_row(), 45);
    assert_eq!(pending.first_data_row(), 46);
    assert_eq!(pending.last_data_row(), 75);
    assert_eq!(pending.last_column(), 14);

    let inspections = DAILY_STATUS_TEMPLATE
        .section(DailyStatusSection::PeriodicInspection)
        .expect("inspection section descriptor");
    assert_eq!(inspections.title_row(), 77);
    assert_eq!(inspections.header_row(), 78);
    assert_eq!(inspections.first_data_row(), 79);
    assert_eq!(inspections.last_data_row(), 91);
    assert_eq!(inspections.first_column(), 2);
    assert_eq!(inspections.last_column(), 12);
}

#[test]
fn fills_each_daily_status_section_and_clears_stale_template_rows() {
    let input = fixture_bytes("일일업무진행현황_0605.xlsx");
    let original = load_from_bytes(&input);
    let original_ranges = merged_ranges(sheet(&original, "6월05일"));

    let output = fill_template_bytes(
        &input,
        &DAILY_STATUS_TEMPLATE,
        &[
            SectionFill::new(
                DailyStatusSection::Results,
                vec![daily_status_row(1, "실적")],
            ),
            SectionFill::new(DailyStatusSection::Plans, vec![daily_status_row(1, "계획")]),
            SectionFill::new(
                DailyStatusSection::PendingBacklog,
                vec![pending_backlog_row(1)],
            ),
            SectionFill::new(
                DailyStatusSection::PeriodicInspection,
                vec![inspection_row(1, "검사사업장")],
            ),
        ],
    )
    .expect("daily-status fill should succeed");

    let book = load_from_bytes(&output);
    let ws = sheet(&book, "6월05일");

    assert_eq!(merged_ranges(ws), original_ranges);
    assert_eq!(cell_value(ws, 4, 4), "실적-사업장");
    assert_eq!(cell_value(ws, 4, 26), "계획-사업장");
    assert_eq!(cell_value(ws, 4, 46), "미결-01");
    assert_eq!(cell_value(ws, 3, 79), "검사사업장");
    assert_eq!(
        cell_value(ws, 4, 5),
        "",
        "section 1 stale row was not cleared"
    );
    assert_eq!(
        cell_value(ws, 4, 27),
        "",
        "section 2 stale row was not cleared"
    );
    assert_eq!(
        cell_value(ws, 4, 47),
        "",
        "section 3 stale row was not cleared"
    );
    assert_eq!(
        cell_value(ws, 3, 80),
        "",
        "section 4 stale row was not cleared"
    );
    assert_eq!(cell_value(ws, 2, 77), "4. 정기검사");
    assert!(cell_value(ws, 6, 1).contains('◈'));
    assert!(ws.highest_row() >= 97);
}

#[test]
fn pending_backlog_overflow_inserts_rows_before_section_4_and_shifts_merges() {
    let input = fixture_bytes("일일업무진행현황_0605.xlsx");
    let pending_rows = (1..=32).map(pending_backlog_row).collect::<Vec<_>>();

    let output = fill_template_bytes(
        &input,
        &DAILY_STATUS_TEMPLATE,
        &[
            SectionFill::new(DailyStatusSection::PendingBacklog, pending_rows),
            SectionFill::new(
                DailyStatusSection::PeriodicInspection,
                vec![inspection_row(1, "이동된검사")],
            ),
        ],
    )
    .expect("overflow fill should succeed");

    let book = load_from_bytes(&output);
    let ws = sheet(&book, "6월05일");
    let ranges = merged_ranges(ws);

    assert_eq!(cell_value(ws, 2, 46), "1");
    assert_eq!(cell_value(ws, 2, 77), "32");
    assert_eq!(cell_value(ws, 4, 77), "미결-32");
    assert_eq!(cell_value(ws, 2, 79), "4. 정기검사");
    assert_eq!(cell_value(ws, 2, 80), "순번");
    assert_eq!(cell_value(ws, 3, 81), "이동된검사");
    assert_eq!(cell_value(ws, 2, 99), "외주");

    assert_eq!(ranges.len(), 16);
    assert!(ranges.contains("F1:J1"));
    assert!(ranges.contains("L2:M2"));
    assert!(!ranges.contains("I79:K79"));
    assert!(ranges.contains("I80:K80"));
    assert!(ranges.contains("I81:K81"));
    assert!(ranges.contains("I93:K93"));
    assert!(ws.highest_row() >= 99);
}

#[test]
fn daily_log_workbook_loads_and_roundtrips_with_three_sheet_structure_intact() {
    let input = fixture_bytes("업무일지_26.05.27.xlsx");
    let output = roundtrip_workbook_bytes(&input).expect("daily-log round-trip should succeed");
    let book = load_from_bytes(&output);

    let daily = sheet(&book, "05월 27일");
    assert_eq!(daily.highest_column(), 38);
    assert_eq!(daily.highest_row(), 151);
    assert_eq!(merged_ranges(daily).len(), 59);

    let battery = sheet(&book, "비엔스틸라 #68배터리 점검 결과");
    assert_eq!(battery.highest_column(), 30);
    assert_eq!(battery.highest_row(), 35);
    assert_eq!(merged_ranges(battery).len(), 25);

    let monthly = sheet(&book, "2026.05월(계획)");
    assert_eq!(monthly.highest_column(), 17);
    assert_eq!(monthly.highest_row(), 50);
    assert_eq!(merged_ranges(monthly).len(), 143);
}
