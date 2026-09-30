-- no-transaction
-- HR read-path performance: 조직도 expression order.
-- Keep one CONCURRENTLY statement per no-transaction migration; see 0084.

CREATE INDEX CONCURRENTLY employees_org_chart_order_idx
    ON employees (
        org_id,
        company,
        (COALESCE(NULLIF(org_unit, ''), '소속 미지정')),
        (COALESCE(NULLIF(position, ''), '직책 미지정')),
        name,
        source_sheet,
        source_row
    );
