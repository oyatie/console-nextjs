-- T1.4 evidence media and WORM replica verification state.
-- Evidence rows are operational state; every mutation is audited by the
-- storage/work-order adapters through with_audit.

CREATE TABLE evidence_media (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    work_order_id       UUID        NOT NULL REFERENCES work_orders(id) ON DELETE CASCADE,
    stage               TEXT        NOT NULL CHECK (
        stage IN ('REQUEST','BEFORE','DURING','AFTER','REPORT','OUTSOURCE_RESULT')
    ),
    s3_key              TEXT        NOT NULL CHECK (s3_key <> ''),
    content_type        TEXT        NOT NULL CHECK (content_type <> ''),
    size_bytes          BIGINT      NOT NULL CHECK (size_bytes >= 0),
    checksum_sha256     TEXT        CHECK (
        checksum_sha256 IS NULL OR checksum_sha256 ~ '^[A-Za-z0-9+/=_-]+$'
    ),
    uploaded_by         UUID        NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    worm_replica_status TEXT        NOT NULL DEFAULT 'PENDING' CHECK (
        worm_replica_status IN ('PENDING','VERIFIED','FAILED')
    ),
    retry_count         INTEGER     NOT NULL DEFAULT 0 CHECK (retry_count >= 0),
    next_retry_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_error          TEXT,
    verified_at         TIMESTAMPTZ,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (s3_key)
);

CREATE INDEX idx_evidence_media_work_order_stage
    ON evidence_media (work_order_id, stage, worm_replica_status);

CREATE INDEX idx_evidence_media_replication_queue
    ON evidence_media (worm_replica_status, next_retry_at, retry_count)
    WHERE worm_replica_status IN ('PENDING','FAILED');

CREATE VIEW unverified_evidence_admin_queue AS
SELECT
    e.id,
    e.work_order_id,
    w.request_no,
    w.branch_id,
    e.stage,
    e.s3_key,
    e.content_type,
    e.size_bytes,
    e.uploaded_by,
    e.worm_replica_status,
    e.retry_count,
    e.next_retry_at,
    e.last_error,
    e.created_at,
    e.updated_at
FROM evidence_media e
JOIN work_orders w ON w.id = e.work_order_id
WHERE e.worm_replica_status IN ('PENDING','FAILED');
