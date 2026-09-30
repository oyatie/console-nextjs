//! Excel platform adapter.
//!
//! Provides byte-fidelity template filling for the Korean daily-status (일일업무진행현황)
//! Excel form using umya-spreadsheet.
//!
//! This crate exists primarily as the result of the T0.10 viability spike (ADR-0008).
//! T4.1 promotes the spike into a small descriptor-driven template-fill engine.

use std::collections::BTreeSet;
use std::fmt;
use std::io::Cursor;

/// Re-export the underlying spreadsheet engine so callers need not add it as a
/// direct dependency.
pub use umya_spreadsheet;

use umya_spreadsheet::{Workbook, Worksheet};

/// Result alias for Excel template operations.
pub type Result<T> = std::result::Result<T, ExcelTemplateError>;

/// Stable identifier for a fillable template section.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SectionKey(&'static str);

impl SectionKey {
    /// Create a section key for a descriptor.
    #[must_use]
    pub const fn new(value: &'static str) -> Self {
        Self(value)
    }

    /// Return the stable string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for SectionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// Sections in the real daily-status workbook (`일일업무진행현황_0605.xlsx`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DailyStatusSection {
    /// `1. 일일 진행업무(실적)`.
    Results,
    /// `2. 일일 진행 업무(계획)`.
    Plans,
    /// `3. 미결 업무 현황(누적)`.
    PendingBacklog,
    /// `4. 정기검사`.
    PeriodicInspection,
}

impl DailyStatusSection {
    /// Stable descriptor key for the section.
    #[must_use]
    pub const fn key(self) -> SectionKey {
        match self {
            Self::Results => SectionKey::new("daily-status.results"),
            Self::Plans => SectionKey::new("daily-status.plans"),
            Self::PendingBacklog => SectionKey::new("daily-status.pending-backlog"),
            Self::PeriodicInspection => SectionKey::new("daily-status.periodic-inspection"),
        }
    }
}

impl From<DailyStatusSection> for SectionKey {
    fn from(value: DailyStatusSection) -> Self {
        value.key()
    }
}

/// Workbook template descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TemplateDescriptor {
    sheet_name: &'static str,
    sections: &'static [SectionDescriptor],
}

impl TemplateDescriptor {
    /// Create a template descriptor.
    #[must_use]
    pub const fn new(sheet_name: &'static str, sections: &'static [SectionDescriptor]) -> Self {
        Self {
            sheet_name,
            sections,
        }
    }

    /// Worksheet name containing the fillable sections.
    #[must_use]
    pub const fn sheet_name(&self) -> &'static str {
        self.sheet_name
    }

    /// Section descriptors in template order.
    #[must_use]
    pub const fn sections(&self) -> &'static [SectionDescriptor] {
        self.sections
    }

    /// Find a section descriptor by key.
    #[must_use]
    pub fn section<S>(&self, section: S) -> Option<&'static SectionDescriptor>
    where
        S: Into<SectionKey>,
    {
        let key = section.into();
        self.sections.iter().find(|candidate| candidate.key == key)
    }
}

/// Descriptor for a fillable section within a template sheet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SectionDescriptor {
    key: SectionKey,
    title: &'static str,
    title_row: u32,
    header_row: u32,
    first_data_row: u32,
    last_data_row: u32,
    first_column: u32,
    last_column: u32,
}

impl SectionDescriptor {
    /// Create a section descriptor.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub const fn new(
        key: SectionKey,
        title: &'static str,
        title_row: u32,
        header_row: u32,
        first_data_row: u32,
        last_data_row: u32,
        first_column: u32,
        last_column: u32,
    ) -> Self {
        Self {
            key,
            title,
            title_row,
            header_row,
            first_data_row,
            last_data_row,
            first_column,
            last_column,
        }
    }

    /// Stable section key.
    #[must_use]
    pub const fn key(&self) -> SectionKey {
        self.key
    }

    /// Human-readable section title as it appears in the workbook.
    #[must_use]
    pub const fn title(&self) -> &'static str {
        self.title
    }

    /// Row containing the section title.
    #[must_use]
    pub const fn title_row(&self) -> u32 {
        self.title_row
    }

    /// Row containing the section column labels.
    #[must_use]
    pub const fn header_row(&self) -> u32 {
        self.header_row
    }

    /// First row in the section's data range.
    #[must_use]
    pub const fn first_data_row(&self) -> u32 {
        self.first_data_row
    }

    /// Last row in the section's template data range.
    #[must_use]
    pub const fn last_data_row(&self) -> u32 {
        self.last_data_row
    }

    /// First writable column for this section.
    #[must_use]
    pub const fn first_column(&self) -> u32 {
        self.first_column
    }

    /// Last writable column for this section.
    #[must_use]
    pub const fn last_column(&self) -> u32 {
        self.last_column
    }

    fn capacity(self) -> u32 {
        self.last_data_row - self.first_data_row + 1
    }
}

const DAILY_STATUS_SECTIONS: [SectionDescriptor; 4] = [
    SectionDescriptor::new(
        DailyStatusSection::Results.key(),
        "1. 일일 진행업무(실적)",
        2,
        3,
        4,
        22,
        1,
        13,
    ),
    SectionDescriptor::new(
        DailyStatusSection::Plans.key(),
        "2. 일일 진행 업무(계획)",
        24,
        25,
        26,
        42,
        1,
        13,
    ),
    SectionDescriptor::new(
        DailyStatusSection::PendingBacklog.key(),
        "3. 미결 업무 현황(누적)",
        44,
        45,
        46,
        75,
        1,
        14,
    ),
    SectionDescriptor::new(
        DailyStatusSection::PeriodicInspection.key(),
        "4. 정기검사",
        77,
        78,
        79,
        91,
        2,
        12,
    ),
];

/// Descriptor for the real daily-status workbook used by T4 reporting exports.
pub const DAILY_STATUS_TEMPLATE: TemplateDescriptor =
    TemplateDescriptor::new("6월05일", &DAILY_STATUS_SECTIONS);

/// A value to write into a template cell.
#[derive(Clone, Debug, PartialEq)]
pub enum CellValue {
    /// String cell value.
    Text(String),
    /// Numeric cell value.
    Number(f64),
    /// Boolean cell value.
    Bool(bool),
    /// Empty cell value.
    Blank,
}

/// One cell write within a row.
#[derive(Clone, Debug, PartialEq)]
pub struct CellWrite {
    column: u32,
    value: CellValue,
}

impl CellWrite {
    /// Create a string cell write.
    #[must_use]
    pub fn text<S>(column: u32, value: S) -> Self
    where
        S: Into<String>,
    {
        Self {
            column,
            value: CellValue::Text(value.into()),
        }
    }

    /// Create a numeric cell write.
    #[must_use]
    pub fn number(column: u32, value: f64) -> Self {
        Self {
            column,
            value: CellValue::Number(value),
        }
    }

    /// Create a boolean cell write.
    #[must_use]
    pub fn boolean(column: u32, value: bool) -> Self {
        Self {
            column,
            value: CellValue::Bool(value),
        }
    }

    /// Create an empty cell write.
    #[must_use]
    pub fn blank(column: u32) -> Self {
        Self {
            column,
            value: CellValue::Blank,
        }
    }

    /// Target column, 1-based.
    #[must_use]
    pub const fn column(&self) -> u32 {
        self.column
    }

    /// Cell value to write.
    #[must_use]
    pub const fn value(&self) -> &CellValue {
        &self.value
    }
}

/// One logical row to write into a section.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TemplateRow {
    cells: Vec<CellWrite>,
}

impl TemplateRow {
    /// Create a row from cell writes.
    #[must_use]
    pub fn new<I>(cells: I) -> Self
    where
        I: IntoIterator<Item = CellWrite>,
    {
        Self {
            cells: cells.into_iter().collect(),
        }
    }

    /// Cell writes in this row.
    #[must_use]
    pub fn cells(&self) -> &[CellWrite] {
        &self.cells
    }
}

/// Rows to write into one descriptor section.
#[derive(Clone, Debug, PartialEq)]
pub struct SectionFill {
    section: SectionKey,
    rows: Vec<TemplateRow>,
}

impl SectionFill {
    /// Create a section fill request.
    #[must_use]
    pub fn new<S>(section: S, rows: Vec<TemplateRow>) -> Self
    where
        S: Into<SectionKey>,
    {
        Self {
            section: section.into(),
            rows,
        }
    }

    /// Target section key.
    #[must_use]
    pub const fn section(&self) -> SectionKey {
        self.section
    }

    /// Rows to write.
    #[must_use]
    pub fn rows(&self) -> &[TemplateRow] {
        &self.rows
    }
}

/// Errors produced by template-fill operations.
#[derive(Debug)]
pub enum ExcelTemplateError {
    /// The input workbook could not be loaded.
    ReadWorkbook(umya_spreadsheet::XlsxError),
    /// The output workbook could not be written.
    WriteWorkbook(umya_spreadsheet::XlsxError),
    /// The descriptor sheet was not present in the workbook.
    MissingSheet { sheet_name: String },
    /// A fill request referenced a section absent from the descriptor.
    UnknownSection { section: SectionKey },
    /// Multiple fill requests targeted the same section.
    DuplicateSection { section: SectionKey },
    /// The descriptor itself is internally inconsistent.
    InvalidDescriptor { message: String },
    /// A row attempted to write outside the section's declared columns.
    CellOutOfRange {
        section: SectionKey,
        column: u32,
        first_column: u32,
        last_column: u32,
    },
}

impl fmt::Display for ExcelTemplateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadWorkbook(error) => write!(f, "failed to read workbook: {error}"),
            Self::WriteWorkbook(error) => write!(f, "failed to write workbook: {error}"),
            Self::MissingSheet { sheet_name } => {
                write!(f, "template sheet '{sheet_name}' was not found")
            }
            Self::UnknownSection { section } => {
                write!(f, "template section '{section}' is not in the descriptor")
            }
            Self::DuplicateSection { section } => {
                write!(f, "template section '{section}' was filled more than once")
            }
            Self::InvalidDescriptor { message } => {
                write!(f, "invalid template descriptor: {message}")
            }
            Self::CellOutOfRange {
                section,
                column,
                first_column,
                last_column,
            } => write!(
                f,
                "column {column} is outside section '{section}' writable range {first_column}..={last_column}"
            ),
        }
    }
}

impl std::error::Error for ExcelTemplateError {}

/// Load a workbook from bytes, fill descriptor sections, and return XLSX bytes.
pub fn fill_template_bytes(
    template_bytes: &[u8],
    descriptor: &TemplateDescriptor,
    fills: &[SectionFill],
) -> Result<Vec<u8>> {
    validate_descriptor(descriptor)?;

    let mut workbook = read_workbook_bytes(template_bytes)?;
    let ordered_fills = ordered_fills(descriptor, fills)?;
    let worksheet = workbook
        .sheet_by_name_mut(descriptor.sheet_name())
        .map_err(|_| ExcelTemplateError::MissingSheet {
            sheet_name: descriptor.sheet_name().to_owned(),
        })?;

    let mut row_shift = 0u32;
    for (section, fill) in ordered_fills {
        let first_row = section.first_data_row + row_shift;
        let mut last_row = section.last_data_row + row_shift;
        let capacity = section.capacity();
        let row_count = u32::try_from(fill.rows().len()).map_err(|_| {
            ExcelTemplateError::InvalidDescriptor {
                message: format!("section '{}' has more than u32::MAX rows", section.key()),
            }
        })?;

        if row_count > capacity {
            let extra_rows = row_count - capacity;
            let insert_at = last_row + 1;
            worksheet.insert_new_row(insert_at, extra_rows);
            copy_inserted_row_layout(
                worksheet,
                last_row,
                insert_at,
                extra_rows,
                section.first_column,
                section.last_column,
            );
            last_row += extra_rows;
            row_shift += extra_rows;
        }

        clear_section_values(
            worksheet,
            first_row,
            last_row,
            section.first_column,
            section.last_column,
        );
        write_section_rows(worksheet, *section, first_row, fill.rows())?;
    }

    write_workbook_bytes(&workbook)
}

/// Load and emit a workbook without edits, used as a structural round-trip guard.
pub fn roundtrip_workbook_bytes(template_bytes: &[u8]) -> Result<Vec<u8>> {
    let workbook = read_workbook_bytes(template_bytes)?;
    write_workbook_bytes(&workbook)
}

fn read_workbook_bytes(template_bytes: &[u8]) -> Result<Workbook> {
    umya_spreadsheet::reader::xlsx::read_reader(Cursor::new(template_bytes), true)
        .map_err(ExcelTemplateError::ReadWorkbook)
}

fn write_workbook_bytes(workbook: &Workbook) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    umya_spreadsheet::writer::xlsx::write_writer(workbook, &mut output)
        .map_err(ExcelTemplateError::WriteWorkbook)?;
    Ok(output)
}

fn ordered_fills<'a>(
    descriptor: &'a TemplateDescriptor,
    fills: &'a [SectionFill],
) -> Result<Vec<(&'static SectionDescriptor, &'a SectionFill)>> {
    let mut seen = BTreeSet::new();
    let mut ordered = Vec::with_capacity(fills.len());

    for fill in fills {
        let section_key = fill.section();
        if !seen.insert(section_key) {
            return Err(ExcelTemplateError::DuplicateSection {
                section: section_key,
            });
        }
        let section =
            descriptor
                .section(section_key)
                .ok_or(ExcelTemplateError::UnknownSection {
                    section: section_key,
                })?;
        ordered.push((section, fill));
    }

    ordered.sort_by_key(|(section, _)| section.first_data_row());
    Ok(ordered)
}

fn validate_descriptor(descriptor: &TemplateDescriptor) -> Result<()> {
    if descriptor.sheet_name().is_empty() {
        return Err(ExcelTemplateError::InvalidDescriptor {
            message: "sheet name is empty".to_owned(),
        });
    }

    let mut previous_last_row = 0u32;
    let mut seen = BTreeSet::new();
    for section in descriptor.sections() {
        if !seen.insert(section.key()) {
            return Err(ExcelTemplateError::DuplicateSection {
                section: section.key(),
            });
        }
        if section.first_data_row() > section.last_data_row() {
            return Err(ExcelTemplateError::InvalidDescriptor {
                message: format!("section '{}' data range is inverted", section.key()),
            });
        }
        if section.first_column() > section.last_column() {
            return Err(ExcelTemplateError::InvalidDescriptor {
                message: format!("section '{}' column range is inverted", section.key()),
            });
        }
        if section.title_row() >= section.header_row()
            || section.header_row() >= section.first_data_row()
        {
            return Err(ExcelTemplateError::InvalidDescriptor {
                message: format!(
                    "section '{}' rows must be title < header < data",
                    section.key()
                ),
            });
        }
        if section.title_row() <= previous_last_row {
            return Err(ExcelTemplateError::InvalidDescriptor {
                message: format!("section '{}' overlaps a prior section", section.key()),
            });
        }
        previous_last_row = section.last_data_row();
    }

    Ok(())
}

fn copy_inserted_row_layout(
    worksheet: &mut Worksheet,
    source_row: u32,
    insert_at: u32,
    num_rows: u32,
    first_column: u32,
    last_column: u32,
) {
    let source_row_dimension = worksheet.row_dimension(source_row).cloned();

    for row in insert_at..insert_at + num_rows {
        if let Some(source) = source_row_dimension.as_ref() {
            let target = worksheet.row_dimension_mut(row);
            target.set_height(source.height());
            target.set_custom_height(source.custom_height());
            target.set_hidden(source.hidden());
            target.set_thick_bot(source.thick_bot());
            target.set_style(source.style().clone());
        }
        worksheet.copy_row_styling(source_row, row, Some(first_column), Some(last_column));
    }
}

fn clear_section_values(
    worksheet: &mut Worksheet,
    first_row: u32,
    last_row: u32,
    first_column: u32,
    last_column: u32,
) {
    for row in first_row..=last_row {
        for column in first_column..=last_column {
            worksheet.cell_mut((column, row)).set_value("");
        }
    }
}

fn write_section_rows(
    worksheet: &mut Worksheet,
    section: SectionDescriptor,
    first_row: u32,
    rows: &[TemplateRow],
) -> Result<()> {
    for (offset, row) in rows.iter().enumerate() {
        let target_row = first_row
            + u32::try_from(offset).map_err(|_| ExcelTemplateError::InvalidDescriptor {
                message: format!("section '{}' row offset overflowed", section.key()),
            })?;
        for cell in row.cells() {
            if cell.column() < section.first_column() || cell.column() > section.last_column() {
                return Err(ExcelTemplateError::CellOutOfRange {
                    section: section.key(),
                    column: cell.column(),
                    first_column: section.first_column(),
                    last_column: section.last_column(),
                });
            }
            write_cell(worksheet, cell, target_row);
        }
    }
    Ok(())
}

fn write_cell(worksheet: &mut Worksheet, cell: &CellWrite, target_row: u32) {
    let target = worksheet.cell_mut((cell.column(), target_row));
    match cell.value() {
        CellValue::Text(value) => {
            target.set_value(value);
        }
        CellValue::Number(value) => {
            target.set_value_number(*value);
        }
        CellValue::Bool(value) => {
            target.set_value_bool(*value);
        }
        CellValue::Blank => {
            target.set_value("");
        }
    }
}
