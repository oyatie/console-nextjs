-- Defense-in-depth: DB-level length bounds on support free-text columns.
--
-- The application already bounds these fields in two places: the unauthenticated
-- customer-intake handler rejects over-length input at the edge
-- (crates/support/rest/src/lib.rs), and the store re-checks them before any
-- INSERT (crates/support/adapter-postgres/src/lib.rs, require_max_chars). These
-- CHECK constraints are a backstop so the bound still holds if a future code
-- path ever writes to support_tickets without going through that validation.
--
-- The limits below mirror the Rust constants EXACTLY (counts must match):
--   MAX_TITLE_CHARS             = 200  -> title
--   MAX_BODY_CHARS              = 8000 -> body
--   MAX_REQUESTER_NAME_CHARS    = 200  -> requester_name
--   MAX_REQUESTER_CONTACT_CHARS = 200  -> requester_contact
--
-- The Rust side counts Unicode scalar values on the *trimmed* value via
-- `str::chars().count()`, and stores that same trimmed string. Postgres
-- `char_length()` likewise counts code points, so the two bounds agree
-- character-for-character. requester_name / requester_contact are NULL on the
-- internal channel, so the checks are written to pass on NULL.

-- console-gate: audited-table support_tickets
ALTER TABLE support_tickets
    ADD CONSTRAINT support_tickets_title_max_chars
        CHECK (char_length(title) <= 200),
    ADD CONSTRAINT support_tickets_body_max_chars
        CHECK (char_length(body) <= 8000),
    ADD CONSTRAINT support_tickets_requester_name_max_chars
        CHECK (requester_name IS NULL OR char_length(requester_name) <= 200),
    ADD CONSTRAINT support_tickets_requester_contact_max_chars
        CHECK (requester_contact IS NULL OR char_length(requester_contact) <= 200);
