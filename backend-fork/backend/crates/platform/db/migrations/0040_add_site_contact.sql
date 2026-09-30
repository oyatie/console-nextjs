-- Issue #13 — customer-site registration: give registry_sites a representative
-- contact (대표 담당자).
--
-- registry_sites carried only a name + the geography from 0039. Dispatching a
-- mechanic to a site needs an on-site point of contact — a name (담당자명), a
-- phone (연락처), and optionally an email — so the field tech and the dispatcher
-- know who to call on arrival. EVERY column added here is NULLable and there is
-- NO backfill: a site gains a contact only once an admin enters it through
-- PATCH /api/v1/sites/{id} (EquipmentManage). Until then the contact is simply
-- absent rather than fabricated.
--
-- registry_sites already carries org_id, a FORCE-ROW-LEVEL-SECURITY
-- `org_isolation` policy (0030), the shared immutable-org trigger (0031), and the
-- (id, org_id) unique key (0034). New columns inherit that row policy, so this
-- migration adds NO new policy, trigger, or org column — only the contact columns
-- and their bounded-text CHECKs.
--
-- console-gate: audited-table registry_sites
ALTER TABLE registry_sites
    ADD COLUMN contact_name  TEXT,   -- 담당자명 (representative-in-charge name)
    ADD COLUMN contact_phone TEXT,   -- 연락처 (phone, free-form: 010-0000-0000)
    ADD COLUMN contact_email TEXT;   -- 이메일 (optional)

-- Bounded-text length CHECKs (defense-in-depth, mirroring 0039_add_site_geography
-- and 0025_bounded_text_constraints). The PATCH /sites handler bounds these in
-- the domain layer (crates/kernel/core/src/validation.rs); these constraints are
-- the backstop so the bound still holds if a future code path ever writes to
-- registry_sites without going through that validation. char_length() counts
-- Unicode code points, matching the Rust `str::chars().count()` bound exactly.
-- All checks pass on NULL, since every contact column is optional.
ALTER TABLE registry_sites
    ADD CONSTRAINT registry_sites_contact_name_max_chars
        CHECK (contact_name IS NULL OR char_length(contact_name) <= 100),
    ADD CONSTRAINT registry_sites_contact_phone_max_chars
        CHECK (contact_phone IS NULL OR char_length(contact_phone) <= 40),
    ADD CONSTRAINT registry_sites_contact_email_max_chars
        CHECK (contact_email IS NULL OR char_length(contact_email) <= 320);
