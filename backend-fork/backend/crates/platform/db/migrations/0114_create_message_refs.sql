-- Messenger object-code references, parsed on write (BE-OBJ slice 2, item 2).
--
-- messenger_messages.body is plain TEXT; the design's `#`-object tokens (e.g.
-- `#WO-20260701-003`, `#AP-12`) render on the client but never survive as live,
-- persisted references. This table captures them at post time so an object's
-- inbound "referenced by" chain and object-graph traversal have a real backend
-- edge, mirroring the existing `@`-mention parse (extract_mention_user_ids).
--
-- Unlike `@`-mentions, `#`-refs carry NO notification (DESIGN §4.7-7: `#` = 알림
-- 없음). Only tokens whose prefix matches a known object_types.code_prefix are
-- stored, so free-form `#hashtag` noise is dropped (the prefix is validated;
-- the referenced target's existence is a read-time, deny-by-omission concern).

-- console-gate: audited-table message_refs
CREATE TABLE message_refs (
    id         UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id     UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    message_id UUID        NOT NULL REFERENCES messenger_messages(id) ON DELETE CASCADE,
    -- The object kind the code's prefix resolved to (FK'd to the seeded
    -- registry, so a ref always names a known kind).
    ref_kind   TEXT        NOT NULL REFERENCES object_types(kind) ON DELETE RESTRICT,
    -- The raw code token as written (e.g. 'WO-20260701-003'), ≤200 chars like
    -- an object id. The target is resolved (and access-gated) at read time.
    ref_code   TEXT        NOT NULL CHECK (char_length(btrim(ref_code)) BETWEEN 1 AND 200),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- One row per distinct code per message (a code written twice links once).
    UNIQUE (org_id, message_id, ref_code)
);

-- Reverse lookup: "which messages reference this code" (the inbound chain).
CREATE INDEX idx_message_refs_code
    ON message_refs (org_id, ref_kind, ref_code);

ALTER TABLE message_refs ENABLE ROW LEVEL SECURITY;
ALTER TABLE message_refs FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON message_refs
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

-- Refs are written once with their message (parse-on-write) and read back; they
-- are never mutated, and are removed only by the message's ON DELETE CASCADE.
GRANT SELECT, INSERT ON message_refs TO console_rt;
