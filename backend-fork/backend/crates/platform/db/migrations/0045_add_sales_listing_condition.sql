-- Sales listing condition (#27 판매 카테고리 재구성): split the storefront sales
-- catalog into 중고 (USED) and 신차 (NEW) sub-categories.
--
-- A "listing" already carries fuel/drive `kind`, sale/rental `listing_type`, and a
-- publication `status`. This adds an orthogonal structured `condition` axis so the
-- public storefront can filter 중고 vs 신차 and the admin console can publish a
-- genuine new-condition (신차) listing — distinct from the free-text
-- `condition_label` display copy (e.g. "검수 완료", "A급"), which stays as-is.
--
-- Modelled exactly like the existing TEXT + CHECK enums on this table
-- (kind/listing_type/status): a bounded TEXT column, never a native pg enum.
-- Every existing row predates the split and describes used equipment, so the
-- column is NOT NULL DEFAULT 'USED' and the (zero-or-more) existing rows backfill
-- to 'USED' via that default.

-- console-gate: audited-table sales_listings
ALTER TABLE sales_listings
    ADD COLUMN condition TEXT NOT NULL DEFAULT 'USED'
                         CHECK (condition IN ('USED', 'NEW'));

-- The public catalog filters 중고/신차 within the storefront-visible set, newest
-- and weightiest first — mirrors idx_sales_listings_org_public with condition.
CREATE INDEX idx_sales_listings_org_condition
    ON sales_listings (org_id, condition, status, sort_weight DESC, created_at DESC);
