-- Validate the schedule provenance FK after 0106 installed it as NOT VALID.
ALTER TABLE workflow_runs VALIDATE CONSTRAINT workflow_runs_schedule_fk;
