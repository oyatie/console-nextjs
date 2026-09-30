# Attendance source fidelity

Base: `87212b127503e4f7164a009d6c38e138f075af48` in the independent
`oyatie/console-nextjs` repository. Owner: HR attendance import in
`backend-fork/backend/app/src/hr.rs`. Status: design awaiting independent review.

The existing native path is multipart preview → stored typed rows → dry-run
matching → checklist-gated apply → PostgreSQL import-event readback. It remains
`lineage_only_not_payable`; this change creates no appointment, authoritative
attendance, payroll liability or payment.

Observable outcome:

- CSV source rows identify original CSV records, including blank records before
  the header and between data. A newline within a quoted field is part of one
  record. Blank records do not become imported facts. Preserve LF, CRLF and CR
  behavior and do not invent a final record after a terminating newline.
  Keep the existing header search budget of the first 25 nonempty CSV records;
  attach each original record number before filtering, rather than shifting
  header-search acceptance by passing empty records to that search.
- XLSX source rows identify actual worksheet rows using Calamine's range origin,
  including leading unused rows and gaps. Preview, stored rows and applied
  event source keys all use the same coordinates.
- Reject repeated nonempty normalized header names and multiple aliases mapping
  to the same canonical field before mapping or persistence. This prevents
  overwriting raw values or arbitrarily choosing one value. Required headers
  and valid distinct columns continue to work; error text contains no source
  cell values. A rejected upload creates no run, row, event or preview audit.
- Re-importing the same attendance facts does not duplicate events. Existing
  fact identity remains unchanged; prior stored source keys
  and source hashes are never rewritten or rehashed.
  A historical compacted source key can collide with a corrected coordinate
  for a different fact. Compare the stored fact key after current matching:
  report a blocking `source_coordinate_conflict`, distinct from a duplicate,
  when the keys disagree. Preserve the old evidence and require repair; never
  insert, overwrite, or claim this collision proves the same fact.

Implementation carries explicit source row numbers from the two callers of
`parse_attendance_tabular_sheet` through that shared parser, and extends the
existing duplicate source lookup with its stored fact key. There is no new parser library, data owner, schema,
credential, role, payable behavior, or enabled prototype screen. The header
scan remains bounded relative to the parsed range/CSV records. Nonattendance
employee imports are separate callers and are unchanged.

Tests first: extend the existing real-router PostgreSQL target
`console-app --test hr_ingest_checklist_gate`, using its genuine `console_rt`
pool and signed test authentication. Exercise CSV and XLSX multipart preview,
persisted coordinates, dry-run, apply/readback, duplicate and historical-coordinate re-import, ambiguous
header rejection with zero writes, and cross-Company denial. Parser unit
checks cover newline styles, quoted newlines, range origin and header aliases.
Test fixtures belong exclusively under `backend/app/tests/fixtures`.

Pre-mortem: a coordinate fix changes new source references but misses applied
events; a mapping refusal occurs after writes; a re-import creates duplicates;
or tests claim browser/authentication proof they never ran. Detect these through
exact persisted row/key/hash assertions, zero-write checks and discovery counts.
Blast radius is new attendance imports only. Existing business rows and migration
bytes remain unchanged. Rollback before deployment reverts code/tests; after
deployment preserve all accepted source references rather than rewriting them.
Stop on a duplicate applied fact, changed historical evidence, permission leak,
or apparent payable/durable-success claim.

Still unresolved: original immutable file custody, omitted unrecognized XLSX
tabs and unnamed/beyond-header cells, mapping repair UI, employee-import coordinate/identity compatibility,
effective Employment serving, approved full-population payroll publication,
verified natural-person approval, authenticated Next workflows, two-site
confirmation/fencing and full B01/B04/B06/P02 acceptance. No live exposure,
institutional operation or settlement is authorized by this slice.
