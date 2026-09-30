-- In-console office editor: immutable document version domain (HANDOFF §12,
-- Euro-Office slice 0).
--
-- The host (console) — never the editor — owns storage, versions, PBAC, and
-- audit. Every save the DocumentServer force-saves back through the callback
-- lands here as an APPEND-ONLY new version row (v1, v2, …). Rollback is
-- non-destructive: it re-publishes an earlier version as a NEW version
-- (`restored_from` records the lineage), it never mutates or deletes a row.
--
-- Honest object-world anchor: there is no `documents` table yet (records-archive
-- is a named backend GAP — 04-backend-contract §Docs/Policy/Inbox/Audit). Rather
-- than invent one for this slice, a version is keyed by an opaque
-- `document_ref` — the logical document code the caller owns (e.g. a records
-- module `DOC-…` object code). It ties into the object world exactly like
-- `message_refs.ref_code` does: a text code resolved/authorized at read time,
-- with no premature FK to a table that does not exist. When the records-archive
-- domain lands it can FK `document_ref` to its code, or migrate to a real id.
--
-- Blobs live in object storage (SeaweedFS, the existing evidence store); only
-- the storage key + content hash are recorded here. Versions are immutable by
-- GRANT: `console_rt` gets SELECT + INSERT only, never UPDATE/DELETE.

-- console-gate: audited-table document_versions
CREATE TABLE document_versions (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id        UUID        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    -- Logical document identity. Monotonic version numbering is scoped to
    -- (org_id, document_ref).
    document_ref  TEXT        NOT NULL CHECK (document_ref = btrim(document_ref) AND char_length(document_ref) BETWEEN 1 AND 200),
    -- 1-based, gap-free per document (COALESCE(MAX,0)+1 at insert time; the
    -- UNIQUE below rejects a concurrent duplicate).
    version_no    INT         NOT NULL CHECK (version_no > 0),
    -- SHA-256 hex of the stored blob. Also the material for the ONLYOFFICE
    -- document.key (host derives key = f(document_ref, version_no); a new
    -- version ⇒ a new key ⇒ the editor never serves a stale cache).
    content_hash  TEXT        NOT NULL CHECK (char_length(content_hash) BETWEEN 1 AND 128),
    -- Object-store key of the immutable blob (never returned to clients).
    storage_key   TEXT        NOT NULL CHECK (char_length(storage_key) BETWEEN 1 AND 1024),
    -- docx | xlsx | pptx (validated in the application layer).
    file_type     TEXT        NOT NULL CHECK (char_length(file_type) BETWEEN 1 AND 16),
    byte_size     BIGINT      NOT NULL CHECK (byte_size >= 0),
    -- The ONLYOFFICE document.key of the editing session that produced this
    -- version (set only by the force-save callback). NULL for restore/ingested
    -- versions. Drives callback idempotency: DocumentServer may retry a save, so
    -- one (org, source_key) maps to exactly one stored version.
    source_key    TEXT        NULL CHECK (source_key IS NULL OR char_length(source_key) BETWEEN 1 AND 128),
    -- Rollback lineage: the version_no this row non-destructively restored.
    restored_from INT         NULL CHECK (restored_from IS NULL OR restored_from > 0),
    -- Actor who produced the version. NULL = system-initiated (the machine
    -- force-save callback carries no user principal).
    created_by    UUID        NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, document_ref, version_no),
    CONSTRAINT document_versions_restored_from_fk
        FOREIGN KEY (org_id, document_ref, restored_from)
        REFERENCES document_versions (org_id, document_ref, version_no)
);

-- Latest-version lookup ("open the document") + the version list use the
-- UNIQUE (org_id, document_ref, version_no) index; Postgres can scan it
-- backward for newest-first ordering, so no duplicate DESC index is needed.
-- Growth/retention plan: rows stay immutable for the live audit trail. When
-- volume requires archival, copy cold document/version rows plus blobs into a
-- records-lifecycle archive by org/document after legal-hold and retention
-- deadlines clear, then introduce the narrow DELETE/partition migration that
-- policy requires instead of weakening this slice-0 append-only grant model.

-- Callback idempotency: at most one stored version per editing-session key.
CREATE UNIQUE INDEX idx_document_versions_source_key
    ON document_versions (org_id, source_key)
    WHERE source_key IS NOT NULL;

ALTER TABLE document_versions ENABLE ROW LEVEL SECURITY;
ALTER TABLE document_versions FORCE ROW LEVEL SECURITY;
CREATE POLICY org_isolation ON document_versions
    USING (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.current_org', true), '')::uuid);

-- Immutable by grant: rows are appended and read, never updated or deleted.
GRANT SELECT, INSERT ON document_versions TO console_rt;
