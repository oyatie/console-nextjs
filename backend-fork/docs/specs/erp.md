> **QUARRY / NON-AUTHORITY.** Idea or draft only. Cannot dispatch work, clear HOLDs, or override product scope. Current authority: repository README + [`docs/current/PRODUCT.md`](../current/PRODUCT.md) / ROADMAP / DELIVERY.

# ERP Foundation Spec (G013)

ERP is split into bounded contexts that share accounting postings rather than one monolith.

1. **Accounting/GL** owns journal validation, chart-of-accounts semantics, VAT source tables, period close, and financial statements.
2. **Sales/AR** owns quote -> order -> tax-invoice draft -> receivable -> receipt lifecycle. It emits balanced GL postings; it does not own VAT law.
3. **Procurement/AP** owns vendor master, PO -> receipt -> vendor invoice/거래명세표 -> payable -> payment lifecycle. It emits balanced GL postings.
4. **Inventory** owns item master, stock movements, average/FIFO cost policy, work-order consumption, and cost-ledger events. It emits inventory/COGS postings.
5. **E-tax relay** owns NTS/HomeTax/registered-ERP protocol feasibility, XML/signature envelope, certificate custody, issue/query/void status, and retry/outbox. It may not issue anything until official protocol and credentials are verified.

First slice deliverable: `console-erp-domain` pure kernel + tests. DB/API/UI follow after the domain contract and 세무사-reviewed golden cases are stable.
