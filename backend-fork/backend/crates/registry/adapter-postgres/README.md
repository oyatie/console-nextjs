# console-registry-adapter-postgres

Postgres registry adapter plus the Excel master-list importer.

The importer reads `docs/reference/master-list_251120.xlsx` with `calamine`
and imports only the two source sheets:

- `K&L 지게차 Master list`: header row 3, data rows 4 through 447.
- `예비 및 여유차량`: header row 4, valid equipment rows through row 61.

Sheets 3 and 4 are pivot/summary sheets and are not imported.

Imported equipment is assigned to branch `HQ`. If roster provisioning has not
created that branch yet, the importer creates region `HQ` and branch `HQ`
before the audited import transaction.
