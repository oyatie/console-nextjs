# Connected Sheet Architecture & Spreadsheet Governance Charter
**System:** Acme Group / Oyatie Console (`oyatie/console`)  
**Domain:** Business Object Connected Sheet, Cell Role Semantics, and Governed Commitment  
**Standard:** Palantir Foundry Workshop, Workday HCM, SAP Fiori, Bloomberg Terminal, Microsoft Excel / Google Sheets  
**Status:** Authoritative Production Specification  

---

## 1. Executive Charter & Core Insight
A sheet is **not** an isolated grid that detaches users from the business database; rather, it is a familiar, high-velocity lens for interacting with live **Business Objects**, with room for independent analysis.

Users must obtain the speed, ergonomics, and spatial familiarity of a spreadsheet while strictly retaining the identity, relationships, permissions, and audit history of the underlying records.

### The Fundamental Distinction
Console enforces a strict architectural boundary across three layers:
1. **What a cell displays**: The authoritative projection or local computed presentation.
2. **What the user is proposing**: An editable draft input or analytical assumption.
3. **What has become authoritative business state**: Owner-calculated, approved, and sealed state.

These distinctions are communicated naturally through column headers, visual affordances, and a compact inspector—without turning every edit into a bureaucratic roadblock.

---

## 2. A Sheet as Another View of the Same Work
An employment entity appears in:
- The Employee Directory (List view)
- The Worker 360 Profile (Inspector drawer)
- The Organization Structure & Site View (Hierarchy tree)
- The Connected Sheet (Spreadsheet grid)

Each surface refers to the **exact same stable object identity** (`employee.id`, `employment.id`).

```
                              ┌───────────────────────────────────┐
                              │     Stable Object Identity        │
                              │   Person: emp_1001 (송민우)       │
                              │   Contract: EC-2026-003           │
                              │   Assignment: QA팀 (SITE-02)      │
                              └─────────────────┬─────────────────┘
                                                │
                 ┌──────────────────────────────┼──────────────────────────────┐
                 │                              │                              │
                 ▼                              ▼                              ▼
     ┌───────────────────────┐      ┌───────────────────────┐      ┌───────────────────────┐
     │ Employee 360 Dossier  │      │ Org Assignment Matrix │      │    Connected Sheet    │
     │  - Person Identity    │      │  - Division / Site    │      │  - Linked Source      │
     │  - Contract Details   │      │  - Lead / DoA Tier    │      │  - Business Inputs    │
     │  - Consequence Modal  │      │  - Active Tree Prev   │      │  - Calculated Gross   │
     └───────────────────────┘      └───────────────────────┘      └───────────────────────┘
```

A user can filter by department in the sheet, inspect an employee's dossier, and return to the exact selected row without exporting, reconciling internal IDs, or re-importing.

---

## 3. The 6 Explicit Cell Roles & Semantics

| Cell Role | Example | User Experience & Semantics |
| :--- | :--- | :--- |
| **`linked-source`** | Employment start date, base salary, clock-in time | Read-only in sheet. Inspect source record; edit only through authorized HR/attendance workflow. Displays lock icon. |
| **`business-input`** | Proposed payroll allowance, bonus, overtime adjustment | Fully editable in sheet. Automatically preserved in durable draft. Highlights proposed diffs without mutating authoritative state until commit. |
| **`calculated-result`** | Payroll gross amount, statutory deductions, net pay | Computed strictly by domain kernel (`payroll-engine`). Inspect calculation formula and provenance; no arbitrary manual overwrite. |
| **`sheet-formula`** | Comparison with prior period, `=([@gross] - [@base])` | Client-side formula expression evaluated within safe sheet rules. User can customize expressions without changing database schema. |
| **`analytical-input`** | Scenario increase rate, planning notes, audit memo | Edit freely for planning. Does not alter employment or payroll truth unless explicitly promoted to a governed property. |
| **`object-reference`** | Assigned department, target site | Searchable pointer to real business objects. Selection opens reference picker with ambiguity resolution. |

---

## 4. The 3 Levels of Spreadsheet Flexibility
1. **Personal or Shared Views**:
   - Sorting, filtering, column order, column widths, and pinned snapshots.
   - Sorting rearranges the visual display only; it **never** alters organizational rank or database primary keys.
2. **Analytical Work**:
   - Formulas, assumptions, scenarios (e.g. "+5% budget scenario"), comparison columns.
   - Belongs to the user's workspace session or saved draft.
3. **Business Changes**:
   - Typed edits that propose mutations to authoritative business properties.
   - Follows the universal cycle:
     $$\text{Grid Edit} \longrightarrow \text{Typed Proposed Change} \longrightarrow \text{Consequence Summary} \longrightarrow \text{Owner Approval} \longrightarrow \text{Audited Commit}$$

---

## 5. Interaction Vocabulary & Semantics
- **Movement**: Arrow keys, Tab, Shift+Tab, Enter.
- **Editing**: Single click to select; double-click or alphanumeric typing to edit; Escape to cancel.
- **Formula Bar**: Displays current coordinate (e.g. `E4`), role badge (`[비즈니스 입력]`), and expression/value editor.
- **Fill Handle**: Dragging fill handle across business inputs (e.g. allowances) proposes values sequentially; dragging across status/approved columns is strictly non-destructive.
- **First-Class Paste Workflow**:
  - Rectangular TSV pasting (from Excel / Google Sheets).
  - Unambiguous currency interpretation (`1,250,000`, `₩1,250,000`, `125만`).
  - Object reference disambiguation when multiple departments match input strings.
  - Invalid cells highlighted without discarding valid cells in the paste block.

---

## 6. Durable Drafts vs. Consequential Commitment
- Autosave is honest: the UI communicates `초안 보존됨 · 제안 변경 3건 (+₩600,000)`. It does not claim "저장됨" if uncommitted.
- Commitment requires examining consequences via `ConsequenceModal`:
  - Quantified financial variance (Gross, Deductions, Net).
  - Affected headcounts and individual line-item diffs.
  - Plain-language legal and banking impact before execution.
- Passkey biometric signing permanently seals bank batches into immutable SHA-256 audit hash chains.
