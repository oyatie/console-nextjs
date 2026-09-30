-- 0054_comms_split_imap_dek.sql
-- B-mail-2 follow-up to 0053. The committed credential cipher
-- (console-comms-credential-cipher) seals EACH secret under its OWN fresh per-row
-- data key (DEK), so the SMTP and IMAP passwords each carry an INDEPENDENT
-- wrapped DEK + nonce. 0053 provisioned a single `dek_wrapped`/`dek_nonce`
-- column-pair, which can only hold one of the two wraps. Add the dedicated IMAP
-- DEK columns so both sealed credentials persist faithfully (the existing
-- `dek_wrapped`/`dek_nonce` now belong to the SMTP secret).
--
-- No data exists yet (the feature is unreleased on this branch — 0053 has never
-- been applied to a populated database), so the columns are added NOT NULL with
-- no backfill. The application layer always writes both wraps on every upsert,
-- so the NOT NULL constraint can never be violated at runtime. No Korean copy.

ALTER TABLE email_accounts
    ADD COLUMN imap_dek_wrapped BYTEA NOT NULL,
    ADD COLUMN imap_dek_nonce   BYTEA NOT NULL;
