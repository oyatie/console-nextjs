-- 정비사 증거 MEDIA 파이프라인 — server-side processing (transcode/optimize) of
-- mechanic-uploaded photos & videos BEFORE they become the stored deliverable.
--
-- WHY: evidence now arrives in arbitrary, unoptimized formats (HEIC/large JPEG,
-- raw phone video). We must PROCESS BEFORE STORAGE: the mechanic uploads the
-- ORIGINAL to a tenant-scoped STAGING key, an async worker transcodes video to
-- 1080p H.264 / optimizes images, strips EXIF/GPS PII, generates a thumbnail,
-- writes the optimized artifact to the FINAL key, then deletes the staging
-- original. The evidence row tracks that lifecycle with a processing_status.
--
-- This migration ADDS NULLABLE columns to the existing, already RLS-scoped
-- `evidence_media` table (no new table). The table is already ENABLE + FORCE
-- ROW LEVEL SECURITY (0035 rollout) with the `org_isolation` policy keyed on the
-- `app.current_org` GUC, and `console_rt` already holds SELECT/INSERT/UPDATE/DELETE
-- via the 0031 `ALTER DEFAULT PRIVILEGES FOR ROLE console_app … GRANT … ON TABLES`.
-- Adding nullable columns therefore needs NO new grant or policy — the existing
-- org_isolation USING/WITH CHECK clauses continue to scope every read and the
-- worker's status UPDATE, and the 0031 enforce_org_id_immutable trigger still
-- prevents any tenant reassignment.
--
-- processing_status lifecycle:
--   PROCESSING  the original sits at staging_s3_key; a transcode job is queued
--   READY       the optimized artifact is at s3_key (+ thumbnail_s3_key); the
--               staging original has been deleted
--   FAILED      processing errored; the staging original is RETAINED for retry
--               and processing_error records the cause
--
-- BACKFILL: every pre-existing evidence row already holds its delivered object
-- at s3_key (the legacy direct-upload flow stored the raw original as the
-- deliverable). Those rows are, by definition, already "delivered", so the
-- column DEFAULT is 'READY' — the transcode poller must never pick them up.
-- Only NEW staging uploads are inserted with processing_status = 'PROCESSING'.
--
-- console-gate: audited-table evidence_media

ALTER TABLE evidence_media
    ADD COLUMN processing_status     TEXT NOT NULL DEFAULT 'READY' CHECK (
        processing_status IN ('PROCESSING','READY','FAILED')
    ),
    -- The tenant-scoped STAGING key where the mechanic PUTs the raw original.
    -- Org-prefixed (orgs/{org}/work-orders/…/staging/…); NULL once deleted after
    -- a successful transcode, and for legacy rows that never had a staging step.
    ADD COLUMN staging_s3_key        TEXT NULL CHECK (staging_s3_key IS NULL OR staging_s3_key <> ''),
    -- The tenant-scoped FINAL thumbnail/poster key (video first frame / image
    -- thumb). NULL until processing completes.
    ADD COLUMN thumbnail_s3_key      TEXT NULL CHECK (thumbnail_s3_key IS NULL OR thumbnail_s3_key <> ''),
    -- The mechanic's ORIGINAL upload content-type (e.g. video/quicktime,
    -- image/heic). content_type holds the OPTIMIZED artifact's type once READY.
    ADD COLUMN original_content_type TEXT NULL CHECK (original_content_type IS NULL OR original_content_type <> ''),
    -- Failure cause, retained so an operator/worker can retry a FAILED item.
    ADD COLUMN processing_error      TEXT NULL,
    ADD COLUMN processed_at          TIMESTAMPTZ NULL;

-- Hot poll path for the transcode worker: claim the oldest still-PROCESSING
-- rows. Partial index keeps it cheap as delivered (READY) rows accumulate.
CREATE INDEX idx_evidence_media_processing_queue
    ON evidence_media (org_id, created_at)
    WHERE processing_status = 'PROCESSING';
