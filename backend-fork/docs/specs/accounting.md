> **QUARRY / NON-AUTHORITY.** Idea or draft only. Cannot dispatch work, clear HOLDs, or override product scope. Current authority: repository README + [`docs/current/PRODUCT.md`](../current/PRODUCT.md) / ROADMAP / DELIVERY.

# Accounting / 회계 Sub-Spec (G013)

Status: regulated foundation slice. This is an accounting implementation contract, not tax/accounting advice. Production financial statements, VAT filing support, and e-tax-invoice issuance remain blocked until a licensed 세무사 validates golden cases and signs the release gate.

## Official source facts checked on 2026-06-27

- NTS VAT guidance lists the general taxpayer VAT rate for all industries as 10%. Source: <https://www.nts.go.kr/nts/cm/cntnts/cntntsView.do?cntntsId=7696&mi=2275>
- NTS VAT reporting guidance defines 6-month VAT tax periods with corporate interim/final filing cadence. Source: <https://www.nts.go.kr/nts/cm/cntnts/cntntsView.do?cntntsId=7693&mi=2272>
- VAT Act law text requires tax invoice supplier/recipient/amount/date fields and sets VAT at 10%. Source: <https://www.law.go.kr/LSW/lsLinkCommonInfo.do?lsJoLnkSeq=1022606733>
- NTS e-tax-invoice guidance says issuance requires an electronic signature and one of business-universal, e-tax-invoice, or ASP common certificates; HomeTax/manual issuance and registered ERP/ASP issuance are described, and non-HomeTax systems transmit issued invoices to the NTS invoice-management system by the next day. Source: <https://www.nts.go.kr/nts/cm/cntnts/cntntsView.do?cntntsId=7788&mi=2462>
- NTS system-operator guidance requires system registration, standard certification, XML/schema/security validation, and HomeTax registration of certificate/public-key/dedicated inbox details before system transmission. Source: <https://www.nts.go.kr/nts/na/ntt/selectNttInfo.do?bbsId=1120&mi=2205&nttSn=1351085>
- Current product policy remains government-direct/no unverified commercial ASP. Therefore this slice models an e-tax-invoice relay envelope and readiness gate, but does not fake issuance or call any unverified API. If product policy later accepts an 국세청-registered ASP gateway as the official channel, that must be a separate decision and test-gated adapter.

## Scope

Included now:

- Pure double-entry journal validation: every entry must have at least two positive lines and equal debit/credit totals.
- Standard 10% VAT calculation and source-versioned VAT rate record.
- Sales/AR posting: 세금계산서 draft -> debit AR, credit revenue, credit VAT payable.
- Procurement/AP posting: vendor invoice/거래명세표 -> debit inventory or expense, debit VAT receivable, credit AP.
- Inventory posting: stock movement cannot go negative; work-order part consumption decrements stock and posts cost to COGS/work-order cost.
- E-tax-invoice relay boundary: generate an auditable relay envelope only when a verified NTS/registered-ERP protocol, production credential, and certificate are present.
- 세무사 validation release gate and golden-case test model.

Excluded until follow-up slices:

- Persistent GL tables, fiscal periods, closing, reporting statements, and web UI.
- Full HomeTax/e세로 adapter, certificate storage, XML signing, and sandbox issuance/query.
- Automatic VAT filing, input-tax disallowance rules, simplified taxpayer rules, exports/zero-rate/exempt flows, withholding, and multi-currency.
- Full PO/GRN/3-way match workflow and price variance accounting.

## Invariants

- Accounting numbers are in integer KRW; no floating-point money.
- Journal entries never post unless balanced.
- VAT, VAT periods, and e-tax invoices carry source URLs and effective dates.
- E-tax invoice issuance is a signed sensitive action: passkey step-up, audit log, and immutable source document id are required in the future REST layer.
- Consolidated group views aggregate per-법인 ledgers; they do not bypass per-org RLS.

## G025 console integration contract

- `/financial` is an operating console for purchase requests, rental quotes, equipment cost ledger, and asset lifecycle economics. It must keep source-object links to equipment, work orders, evidence, approval/payment line state, and cost-ledger outcomes instead of acting as a standalone form.
- Purchase execution is not an accounting close. It may append an equipment cost-ledger event and audit transition, but GL/AP/VAT posting remains gated by the ERP/accounting release rules above.
- Rental pricing and asset sell/keep/acquire signals are deterministic review aids. They must not automatically change prices, sell assets, issue invoices, or bypass approval/policy workflows.
- Any future action that has signature-grade effect—전자세금계산서 issuance, asset sale/disposal, payment execution, write-off, or regulated accounting close—requires passkey step-up, source document lineage, immutable audit evidence, and professional/legal release gates before the UI may label it enforced.
