-- Repair the evidence_reference CHECK on logistics_pod_evidence.
--
-- 0179 shipped `evidence_reference ~ '^evidence://[A-Za-z0-9._/-]{8,400}$'`.
-- Postgres caps a bounded repetition at RE_DUP_MAX = 255, so `{8,400}` is not a
-- valid regex. CHECK regexes compile lazily at INSERT time, so CREATE TABLE
-- succeeded and every subsequent INSERT into logistics_pod_evidence raised
-- `invalid regular expression: invalid repetition count(s)`. Proof of delivery
-- has therefore been unwritable since 0179 shipped.
--
-- The intended contract is unchanged: an 'evidence://' scheme followed by 8..400
-- characters drawn from [A-Za-z0-9._/-]. A bounded repetition cannot encode 400,
-- so the two concerns are split: the regex matches the character class unbounded,
-- and char_length carries the real window. 'evidence://' is 11 characters, so the
-- window is 11 + 8 = 19 through 11 + 400 = 411.
--
-- These are not bounds chosen here. 0182_create_equipment_3r.sql lines 45-49
-- already constrain the identical evidence:// field this exact way, down to the
-- 19..411 window and the note that the "window lives in char_length
-- ('evidence://' scheme is 11 characters)". 0179 is the outlier; this restores it
-- to the idiom the rest of the schema already uses.
ALTER TABLE logistics_pod_evidence
    DROP CONSTRAINT logistics_pod_evidence_evidence_reference_check;
ALTER TABLE logistics_pod_evidence
    ADD CONSTRAINT logistics_pod_evidence_evidence_reference_check
    CHECK (evidence_reference ~ '^evidence://[A-Za-z0-9._/-]+$'
        AND char_length(evidence_reference) BETWEEN 19 AND 411);

-- Repair logistics_terminal_immutable(), which the POD outage was masking.
--
-- 0179 raised on `OLD.status IN ('DELIVERED','SETTLED') AND NEW.status <>
-- OLD.status`, but the pilot lifecycle ends in SETTLED and settlement moves both
-- logistics_shipments and logistics_fulfillments from DELIVERED to SETTLED. The
-- trigger therefore forbade the one transition that reaches the terminal state,
-- leaving 'SETTLED' unreachable on both tables. It went unnoticed because no
-- shipment could reach DELIVERED while the CHECK above rejected every POD.
--
-- DELIVERED is not terminal; SETTLED is. Settlement is allowed through, and
-- every other transition out of DELIVERED or SETTLED still raises, so this is no
-- weaker than 0179 anywhere except the transition the pilot is designed to make.
CREATE OR REPLACE FUNCTION logistics_terminal_immutable() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
 IF OLD.status = 'SETTLED' AND NEW.status <> 'SETTLED' THEN RAISE EXCEPTION 'terminal logistics state is immutable'; END IF;
 IF OLD.status = 'DELIVERED' AND NEW.status NOT IN ('DELIVERED','SETTLED') THEN RAISE EXCEPTION 'terminal logistics state is immutable'; END IF;
 RETURN NEW; END $$;
